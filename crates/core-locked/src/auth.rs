//! Sesi admin, token CSRF, dan pembatasan percobaan login.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use subtle::ConstantTimeEq;

use crate::crypto::random_token;

pub const SESSION_COOKIE: &str = "mp_sid";
pub const CSRF_HEADER: &str = "x-csrf-token";

#[derive(Clone, Debug)]
pub struct Session {
    pub csrf: String,
    pub expires: Instant,
}

/// Sesi disimpan di memori: restart panel = login ulang. Ringan dan tanpa tabel.
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

    /// Membuat sesi baru, mengembalikan (token sesi, token CSRF).
    pub fn create(&self) -> (String, String) {
        let token = random_token();
        let csrf = random_token();
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, s| s.expires > now);
        map.insert(
            token.clone(),
            Session {
                csrf: csrf.clone(),
                expires: now + self.ttl,
            },
        );
        (token, csrf)
    }

    pub fn get(&self, token: &str) -> Option<Session> {
        let mut map = self.inner.lock().unwrap();
        match map.get(token) {
            Some(s) if s.expires > Instant::now() => Some(s.clone()),
            Some(_) => {
                map.remove(token);
                None
            }
            None => None,
        }
    }

    pub fn remove(&self, token: &str) {
        self.inner.lock().unwrap().remove(token);
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
        let s = Sessions::new(Duration::from_millis(20));
        let (t, csrf) = s.create();
        assert_eq!(s.get(&t).unwrap().csrf, csrf);
        std::thread::sleep(Duration::from_millis(30));
        assert!(s.get(&t).is_none());
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
