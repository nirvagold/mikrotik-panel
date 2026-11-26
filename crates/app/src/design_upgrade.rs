//! Saat desain bawaan berganti, versi draft/aktif yang masih persis sama dengan desain
//! bawaan lama diganti ke bawaan baru. Desain yang sudah diubah admin atau agen tidak disentuh.

use panel_agent::design::{DesignSpec, default_spec};
use rusqlite::Connection;
use serde_json::Value;

/// Desain bawaan rilis sebelumnya (dibandingkan setelah dinormalkan).
const LEGACY_DEFAULTS: &[&str] = &[include_str!("legacy_designs/noc_v0_1.json")];

fn canonical(spec: &str) -> Option<Value> {
    let spec: DesignSpec = serde_json::from_str(spec).ok()?;
    serde_json::to_value(spec.normalized()).ok()
}

/// Mengembalikan jumlah baris yang diperbarui.
pub fn upgrade(c: &mut Connection) -> rusqlite::Result<usize> {
    let legacy: Vec<Value> = LEGACY_DEFAULTS.iter().filter_map(|s| canonical(s)).collect();
    let new = default_spec();
    let new_value = serde_json::to_value(&new).expect("spec bawaan");
    let new_text = new_value.to_string();
    let tx = c.transaction()?;
    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT id, spec FROM design_versions WHERE status IN ('draft', 'active')")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    let mut changed = 0;
    for (id, spec) in rows {
        let Some(current) = canonical(&spec) else { continue };
        if current != new_value && legacy.contains(&current) {
            changed += tx.execute(
                "UPDATE design_versions SET spec = ?1, custom_css = ?2 WHERE id = ?3",
                (&new_text, &new.custom_css, &id),
            )?;
        }
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE design_versions (id TEXT PRIMARY KEY, parent_id TEXT, spec TEXT, custom_css TEXT, source TEXT, status TEXT, created_at INTEGER);",
        )
        .unwrap();
        c
    }

    #[test]
    fn replaces_only_untouched_legacy_default() {
        let mut c = db();
        let legacy = LEGACY_DEFAULTS[0];
        let mut custom: Value = serde_json::from_str(legacy).unwrap();
        custom["tokens"]["radius"] = 14.into();
        c.execute("INSERT INTO design_versions VALUES ('a', NULL, ?1, '', 'default', 'active', 0)", [legacy]).unwrap();
        c.execute("INSERT INTO design_versions VALUES ('d', 'a', ?1, '', 'agent', 'draft', 1)", [custom.to_string()]).unwrap();
        c.execute("INSERT INTO design_versions VALUES ('old', NULL, ?1, '', 'default', 'archived', 0)", [legacy]).unwrap();
        assert_eq!(upgrade(&mut c).unwrap(), 1);
        let spec = |id: &str| -> String { c.query_row("SELECT spec FROM design_versions WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
        assert_eq!(canonical(&spec("a")), Some(serde_json::to_value(default_spec()).unwrap()));
        assert_eq!(canonical(&spec("d")), canonical(&custom.to_string()));
        assert_eq!(spec("old"), legacy);
        assert_eq!(upgrade(&mut c).unwrap(), 0, "idempoten");
    }
}
