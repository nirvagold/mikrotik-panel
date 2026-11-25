use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("gagal membaca {0}: {1}")]
    Read(PathBuf, std::io::Error),
    #[error("format konfigurasi salah: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("konfigurasi tidak valid: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Alamat bind, contoh "127.0.0.1".
    #[serde(default = "default_bind")]
    pub bind: IpAddr,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_db")]
    pub database: PathBuf,
    /// Kunci 32 byte (base64) untuk enkripsi rahasia di config dan database.
    pub secret_key: String,
    /// Hash Argon2 kata sandi admin.
    pub admin_password_hash: String,
    /// Cookie hanya dikirim lewat HTTPS. Wajib true bila diakses lewat internet.
    #[serde(default = "default_true")]
    pub cookie_secure: bool,
    #[serde(default = "default_session_hours")]
    pub session_hours: u64,
    /// IP reverse proxy tepercaya; hanya dari IP ini header X-Forwarded-For dipercaya.
    #[serde(default)]
    pub trusted_proxies: Vec<IpAddr>,
    pub router: RouterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub host: String,
    /// 8728 untuk API biasa, 8729 untuk API-SSL.
    #[serde(default = "default_api_port")]
    pub port: u16,
    pub user: String,
    /// Kata sandi router terenkripsi (keluaran `mikrotik-panel encrypt`).
    pub password_encrypted: String,
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
}

fn default_bind() -> IpAddr {
    IpAddr::from([127, 0, 0, 1])
}
fn default_port() -> u16 {
    8765
}
fn default_db() -> PathBuf {
    PathBuf::from("/var/lib/mikrotik-panel/panel.sqlite3")
}
fn default_true() -> bool {
    true
}
fn default_session_hours() -> u64 {
    8
}
fn default_api_port() -> u16 {
    8728
}
fn default_poll() -> u64 {
    60
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read(path.into(), e))?;
        let config: Config = toml::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        crate::crypto::Cipher::from_base64(&self.secret_key)
            .map_err(|_| ConfigError::Invalid("secret_key harus 32 byte base64".into()))?;
        if !self.admin_password_hash.starts_with("$argon2") {
            return Err(ConfigError::Invalid(
                "admin_password_hash harus hash Argon2".into(),
            ));
        }
        if self.session_hours == 0 || self.session_hours > 24 * 7 {
            return Err(ConfigError::Invalid("session_hours harus 1–168".into()));
        }
        if self.router.poll_seconds < 10 {
            return Err(ConfigError::Invalid(
                "router.poll_seconds minimal 10".into(),
            ));
        }
        Ok(())
    }
}
