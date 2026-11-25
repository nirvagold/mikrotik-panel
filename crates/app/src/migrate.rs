//! Pindahkan data dari panel lama (panel.sqlite3 + private.json) ke database baru.
//! Database lama hanya dibaca, tidak diubah.

use std::path::Path;

use anyhow::{Context, bail};
use chrono::DateTime;
use panel_core::crypto::Cipher;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub daily: usize,
    pub coverage: usize,
    pub audit: usize,
    pub ai_settings: bool,
}

pub fn run(
    new: &mut Connection,
    old_db: &Path,
    old_config: Option<&Path>,
    cipher: &Cipher,
) -> anyhow::Result<Report> {
    let done: Option<String> = new
        .query_row(
            "SELECT value FROM settings WHERE key = 'migrated_from_legacy'",
            [],
            |r| r.get(0),
        )
        .ok();
    if let Some(at) = done {
        bail!("data lama sudah pernah dipindahkan ({at})");
    }
    let old = Connection::open_with_flags(old_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("gagal membuka {}", old_db.display()))?;
    let fernet = match old_config {
        Some(p) => {
            let cfg: Value = serde_json::from_str(&std::fs::read_to_string(p)?)
                .context("private.json tidak valid")?;
            let key = cfg["encryption_key"]
                .as_str()
                .context("encryption_key tidak ada di private.json")?;
            Some(fernet::Fernet::new(key).context("encryption_key lama tidak valid")?)
        }
        None => None,
    };
    let tx = new.transaction()?;
    let mut report = Report::default();

    let mut s = old.prepare("SELECT day, account, download, upload FROM daily")?;
    for row in s.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })? {
        let (day, account, dl, ul) = row?;
        tx.execute(
            "INSERT INTO daily (day, account, download, upload) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(day, account) DO UPDATE SET download = download + excluded.download, upload = upload + excluded.upload",
            (day, account, dl, ul),
        )?;
        report.daily += 1;
    }
    let mut s = old.prepare("SELECT day, seconds FROM coverage")?;
    for row in s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (day, secs) = row?;
        tx.execute(
            "INSERT INTO coverage (day, seconds) VALUES (?1, ?2)
             ON CONFLICT(day) DO UPDATE SET seconds = MIN(86400, seconds + excluded.seconds)",
            (day, secs),
        )?;
        report.coverage += 1;
    }
    let mut s = old.prepare("SELECT id, at, account, before, after, status FROM audit")?;
    let rows = s.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    for row in rows {
        let (id, at, account, before, after, status) = row?;
        let at = DateTime::parse_from_rfc3339(&at)
            .map(|d| d.timestamp())
            .unwrap_or(0);
        report.audit += tx.execute(
            "INSERT OR IGNORE INTO audit (id, at, actor, account, action, before, after, status)
             VALUES (?1, ?2, 'admin', ?3, 'quota.increase', ?4, ?5, ?6)",
            (
                id,
                at,
                account,
                before.map(|v| v.to_string()),
                after.map(|v| v.to_string()),
                status,
            ),
        )?;
    }
    let ai: Option<String> = old
        .query_row("SELECT value FROM settings WHERE key = 'ai'", [], |r| {
            r.get(0)
        })
        .ok();
    if let (Some(ai), Some(f)) = (ai, &fernet) {
        let mut v: Value = serde_json::from_str(&ai).context("pengaturan AI lama tidak valid")?;
        if let Some(token) = v["encrypted_key"].as_str() {
            let plain = f.decrypt(token).map_err(|_| {
                anyhow::anyhow!("API key AI lama tidak bisa dibuka dengan encryption_key")
            })?;
            v["encrypted_key"] = Value::String(cipher.encrypt(&String::from_utf8(plain)?));
            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('ai', ?1)",
                [v.to_string()],
            )?;
            report.ai_settings = true;
        }
    }
    tx.execute(
        "INSERT INTO settings (key, value) VALUES ('migrated_from_legacy', ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_data() {
        let dir = std::env::temp_dir().join(format!("mp-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old_path = dir.join("panel.sqlite3");
        let _ = std::fs::remove_file(&old_path);
        let key = fernet::Fernet::generate_key();
        let token = fernet::Fernet::new(&key).unwrap().encrypt(b"sk-rahasia");
        {
            let old = Connection::open(&old_path).unwrap();
            old.execute_batch(&format!(
                "CREATE TABLE daily (day TEXT, account TEXT, download INTEGER, upload INTEGER, PRIMARY KEY(day, account));
                 CREATE TABLE coverage (day TEXT PRIMARY KEY, seconds INTEGER);
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE audit (id TEXT PRIMARY KEY, at TEXT, account TEXT, before INTEGER, after INTEGER, status TEXT);
                 INSERT INTO daily VALUES ('2026-11-01', 'budi', 100, 10), ('2026-11-02', 'budi', 200, 20);
                 INSERT INTO coverage VALUES ('2026-11-01', 86400);
                 INSERT INTO audit VALUES ('a1', '2026-11-03T10:00:00+07:00', 'budi', 1, 2, 'Terverifikasi');
                 INSERT INTO settings VALUES ('ai', '{{\"url\":\"https://api.example.com\",\"protocol\":\"openai\",\"model\":\"m\",\"encrypted_key\":\"{token}\"}}');"
            ))
            .unwrap();
        }
        let cfg_path = dir.join("private.json");
        std::fs::write(&cfg_path, format!("{{\"encryption_key\": \"{key}\"}}")).unwrap();
        let cipher = Cipher::from_base64(&Cipher::generate_key()).unwrap();
        let store = crate::store::Store::open(&dir.join("new.sqlite3")).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let (c2, o2, p2) = (cipher.clone(), old_path.clone(), cfg_path.clone());
        let report = rt
            .block_on(
                store.run(move |c| Ok(run(c, &o2, Some(&p2), &c2).map_err(|e| e.to_string()))),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            report,
            Report {
                daily: 2,
                coverage: 1,
                audit: 1,
                ai_settings: true
            }
        );
        let ai: String = rt
            .block_on(store.run(|c| {
                c.query_row("SELECT value FROM settings WHERE key='ai'", [], |r| {
                    r.get(0)
                })
            }))
            .unwrap();
        let v: Value = serde_json::from_str(&ai).unwrap();
        assert_eq!(
            cipher
                .decrypt(v["encrypted_key"].as_str().unwrap())
                .unwrap(),
            "sk-rahasia"
        );
        // Tidak bisa dijalankan dua kali (mencegah trafik terhitung ganda).
        let (o3, c3) = (old_path.clone(), cipher.clone());
        let again = rt
            .block_on(store.run(move |c| Ok(run(c, &o3, None, &c3).map_err(|e| e.to_string()))))
            .unwrap();
        assert!(again.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
