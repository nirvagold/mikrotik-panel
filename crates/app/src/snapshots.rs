//! Kolektor snapshot konfigurasi router (read-only) tiap 15 menit. Hanya perubahan yang disimpan
//! (tabel router_snapshots: diff per bagian), salinan lengkap terakhir disimpan di settings.
//! Secret dimasking sebelum disimpan. Dipakai alat agen `snapshot_diff`.
use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::http::Shared;
use crate::routeros::{Command, Row};
use crate::store::now;

const INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Simpan riwayat perubahan 90 hari.
const KEEP_SECONDS: i64 = 90 * 24 * 3600;

/// Bagian konfigurasi yang dipantau.
const SECTIONS: &[&str] = &[
    "/system/identity",
    "/interface",
    "/interface/bridge/port",
    "/interface/vlan",
    "/ip/address",
    "/ip/route",
    "/ip/pool",
    "/ip/dns",
    "/ip/dhcp-server",
    "/ip/firewall/filter",
    "/ip/firewall/nat",
    "/ip/firewall/mangle",
    "/ip/service",
    "/ppp/profile",
    "/interface/pppoe-server/server",
    "/queue/simple",
    "/queue/type",
    "/user-manager/profile",
    "/user-manager/limitation",
    "/user-manager/router",
    "/radius",
    "/system/scheduler",
];

/// Kolom yang berubah sendiri (penghitung, status jalan) tidak dianggap perubahan konfigurasi.
fn volatile(key: &str) -> bool {
    const PARTS: &[&str] = &["byte", "packet", "last-", "uptime", "running", "link-downs", "drop", "error", "fp-", "rx-", "tx-", "invalid", "next-run", "run-count", "slave", "actual-", "inactive", "active"];
    PARTS.iter().any(|p| key.contains(p))
}

fn secret(key: &str) -> bool {
    ["password", "secret", "passphrase", "key", "private"].iter().any(|s| key.contains(s))
}

fn clean(rows: Vec<Row>) -> BTreeMap<String, BTreeMap<String, String>> {
    rows.into_iter()
        .filter(|r| r.get("dynamic").map(String::as_str) != Some("true"))
        .enumerate()
        .map(|(i, r)| {
            let id = r.get(".id").cloned().unwrap_or_else(|| format!("#{i}"));
            let fields = r
                .into_iter()
                .filter(|(k, _)| k != ".id" && !k.starts_with('.') && !volatile(k))
                .map(|(k, v)| {
                    let v = if secret(&k) && !v.is_empty() { "***".into() } else { v };
                    (k, v)
                })
                .collect();
            (id, fields)
        })
        .collect()
}

fn diff(old: &BTreeMap<String, BTreeMap<String, String>>, new: &BTreeMap<String, BTreeMap<String, String>>) -> Value {
    let label = |f: &BTreeMap<String, String>| ["name", "comment", "address", "dst-address", "interface", "chain"].iter().find_map(|k| f.get(*k).cloned()).unwrap_or_default();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (id, f) in new {
        match old.get(id) {
            None => added.push(json!({ "id": id, "fields": f })),
            Some(o) if o != f => {
                let mut fields = BTreeMap::new();
                for k in o.keys().chain(f.keys()) {
                    if o.get(k) != f.get(k) {
                        fields.insert(k.clone(), json!({ "old": o.get(k), "new": f.get(k) }));
                    }
                }
                changed.push(json!({ "id": id, "name": label(f), "fields": fields }));
            }
            _ => {}
        }
    }
    for (id, f) in old {
        if !new.contains_key(id) {
            removed.push(json!({ "id": id, "fields": f }));
        }
    }
    json!({ "added": added, "removed": removed, "changed": changed })
}

/// Ambil satu putaran snapshot. Mengembalikan jumlah bagian yang berubah.
pub async fn collect(state: &Shared) -> anyhow::Result<usize> {
    let mut changed = 0;
    for section in SECTIONS {
        let Ok(cmd) = Command::new(&format!("{section}/print")) else { continue };
        // Menu yang tidak ada (paket tidak terpasang) dilewati.
        let Ok(rows) = state.router.call(&cmd).await else { continue };
        let current = clean(rows);
        let text = serde_json::to_string(&current)?;
        let hash: String = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        let key = format!("snap:{section}");
        let (k2, sec) = (key.clone(), section.to_string());
        let prev: Option<String> = state
            .store
            .run(move |c| {
                use rusqlite::OptionalExtension;
                c.query_row("SELECT value FROM settings WHERE key = ?1", [k2], |r| r.get(0)).optional()
            })
            .await?;
        let prev_map: Option<BTreeMap<String, BTreeMap<String, String>>> = prev.as_deref().and_then(|p| serde_json::from_str(p).ok());
        if prev.as_deref() == Some(text.as_str()) {
            continue;
        }
        let d = prev_map.as_ref().map(|old| diff(old, &current));
        state
            .store
            .run(move |c| {
                c.execute("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", (&key, &text))?;
                if let Some(d) = d {
                    c.execute(
                        "INSERT INTO router_snapshots (taken_at, section, diff, full_hash) VALUES (?1, ?2, ?3, ?4)",
                        (now(), &sec, d.to_string(), &hash),
                    )?;
                }
                c.execute("DELETE FROM router_snapshots WHERE taken_at < ?1", [now() - KEEP_SECONDS])?;
                Ok(())
            })
            .await?;
        if prev_map.is_some() {
            changed += 1;
        }
    }
    Ok(changed)
}

pub fn spawn(state: Shared) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            match collect(&state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("snapshot router: {n} bagian konfigurasi berubah"),
                Err(err) => tracing::warn!("snapshot router gagal: {err}"),
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// Perubahan sejak `since_hours` jam lalu (alat agen `snapshot_diff`).
pub async fn since(state: &Shared, since_hours: i64) -> anyhow::Result<Value> {
    let from = now() - since_hours.clamp(1, 720) * 3600;
    let rows: Vec<Value> = state
        .store
        .run(move |c| {
            let mut s = c.prepare("SELECT taken_at, section, diff FROM router_snapshots WHERE taken_at >= ?1 ORDER BY taken_at DESC LIMIT 50")?;
            s.query_map([from], |r| {
                let diff: String = r.get(2)?;
                Ok(json!({ "at": r.get::<_, i64>(0)?, "section": r.get::<_, String>(1)?, "diff": serde_json::from_str::<Value>(&diff).unwrap_or(Value::Null) }))
            })?
            .collect()
        })
        .await?;
    let baseline: i64 = state
        .store
        .run(|c| c.query_row("SELECT COUNT(*) FROM settings WHERE key LIKE 'snap:%'", [], |r| r.get(0)))
        .await?;
    let note = if baseline == 0 {
        "Snapshot pertama belum diambil (berjalan 1 menit setelah panel menyala, lalu tiap 15 menit)."
    } else if rows.is_empty() {
        "Tidak ada perubahan konfigurasi yang tercatat pada rentang ini."
    } else {
        "Perubahan per bagian; secret dimasking."
    };
    Ok(json!({ "since_hours": since_hours, "changes": rows, "note": note }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn diff_ignores_counters_and_masks_secrets() {
        let old = clean(vec![row(&[(".id", "*1"), ("name", "ether1"), ("rx-byte", "10"), ("mtu", "1500")]), row(&[(".id", "*2"), ("name", "x"), ("password", "rahasia")])]);
        let new = clean(vec![row(&[(".id", "*1"), ("name", "ether1"), ("rx-byte", "99"), ("mtu", "1400")]), row(&[(".id", "*3"), ("name", "y")])]);
        assert_eq!(old["*2"]["password"], "***");
        let d = diff(&old, &new);
        assert_eq!(d["changed"][0]["fields"]["mtu"]["new"], "1400");
        assert!(d["changed"][0]["fields"].get("rx-byte").is_none());
        assert_eq!(d["added"][0]["id"], "*3");
        assert_eq!(d["removed"][0]["id"], "*2");
    }
}
