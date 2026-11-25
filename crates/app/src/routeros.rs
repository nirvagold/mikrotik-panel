//! Klien RouterOS API (port 8728). Protokolnya berbasis "kata" ber-awalan panjang, jadi nilai
//! parameter tidak pernah ditafsirkan sebagai perintah: tidak ada risiko injeksi seperti skrip SSH.
//! Satu koneksi persisten dipakai bergantian dan disambung ulang otomatis bila putus.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

pub type Row = BTreeMap<String, String>;

const MAX_WORD: usize = 1 << 20;
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("koneksi ke router gagal: {0}")]
    Io(#[from] std::io::Error),
    #[error("router tidak menjawab dalam batas waktu")]
    Timeout,
    #[error("router menolak perintah: {0}")]
    Trap(String),
    #[error("router memutus sesi API: {0}")]
    Fatal(String),
    #[error("login router gagal: {0}")]
    Login(String),
    #[error("perintah tidak valid: {0}")]
    Invalid(String),
    #[error("balasan router tidak dapat dibaca")]
    Protocol,
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

struct Conn {
    io: BufReader<Box<dyn Io>>,
}

impl Conn {
    async fn write_sentence(&mut self, words: &[String]) -> Result<(), RouterError> {
        let mut buf = Vec::new();
        for w in words {
            encode_len(w.len(), &mut buf);
            buf.extend_from_slice(w.as_bytes());
        }
        buf.push(0);
        let inner = self.io.get_mut();
        inner.write_all(&buf).await?;
        inner.flush().await?;
        Ok(())
    }

    async fn read_word(&mut self) -> Result<String, RouterError> {
        let len = read_len(&mut self.io).await?;
        if len > MAX_WORD {
            return Err(RouterError::Protocol);
        }
        let mut buf = vec![0u8; len];
        self.io.read_exact(&mut buf).await?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    async fn read_sentence(&mut self) -> Result<Vec<String>, RouterError> {
        let mut words = Vec::new();
        loop {
            let w = self.read_word().await?;
            if w.is_empty() {
                if words.is_empty() {
                    continue;
                }
                return Ok(words);
            }
            words.push(w);
        }
    }

    /// Kirim satu perintah, panggil `on_row` untuk tiap baris `!re` tanpa menampung semuanya.
    async fn call(
        &mut self,
        words: &[String],
        on_row: &mut (dyn FnMut(Row) + Send),
    ) -> Result<Row, RouterError> {
        self.write_sentence(words).await?;
        let mut trap: Option<String> = None;
        loop {
            let sentence = self.read_sentence().await?;
            let (head, rest) = sentence.split_first().ok_or(RouterError::Protocol)?;
            let attrs = parse_attrs(rest);
            match head.as_str() {
                "!re" => on_row(attrs),
                "!trap" => trap = Some(attrs.get("message").cloned().unwrap_or_default()),
                "!done" => return trap.map_or(Ok(attrs), |m| Err(RouterError::Trap(m))),
                "!fatal" => return Err(RouterError::Fatal(rest.join(" "))),
                // !empty (RouterOS 7.18+) dan balasan lain diabaikan.
                _ => {}
            }
        }
    }
}

fn parse_attrs(words: &[String]) -> Row {
    words
        .iter()
        .filter_map(|w| {
            let body = w.strip_prefix('=')?;
            let (k, v) = body.split_once('=').unwrap_or((body, ""));
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

fn encode_len(len: usize, out: &mut Vec<u8>) {
    let l = len as u32;
    match len {
        0..=0x7F => out.push(l as u8),
        0x80..=0x3FFF => out.extend_from_slice(&((l | 0x8000) as u16).to_be_bytes()),
        0x4000..=0x1F_FFFF => out.extend_from_slice(&(l | 0xC0_0000).to_be_bytes()[1..]),
        0x20_0000..=0x0FFF_FFFF => out.extend_from_slice(&(l | 0xE000_0000).to_be_bytes()),
        _ => {
            out.push(0xF0);
            out.extend_from_slice(&l.to_be_bytes());
        }
    }
}

async fn read_len<R: AsyncRead + Unpin>(r: &mut R) -> Result<usize, RouterError> {
    let first = r.read_u8().await?;
    let (extra, initial) = match first {
        b if b & 0x80 == 0x00 => (0, b as u32),
        b if b & 0xC0 == 0x80 => (1, (b & 0x3F) as u32),
        b if b & 0xE0 == 0xC0 => (2, (b & 0x1F) as u32),
        b if b & 0xF0 == 0xE0 => (3, (b & 0x0F) as u32),
        0xF0 => (4, 0),
        _ => return Err(RouterError::Protocol),
    };
    let mut value = initial;
    for _ in 0..extra {
        value = (value << 8) | r.read_u8().await? as u32;
    }
    Ok(value as usize)
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path[1..].split('/').all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')
        })
}

/// Satu perintah API. Nilai parameter aman apa pun isinya karena dikirim sebagai kata terpisah.
#[derive(Debug, Clone)]
pub struct Command {
    words: Vec<String>,
}

impl Command {
    /// `path` contoh: "/ppp/active/print".
    pub fn new(path: &str) -> Result<Self, RouterError> {
        if !valid_path(path) {
            return Err(RouterError::Invalid(format!("path {path:?}")));
        }
        Ok(Self {
            words: vec![path.to_string()],
        })
    }

    pub fn arg(mut self, key: &str, value: impl AsRef<str>) -> Result<Self, RouterError> {
        if !valid_key(key) {
            return Err(RouterError::Invalid(format!("parameter {key:?}")));
        }
        self.words.push(format!("={key}={}", value.as_ref()));
        Ok(self)
    }

    /// Filter `?key=value` (cocok persis).
    pub fn filter(mut self, key: &str, value: impl AsRef<str>) -> Result<Self, RouterError> {
        if !valid_key(key) {
            return Err(RouterError::Invalid(format!("filter {key:?}")));
        }
        self.words.push(format!("?{key}={}", value.as_ref()));
        Ok(self)
    }

    /// Hanya ambil kolom tertentu: hemat bandwidth dan memori.
    #[allow(dead_code)] // dipakai telemetri (tahap 2)
    pub fn proplist(self, keys: &[&str]) -> Result<Self, RouterError> {
        if !keys.iter().all(|k| valid_key(k)) {
            return Err(RouterError::Invalid("proplist".into()));
        }
        self.arg(".proplist", keys.join(","))
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &str {
        &self.words[0]
    }
}

#[derive(Debug, Clone)]
pub struct RouterTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    /// Sidik jari sertifikat (API-SSL). None = API biasa tanpa TLS.
    pub tls_fingerprint: Option<String>,
}

pub struct RouterOs {
    target: RouterTarget,
    conn: Mutex<Option<Conn>>,
}

impl RouterOs {
    pub fn new(target: RouterTarget) -> Self {
        Self {
            target,
            conn: Mutex::new(None),
        }
    }

    async fn connect(&self) -> Result<Conn, RouterError> {
        let addr = (self.target.host.as_str(), self.target.port);
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| RouterError::Timeout)??;
        tcp.set_nodelay(true)?;
        let io: Box<dyn Io> = match &self.target.tls_fingerprint {
            Some(fp) => Box::new(tls_handshake(tcp, &self.target.host, Some(fp)).await?.0),
            None => Box::new(tcp),
        };
        let mut conn = Conn {
            io: BufReader::with_capacity(8192, io),
        };
        login(&mut conn, &self.target.user, &self.target.password).await?;
        Ok(conn)
    }

    /// Jalankan perintah dan tampung semua baris (untuk hasil kecil).
    pub async fn call(&self, cmd: &Command) -> Result<Vec<Row>, RouterError> {
        let mut rows = Vec::new();
        self.each(cmd, &mut |r| rows.push(r)).await?;
        Ok(rows)
    }

    /// Jalankan perintah dan proses baris satu per satu (untuk hasil besar, tanpa batas 8 MB).
    pub async fn each(
        &self,
        cmd: &Command,
        on_row: &mut (dyn FnMut(Row) + Send),
    ) -> Result<Row, RouterError> {
        let mut guard = self.conn.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(self.connect().await?);
            }
            let conn = guard.as_mut().expect("koneksi ada");
            match tokio::time::timeout(CALL_TIMEOUT, conn.call(&cmd.words, on_row)).await {
                Ok(Ok(done)) => return Ok(done),
                Ok(Err(e @ (RouterError::Trap(_) | RouterError::Invalid(_)))) => return Err(e),
                Ok(Err(RouterError::Io(_))) if attempt == 0 => *guard = None,
                Ok(Err(e)) => {
                    *guard = None;
                    return Err(e);
                }
                Err(_) => {
                    *guard = None;
                    return Err(RouterError::Timeout);
                }
            }
        }
        Err(RouterError::Timeout)
    }

    /// Cek cepat untuk /api/health.
    pub async fn ping(&self) -> Result<String, RouterError> {
        let rows = self.call(&Command::new("/system/identity/print")?).await?;
        Ok(rows
            .first()
            .and_then(|r| r.get("name").cloned())
            .unwrap_or_default())
    }
}

async fn tls_handshake(
    tcp: TcpStream,
    host: &str,
    fingerprint: Option<&str>,
) -> Result<(tokio_rustls::client::TlsStream<TcpStream>, Option<String>), RouterError> {
    let (config, seen) = crate::tls::client_config(fingerprint);
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| RouterError::Invalid(format!("host {host:?}")))?;
    let stream = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_rustls::TlsConnector::from(config).connect(name, tcp),
    )
    .await
    .map_err(|_| RouterError::Timeout)??;
    let fp = seen.lock().unwrap().clone();
    Ok((stream, fp))
}

/// Baca sidik jari sertifikat API-SSL router (untuk dipasang di config oleh admin).
pub async fn read_fingerprint(host: &str, port: u16) -> Result<String, RouterError> {
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| RouterError::Timeout)??;
    let (_, fp) = tls_handshake(tcp, host, None).await?;
    fp.ok_or(RouterError::Protocol)
}

async fn login(conn: &mut Conn, user: &str, password: &str) -> Result<(), RouterError> {
    let words = vec![
        "/login".to_string(),
        format!("=name={user}"),
        format!("=password={password}"),
    ];
    match conn.call(&words, &mut |_| {}).await {
        Ok(_) => Ok(()),
        Err(RouterError::Trap(m)) | Err(RouterError::Fatal(m)) => Err(RouterError::Login(m)),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn length_encoding_roundtrip() {
        for len in [
            0usize,
            1,
            0x7F,
            0x80,
            0x3FFF,
            0x4000,
            0x1F_FFFF,
            0x20_0000,
            0x0FFF_FFFF,
            0x1000_0000,
        ] {
            let mut buf = Vec::new();
            encode_len(len, &mut buf);
            let mut slice = buf.as_slice();
            assert_eq!(read_len(&mut slice).await.unwrap(), len, "len {len:#x}");
        }
    }

    #[test]
    fn rejects_bad_paths_and_keys() {
        assert!(Command::new("/ppp/active/print").is_ok());
        assert!(Command::new("ppp").is_err());
        assert!(Command::new("/ppp//print").is_err());
        assert!(Command::new("/system/script/run; /reboot").is_err());
        assert!(Command::new("/x").unwrap().arg("name=evil", "v").is_err());
        // Nilai berisi karakter aneh tetap aman: ia satu kata utuh.
        let c = Command::new("/ppp/secret/set")
            .unwrap()
            .arg("comment", "a\n/system/reboot")
            .unwrap();
        assert_eq!(c.words[1], "=comment=a\n/system/reboot");
    }

    #[tokio::test]
    async fn call_reads_rows_and_traps() {
        let (client, server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let mut s = Conn {
                io: BufReader::new(Box::new(server)),
            };
            // login
            let login = s.read_sentence().await.unwrap();
            assert_eq!(login[0], "/login");
            s.write_sentence(&["!done".into()]).await.unwrap();
            // print
            let cmd = s.read_sentence().await.unwrap();
            assert_eq!(
                cmd,
                vec![
                    "/ppp/active/print".to_string(),
                    "?service=pppoe".to_string()
                ]
            );
            s.write_sentence(&[
                "!re".into(),
                "=name=budi".into(),
                "=address=10.0.0.2".into(),
            ])
            .await
            .unwrap();
            s.write_sentence(&["!re".into(), "=name=sari".into()])
                .await
                .unwrap();
            s.write_sentence(&["!done".into()]).await.unwrap();
            // trap
            s.read_sentence().await.unwrap();
            s.write_sentence(&["!trap".into(), "=message=no such item".into()])
                .await
                .unwrap();
            s.write_sentence(&["!done".into()]).await.unwrap();
            // !empty
            s.read_sentence().await.unwrap();
            s.write_sentence(&["!empty".into()]).await.unwrap();
            s.write_sentence(&["!done".into()]).await.unwrap();
        });
        let mut c = Conn {
            io: BufReader::new(Box::new(client)),
        };
        login(&mut c, "panel", "x").await.unwrap();
        let mut rows = Vec::new();
        let cmd = Command::new("/ppp/active/print")
            .unwrap()
            .filter("service", "pppoe")
            .unwrap();
        c.call(&cmd.words, &mut |r| rows.push(r)).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["address"], "10.0.0.2");
        let err = c
            .call(
                &Command::new("/ppp/secret/remove").unwrap().words,
                &mut |_| {},
            )
            .await;
        assert!(matches!(err, Err(RouterError::Trap(m)) if m == "no such item"));
        let mut n = 0;
        c.call(
            &Command::new("/queue/simple/print").unwrap().words,
            &mut |_| n += 1,
        )
        .await
        .unwrap();
        assert_eq!(n, 0);
    }
}

/// Router palsu untuk tes: menjawab login dan perintah sesuai tabel `path -> baris`.
/// Perintah `.../set` dicatat dan menerapkan perubahan ke baris print yang cocok (`.id`).
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    pub type Tables = Arc<StdMutex<std::collections::HashMap<String, Vec<Row>>>>;

    pub async fn start(tables: Tables) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let tables = tables.clone();
                tokio::spawn(async move {
                    let mut c = Conn {
                        io: BufReader::new(Box::new(sock)),
                    };
                    while let Ok(words) = c.read_sentence().await {
                        let path = words[0].clone();
                        let attrs = parse_attrs(&words[1..]);
                        let filters: Vec<(String, String)> = words[1..]
                            .iter()
                            .filter_map(|w| {
                                w.strip_prefix('?')?
                                    .split_once('=')
                                    .map(|(k, v)| (k.into(), v.into()))
                            })
                            .collect();
                        if let Some(menu) = path.strip_suffix("/set") {
                            let mut t = tables.lock().unwrap();
                            if let Some(rows) = t.get_mut(&format!("{menu}/print")) {
                                for r in
                                    rows.iter_mut().filter(|r| r.get(".id") == attrs.get(".id"))
                                {
                                    for (k, v) in &attrs {
                                        r.insert(k.clone(), v.clone());
                                    }
                                }
                            }
                        }
                        let rows: Vec<Row> = tables
                            .lock()
                            .unwrap()
                            .get(&path)
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|r| filters.iter().all(|(k, v)| r.get(k) == Some(v)))
                            .collect();
                        for r in rows {
                            let mut s = vec!["!re".to_string()];
                            s.extend(r.iter().map(|(k, v)| format!("={k}={v}")));
                            c.write_sentence(&s).await.unwrap();
                        }
                        c.write_sentence(&["!done".into()]).await.unwrap();
                    }
                });
            }
        });
        port
    }
}
