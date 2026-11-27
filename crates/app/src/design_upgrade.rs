//! Saat desain bawaan berganti, versi draft/aktif yang masih persis sama dengan desain
//! bawaan lama diganti ke bawaan baru. Desain yang sudah diubah admin atau agen tidak disentuh.

use panel_agent::design::{DesignSpec, default_spec};
use rusqlite::Connection;
use serde_json::Value;

/// Desain bawaan rilis sebelumnya (dibandingkan setelah dinormalkan).
const LEGACY_DEFAULTS: &[&str] = &[
    include_str!("legacy_designs/noc_v0_1.json"),
    include_str!("legacy_designs/vercel_v0_1_4.json"),
    include_str!("legacy_designs/vercel_v0_1_5.json"),
];

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
        let Some(mut current) = canonical(&spec) else { continue };
        if current != new_value && legacy.contains(&current) {
            changed += tx.execute(
                "UPDATE design_versions SET spec = ?1, custom_css = ?2 WHERE id = ?3",
                (&new_text, &new.custom_css, &id),
            )?;
        } else if merge_settings(&mut current) {
            // Desain yang sudah diubah: hanya menu pengaturan yang dirapikan, sisanya tetap.
            let valid = serde_json::from_value::<DesignSpec>(current.clone()).is_ok_and(|d| d.validate().is_ok());
            if let Some(text) = valid.then(|| canonical(&current.to_string())).flatten().map(|v| v.to_string()) {
                changed += tx.execute("UPDATE design_versions SET spec = ?1 WHERE id = ?2", (&text, &id))?;
            }
        }
    }
    tx.commit()?;
    Ok(changed)
}

/// Halaman builtin yang hanya berisi satu blok builtin `name`.
fn only_builtin(page: &Value, name: &str) -> bool {
    page["blocks"].as_array().is_some_and(|b| {
        b.len() == 1 && b[0]["kind"] == "builtin" && b[0]["props"]["name"] == name
    })
}

/// v0.1.9: halaman "Pengaturan AI", "Memori agen", dan "Agen AI" (isinya hanya petunjuk) digabung
/// menjadi satu halaman Pengaturan bertab. Hanya halaman yang isinya masih persis bawaan yang disentuh.
fn merge_settings(spec: &mut Value) -> bool {
    let Some(pages) = spec["pages"].as_array() else { return false };
    let has_settings = pages.iter().any(|p| {
        p["blocks"].as_array().is_some_and(|b| b.iter().any(|x| x["kind"] == "builtin" && x["props"]["name"] == "settings"))
    });
    let Some(ai_index) = pages.iter().position(|p| only_builtin(p, "ai_settings")) else { return false };
    if has_settings || pages.iter().any(|p| p["id"] == "pengaturan") {
        return false;
    }
    let old_id = pages[ai_index]["id"].as_str().unwrap_or_default().to_string();
    let drop: Vec<String> = pages
        .iter()
        .filter(|p| only_builtin(p, "agent") || only_builtin(p, "agent_memory"))
        .filter_map(|p| p["id"].as_str().map(str::to_owned))
        .collect();
    let pages = spec["pages"].as_array_mut().expect("pages");
    pages.retain(|p| !drop.iter().any(|d| p["id"] == d.as_str()));
    for p in pages.iter_mut() {
        if p["id"] == old_id.as_str() {
            p["id"] = "pengaturan".into();
            p["title"] = "Pengaturan".into();
            p["blocks"][0]["props"]["name"] = "settings".into();
        }
    }
    if let Some(items) = spec["nav"]["items"].as_array_mut() {
        items.retain(|i| !drop.iter().any(|d| i["page"] == d.as_str()));
        for i in items.iter_mut() {
            if i["page"] == old_id.as_str() {
                i["page"] = "pengaturan".into();
                i["label"] = "Pengaturan".into();
            }
        }
    }
    true
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
        assert_eq!(upgrade(&mut c).unwrap(), 2, "bawaan lama diganti + desain agen dirapikan");
        let spec = |id: &str| -> String { c.query_row("SELECT spec FROM design_versions WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
        assert_eq!(canonical(&spec("a")), Some(serde_json::to_value(default_spec()).unwrap()));
        // Desain yang diubah agen tetap miliknya; hanya halaman pengaturan yang digabung.
        let d: Value = serde_json::from_str(&spec("d")).unwrap();
        assert_eq!(d["tokens"]["radius"], 14);
        let ids: Vec<&str> = d["pages"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str()).collect();
        assert!(ids.contains(&"pengaturan"), "{ids:?}");
        assert!(!ids.contains(&"pengaturan-ai") && !ids.contains(&"memori"));
        assert!(d["nav"]["items"].as_array().unwrap().iter().any(|i| i["page"] == "pengaturan"));
        assert_eq!(spec("old"), legacy);
        assert_eq!(upgrade(&mut c).unwrap(), 0, "idempoten");
    }
}
