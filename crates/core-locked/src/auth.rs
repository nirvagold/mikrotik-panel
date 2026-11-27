//! Sesi admin, token CSRF, dan pembatasan percobaan login.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use subtle::ConstantTimeEq;

use crate::crypto::random_token;

pub const SESSION_COOKIE: &str = "mp_sid";
pub const CSRF_HEADER: &str = "x-csrf-token";

#[derive(Clone, Debug)]
pub struct Session {
    pub csrf: String,
    /// Detik Unix saat sesi berakhir (bertahan saat panel restart bila disimpan ke database).
    pub expires: u64,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sidik token sesi: yang disimpan di memori dan database hanya hash, bukan token cookie-nya.
pub fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Sesi baru yang perlu disimpan pemanggil.
pub struct NewSession {
    pub token: String,
    pub csrf: String,
    pub hash: String,
    pub expires: u64,
}

pub struct Sessions {
    ttl: Duration,
    inner: Mutex<HashMap<String, Session>>,
}

impl Sessions {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(&self) -> (String, String) {
        let s = self.create_new();
        (s.token, s.csrf)
    }

    pub fn create_new(&self) -> NewSession {
        let token = random_token();
        let csrf = random_token();
        let hash = token_hash(&token);
        let now = unix_now();
        let expires = now + self.ttl.as_secs();
        let mut map = self.inner.lock().unwrap();
        map.retain(|_, s| s.expires > now);
        map.insert(
            hash.clone(),
            Session {
                csrf: csrf.clone(),
                expires,
            },
        );
        NewSession {
            token,
            csrf,
            hash,
            expires,
        }
    }

    /// Pulihkan sesi dari database saat panel menyala.
    pub fn restore(&self, hash: String, csrf: String, expires: u64) {
        if expires > unix_now() {
            self.inner
                .lock()
                .unwrap()
                .insert(hash, Session { csrf, expires });
        }
    }

    pub fn get(&self, token: &str) -> Option<Session> {
        let hash = token_hash(token);
        let mut map = self.inner.lock().unwrap();
        match map.get(&hash) {
            Some(s) if s.expires > unix_now() => Some(s.clone()),
            Some(_) => {
                map.remove(&hash);
                None
            }
            None => None,
        }
    }

    pub fn remove(&self, token: &str) {
        self.inner.lock().unwrap().remove(&token_hash(token));
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }
}

pub fn tokens_match(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Maksimal `max_failures` gagal per IP dalam `window`; setelah itu dikunci selama `window`.
pub struct LoginLimiter {
    max_failures: u32,
    window: Duration,
    inner: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl LoginLimiter {
    pub fn new(max_failures: u32, window: Duration) -> Self {
        Self {
            max_failures,
            window,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Sisa waktu kunci bila IP sedang dikunci.
    pub fn locked(&self, ip: IpAddr) -> Option<Duration> {
        let map = self.inner.lock().unwrap();
        let (count, since) = map.get(&ip)?;
        let elapsed = since.elapsed();
        (*count >= self.max_failures && elapsed < self.window).then(|| self.window - elapsed)
    }

    /// Catat satu percobaan sebelum kata sandi diperiksa (atomik), supaya percobaan paralel
    /// tidak bisa melewati batas. Berhasil masuk menghapus catatan lewat `succeeded`.
    pub fn begin(&self, ip: IpAddr) -> Result<(), Duration> {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, (_, since)| now.duration_since(*since) < self.window);
        let entry = map.entry(ip).or_insert((0, now));
        if entry.0 >= self.max_failures {
            return Err(self.window.saturating_sub(now.duration_since(entry.1)));
        }
        entry.0 += 1;
        if entry.0 >= self.max_failures {
            entry.1 = now;
        }
        Ok(())
    }

    pub fn failed(&self, ip: IpAddr) {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, (_, since)| now.duration_since(*since) < self.window);
        let entry = map.entry(ip).or_insert((0, now));
        entry.0 += 1;
        if entry.0 >= self.max_failures {
            entry.1 = now;
        }
    }

    pub fn succeeded(&self, ip: IpAddr) {
        self.inner.lock().unwrap().remove(&ip);
    }
}

/// IP asli klien. Header X-Forwarded-For hanya dipercaya bila koneksi datang dari proxy tepercaya;
/// diambil alamat paling kanan yang bukan proxy tepercaya.
pub fn client_ip(peer: IpAddr, forwarded_for: Option<&str>, trusted: &[IpAddr]) -> IpAddr {
    if !trusted.contains(&peer) {
        return peer;
    }
    forwarded_for
        .into_iter()
        .flat_map(|h| h.split(','))
        .rev()
        .filter_map(|part| part.trim().parse::<IpAddr>().ok())
        .find(|ip| !trusted.contains(ip))
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_expire() {
        let s = Sessions::new(Duration::from_secs(60));
        let n = s.create_new();
        assert_eq!(s.get(&n.token).unwrap().csrf, n.csrf);
        assert_eq!(n.hash, token_hash(&n.token));
        // Sesi yang sudah lewat waktunya tidak dipulihkan dan tidak diterima.
        let other = Sessions::new(Duration::from_secs(60));
        other.restore(n.hash.clone(), n.csrf.clone(), 1);
        assert!(other.get(&n.token).is_none());
        // Sesi yang masih berlaku dipulihkan setelah "restart".
        other.restore(n.hash, n.csrf.clone(), n.expires);
        assert_eq!(other.get(&n.token).unwrap().csrf, n.csrf);
        s.remove(&n.token);
        assert!(s.get(&n.token).is_none());
    }

    #[test]
    fn limiter_locks_after_failures() {
        let l = LoginLimiter::new(3, Duration::from_secs(60));
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        for _ in 0..2 {
            l.failed(ip);
        }
        assert!(l.locked(ip).is_none());
        l.failed(ip);
        assert!(l.locked(ip).is_some());
        l.succeeded(ip);
        assert!(l.locked(ip).is_none());
        // Percobaan dicatat sebelum diperiksa: percobaan ke-4 ditolak walau belum ada yang selesai.
        for _ in 0..3 {
            assert!(l.begin(ip).is_ok());
        }
        assert!(l.begin(ip).is_err());
    }

    #[test]
    fn forwarded_only_from_trusted_proxy() {
        let proxy: IpAddr = "127.0.0.1".parse().unwrap();
        let outsider: IpAddr = "203.0.113.9".parse().unwrap();
        let header = Some("1.2.3.4, 198.51.100.7");
        assert_eq!(client_ip(outsider, header, &[proxy]), outsider);
        assert_eq!(
            client_ip(proxy, header, &[proxy]),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(client_ip(proxy, None, &[proxy]), proxy);
    }

    #[test]
    fn token_compare() {
        assert!(tokens_match("abc", "abc"));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", "abcd"));
    }
}
