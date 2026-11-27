//! Perubahan router dari agen dan tombol aksi custom: pelanggan (User Manager), tombol tersimpan
//! (simpan/ubah/hapus/jalankan/kembalikan), dan reboot. Semua dipanggil SETELAH izin admin
//! dipenuhi (kartu usulan atau dialog tombol); validasi perintah tetap diulang di sini.
use std::collections::{BTreeMap, BTreeSet};

use panel_core::crypto::random_token;
use panel_core::policy::Permission;
use panel_core::policy::command::{
    Command as PolicyCommand, FillContext, Input, ParamSpec, Value as PV, validate_params,
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

use crate::http::Shared;
use crate::routeros::{Command, Row};
use crate::store::now;

type R<T> = Result<T, String>;

fn e(err: impl ToString) -> String {
    err.to_string()
}

/// Batas objek yang boleh diubah satu perintah.
const MAX_OBJECTS: usize = 20;

async fn print(st: &Shared, path: &str, filters: &[(&str, &str)], props: &[&str]) -> R<Vec<Row>> {
    let mut cmd = Command::new(&format!("{path}/print")).map_err(e)?;
    for (k, v) in filters {
        cmd = cmd.filter(k, v).map_err(e)?;
    }
    if !props.is_empty() {
        cmd = cmd.proplist(props).map_err(e)?;
    }
    st.router.call(&cmd).await.map_err(e)
}

async fn call(st: &Shared, path: &str, args: &[(&str, &str)]) -> R<Row> {
    let mut cmd = Command::new(path).map_err(e)?;
    for (k, v) in args {
        cmd = cmd.arg(k, v).map_err(e)?;
    }
    st.router.each(&cmd, &mut |_| {}).await.map_err(e)
}

fn ids(rows: &[Row]) -> Vec<String> {
    rows.iter().filter_map(|r| r.get(".id").cloned()).collect()
}

async fn um_user(st: &Shared, account: &str) -> R<Row> {
    let rows = print(st, "/user-manager/user", &[("name", account)], &[".id", "name", "disabled", "attributes", "group", "shared-users", "comment", "caller-id"]).await?;
    match rows.as_slice() {
        [row] => Ok(row.clone()),
        [] => Err(format!("Pelanggan {account} tidak ditemukan di User Manager.")),
        _ => Err(format!("Ada lebih dari satu user bernama {account} di User Manager.")),
    }
}

// ---------- Pelanggan ----------

/// Putus semua sesi aktif pelanggan (PPPoE dan hotspot). Pelanggan bisa langsung tersambung lagi.
pub async fn disconnect(st: &Shared, account: &str) -> R<String> {
    let mut removed = 0;
    let mut errors = Vec::new();
    for (path, key) in [("/ppp/active", "name"), ("/ip/hotspot/active", "user")] {
        match print(st, path, &[(key, account)], &[".id"]).await {
            Ok(rows) => {
                for id in ids(&rows) {
                    call(st, &format!("{path}/remove"), &[(".id", &id)]).await?;
                    removed += 1;
                }
            }
            Err(err) => errors.push(err),
        }
    }
    if errors.len() == 2 {
        return Err(format!("Gagal membaca sesi aktif: {}", errors.join("; ")));
    }
    Ok(match removed {
        0 => format!("{account} sedang tidak punya sesi aktif."),
        n => format!("{n} sesi {account} diputus; pelanggan bisa tersambung lagi."),
    })
}

/// Aktifkan atau nonaktifkan user User Manager. Menonaktifkan juga memutus sesinya.
pub async fn set_enabled(st: &Shared, account: &str, enabled: bool) -> R<String> {
    let user = um_user(st, account).await?;
    let id = user.get(".id").cloned().unwrap_or_default();
    call(st, "/user-manager/user/set", &[(".id", &id), ("disabled", if enabled { "no" } else { "yes" })]).await?;
    if enabled {
        return Ok(format!("{account} diaktifkan."));
    }
    let cut = disconnect(st, account).await.unwrap_or_else(|err| format!("Sesi belum diputus: {err}"));
    Ok(format!("{account} dinonaktifkan. {cut}"))
}

/// Ganti paket: pasang profil baru lalu lepas profil lama milik user itu.
pub async fn set_profile(st: &Shared, account: &str, profile: &str) -> R<String> {
    um_user(st, account).await?;
    if print(st, "/user-manager/profile", &[("name", profile)], &["name"]).await?.is_empty() {
        return Err(format!("Profil {profile} tidak ada di User Manager."));
    }
    let current = print(st, "/user-manager/user-profile", &[("user", account)], &[".id", "profile"]).await?;
    if current.len() == 1 && current[0].get("profile").map(String::as_str) == Some(profile) {
        return Ok(format!("{account} sudah memakai profil {profile}."));
    }
    call(st, "/user-manager/user-profile/add", &[("user", account), ("profile", profile)]).await?;
    let mut old = Vec::new();
    for row in &current {
        if let Some(id) = row.get(".id") {
            call(st, "/user-manager/user-profile/remove", &[(".id", id)]).await?;
            old.push(row.get("profile").cloned().unwrap_or_default());
        }
    }
    let was = if old.is_empty() { "tanpa profil".to_string() } else { old.join(", ") };
    Ok(format!("Profil {account}: {was} → {profile}. Berlaku saat pelanggan tersambung ulang."))
}

/// Field user User Manager yang boleh diubah lewat set_radius_user. Kunci lain dianggap atribut RADIUS.
const USER_FIELDS: &[&str] = &["group", "shared-users", "comment", "caller-id"];

fn valid_attr_name(name: &str) -> bool {
    (2..=64).contains(&name.len())
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "yes" } else { "no" }.into()),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

/// Atur field user dan atribut RADIUS (mis. Mikrotik-Rate-Limit). Nilai kosong = hapus atribut.
pub async fn set_radius(st: &Shared, account: &str, attrs: &serde_json::Map<String, Value>) -> R<String> {
    if attrs.is_empty() || attrs.len() > 16 {
        return Err("Isi 1 sampai 16 atribut.".into());
    }
    let user = um_user(st, account).await?;
    let id = user.get(".id").cloned().unwrap_or_default();
    let mut radius: Vec<(String, String)> = user
        .get("attributes")
        .map(|a| a.split(',').filter_map(|p| p.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect())
        .unwrap_or_default();
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut changes = Vec::new();
    for (key, value) in attrs {
        let value = scalar(value).ok_or_else(|| format!("Nilai {key} harus teks atau angka."))?;
        if value.len() > 128 || value.contains(['\n', '\r', ',']) {
            return Err(format!("Nilai {key} terlalu panjang atau berisi koma/baris baru."));
        }
        if USER_FIELDS.contains(&key.as_str()) {
            changes.push(format!("{key}: {} → {value}", user.get(key).map_or("-", String::as_str)));
            fields.push((key.clone(), value));
        } else if valid_attr_name(key) {
            let old = radius.iter().position(|(k, _)| k.eq_ignore_ascii_case(key));
            let before = old.map(|i| radius[i].1.clone()).unwrap_or_else(|| "-".into());
            match (old, value.is_empty()) {
                (Some(i), true) => {
                    radius.remove(i);
                }
                (Some(i), false) => radius[i].1 = value.clone(),
                (None, false) => radius.push((key.clone(), value.clone())),
                (None, true) => {}
            }
            changes.push(format!("{key}: {before} → {}", if value.is_empty() { "dihapus" } else { &value }));
        } else {
            return Err(format!("`{key}` bukan field user atau nama atribut RADIUS yang valid."));
        }
    }
    let joined = radius.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(",");
    let mut args: Vec<(&str, &str)> = vec![(".id", &id)];
    for (k, v) in &fields {
        args.push((k, v));
    }
    if attrs.keys().any(|k| !USER_FIELDS.contains(&k.as_str())) {
        args.push(("attributes", &joined));
    }
    call(st, "/user-manager/user/set", &args).await?;
    Ok(format!("{account} diperbarui: {}. Berlaku saat pelanggan tersambung ulang.", changes.join("; ")))
}

/// Reboot router. Koneksi API biasanya terputus sebelum jawaban datang, jadi itu bukan kegagalan.
pub async fn reboot(st: &Shared) -> R<String> {
    match call(st, "/system/reboot", &[]).await {
        Ok(_) => Ok("Router sedang reboot; panel tersambung lagi setelah router menyala.".into()),
        Err(err) if err.contains("Trap") || err.contains("trap") => Err(format!("Router menolak reboot: {err}")),
        Err(_) => Ok("Perintah reboot terkirim; koneksi terputus karena router sedang restart.".into()),
    }
}

// ---------- Perintah dengan catatan nilai lama ----------

/// Jalankan perintah tervalidasi dan kembalikan (pesan, data untuk revert atau null).
pub async fn run_command(st: &Shared, cmd: &PolicyCommand) -> R<(String, Value)> {
    cmd.validate().map_err(e)?;
    if cmd.path == ["ip", "service"] {
        return Err("Perubahan /ip service hanya lewat usulan agen (dengan pengaman IP panel).".into());
    }
    let args = PolicyCommand::api_pairs(&cmd.args).map_err(e)?;
    let base = format!("/{}", cmd.path.join("/"));
    let keys: Vec<&str> = args.iter().map(|(k, _)| k.as_str()).collect();
    let build = |id: Option<&str>| -> R<Command> {
        let mut c = Command::new(&cmd.api_path()).map_err(e)?;
        if let Some(id) = id {
            c = c.arg(".id", id).map_err(e)?;
        }
        for (k, v) in &args {
            c = c.arg(k, v).map_err(e)?;
        }
        Ok(c)
    };
    if cmd.find.is_empty() {
        let mut before = Value::Null;
        if cmd.verb == "set" && !keys.is_empty() {
            let mut props = keys.clone();
            props.push(".id");
            let rows = print(st, &base, &[], &props).await.unwrap_or_default();
            if let [row] = rows.as_slice() {
                before = json!({ "kind": "set", "path": base, "items": [{ "id": row.get(".id"), "values": values_of(row, &keys) }] });
            }
        }
        let done = st.router.each(&build(None)?, &mut |_| {}).await.map_err(e)?;
        if cmd.verb == "add" {
            if let Some(ret) = done.get("ret") {
                before = json!({ "kind": "remove", "path": base, "ids": [ret] });
            }
        }
        return Ok(("Perintah dijalankan.".into(), before));
    }
    let mut finder = Command::new(&cmd.api_find_path()).map_err(e)?;
    for (k, v) in PolicyCommand::api_pairs(&cmd.find).map_err(e)? {
        finder = finder.filter(&k, v).map_err(e)?;
    }
    let mut props = keys.clone();
    props.extend([".id", "disabled"]);
    let rows = st.router.call(&finder.proplist(&props).map_err(e)?).await.map_err(e)?;
    if rows.is_empty() {
        return Err("Tidak ada objek yang cocok di router.".into());
    }
    if rows.len() > MAX_OBJECTS {
        return Err(format!("{} objek cocok; batas {MAX_OBJECTS} per perintah.", rows.len()));
    }
    let found = ids(&rows);
    let before = match cmd.verb.as_str() {
        "set" => json!({ "kind": "set", "path": base,
            "items": rows.iter().map(|r| json!({ "id": r.get(".id"), "values": values_of(r, &keys) })).collect::<Vec<_>>() }),
        "disable" | "enable" => {
            // Kembalikan tiap objek ke keadaan semula (yang sudah nonaktif tetap nonaktif).
            let was_disabled: Vec<&String> = rows.iter().filter(|r| r.get("disabled").map(String::as_str) == Some("true")).filter_map(|r| r.get(".id")).collect();
            let was_enabled: Vec<&String> = rows.iter().filter(|r| r.get("disabled").map(String::as_str) != Some("true")).filter_map(|r| r.get(".id")).collect();
            json!({ "kind": "toggle", "path": base, "disable": was_disabled, "enable": was_enabled })
        }
        _ => Value::Null,
    };
    let mut done: Vec<&String> = Vec::new();
    for id in &found {
        if let Err(err) = st.router.call(&build(Some(id))?).await {
            if done.is_empty() {
                return Err(e(err));
            }
            // Sebagian objek sudah berubah: simpan nilai lama objek-objek itu agar tetap bisa dikembalikan.
            let partial = only_ids(&before, &done);
            return Err(Partial::message(
                format!("Gagal setelah {} dari {} objek berubah: {err}", done.len(), found.len()),
                partial,
            ));
        }
        done.push(id);
    }
    Ok((format!("Perintah dijalankan pada {} objek.", found.len()), before))
}

/// Data revert yang dibatasi pada objek yang benar-benar sudah berubah.
fn only_ids(before: &Value, done: &[&String]) -> Value {
    let keep = |v: &Value| v.as_str().is_some_and(|id| done.iter().any(|d| d.as_str() == id));
    let mut out = before.clone();
    if let Some(items) = out.get_mut("items").and_then(Value::as_array_mut) {
        items.retain(|i| keep(&i["id"]));
    }
    for key in ["disable", "enable"] {
        if let Some(ids) = out.get_mut(key).and_then(Value::as_array_mut) {
            ids.retain(keep);
        }
    }
    out
}

/// Kegagalan sebagian dibawa lewat pesan error: penanda + JSON data revert setelah baris pertama.
struct Partial;

impl Partial {
    const MARK: &'static str = "\n#partial-revert#";

    fn message(text: String, before: Value) -> String {
        if before.is_null() { text } else { format!("{text}{}{before}", Self::MARK) }
    }

    /// Pisahkan pesan untuk pengguna dan data revert (bila ada).
    fn split(msg: &str) -> (String, Option<Value>) {
        match msg.split_once(Self::MARK) {
            Some((text, data)) => (text.to_string(), serde_json::from_str(data).ok()),
            None => (msg.to_string(), None),
        }
    }
}

/// Bungkus `run_command` untuk pemanggil yang hanya butuh pesan (tanpa data revert).
pub async fn run_command_text(st: &Shared, cmd: &PolicyCommand) -> R<(String, Value)> {
    run_command(st, cmd).await.map_err(|m| Partial::split(&m).0)
}

fn values_of(row: &Row, keys: &[&str]) -> Value {
    Value::Object(keys.iter().map(|k| (k.to_string(), row.get(*k).map_or(Value::Null, |v| json!(v)))).collect())
}

/// Terapkan data revert dari `run_command`.
async fn apply_revert(st: &Shared, data: &Value) -> R<String> {
    let path = data["path"].as_str().ok_or("Data pengembalian rusak.")?;
    Command::new(&format!("{path}/print")).map_err(e)?; // validasi path
    let mut n = 0;
    match data["kind"].as_str() {
        Some("set") => {
            for item in data["items"].as_array().into_iter().flatten() {
                let id = item["id"].as_str();
                let mut set: Vec<(String, String)> = Vec::new();
                let mut unset: Vec<String> = Vec::new();
                for (k, v) in item["values"].as_object().into_iter().flatten() {
                    match v.as_str() {
                        Some(s) => set.push((k.clone(), s.to_string())),
                        None => unset.push(k.clone()),
                    }
                }
                let mut args: Vec<(&str, &str)> = id.map(|i| vec![(".id", i)]).unwrap_or_default();
                args.extend(set.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                if !set.is_empty() {
                    call(st, &format!("{path}/set"), &args).await?;
                }
                for key in &unset {
                    let mut a: Vec<(&str, &str)> = id.map(|i| vec![(".id", i)]).unwrap_or_default();
                    a.push(("value-name", key));
                    let _ = call(st, &format!("{path}/unset"), &a).await;
                }
                n += 1;
            }
        }
        Some("remove") => {
            for id in data["ids"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                call(st, &format!("{path}/remove"), &[(".id", id)]).await?;
                n += 1;
            }
        }
        Some("toggle") => {
            for (verb, key) in [("disable", "disable"), ("enable", "enable")] {
                for id in data[key].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    call(st, &format!("{path}/{verb}"), &[(".id", id)]).await?;
                    n += 1;
                }
            }
        }
        _ => return Err("Eksekusi ini tidak bisa dikembalikan otomatis.".into()),
    }
    Ok(format!("Nilai lama dikembalikan pada {n} objek."))
}

// ---------- Tombol aksi custom ----------

fn risk_of(p: Permission) -> &'static str {
    match p {
        Permission::Free => "read",
        Permission::Approve => "write",
        Permission::Critical => "critical",
    }
}

pub fn permission_of(risk: &str) -> Permission {
    match risk {
        "read" => Permission::Free,
        "write" => Permission::Approve,
        _ => Permission::Critical,
    }
}

fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len()) && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Tombol tersimpan.
pub struct Action {
    pub id: String,
    pub label: String,
    pub description: String,
    pub command: PolicyCommand,
    pub params: Vec<ParamSpec>,
    pub risk: String,
}

impl Action {
    pub fn to_json(&self) -> Value {
        let mut shown = self.command.clone();
        for v in shown.find.values_mut().chain(shown.args.values_mut()) {
            if let PV::Param { param, .. } = v {
                *v = PV::Text(format!("<{param}>"));
            }
        }
        json!({ "id": self.id, "label": self.label, "description": self.description, "risk": self.risk,
                "params": self.params, "preview": shown.render().ok() })
    }
}

fn check_template(command: &PolicyCommand, params: &[ParamSpec]) -> R<Permission> {
    command.validate().map_err(e)?;
    validate_params(params).map_err(e)?;
    if command.path == ["ip", "service"] {
        return Err("Tombol tidak boleh mengubah /ip service.".into());
    }
    let declared: BTreeSet<String> = params.iter().map(|p| p.name.clone()).collect();
    if command.placeholders() != declared {
        return Err("Setiap placeholder harus dideklarasikan tepat sekali di params.".into());
    }
    Ok(command.permission())
}

pub async fn load(st: &Shared, id: &str) -> R<Action> {
    let id2 = id.to_string();
    let row = st
        .store
        .run(move |c| {
            c.query_row("SELECT label, template, params_schema, risk FROM custom_actions WHERE id = ?1", [id2], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })
            .optional()
        })
        .await
        .map_err(e)?
        .ok_or_else(|| format!("Tombol `{id}` tidak ditemukan."))?;
    let template: Value = serde_json::from_str(&row.1).map_err(e)?;
    Ok(Action {
        id: id.to_string(),
        label: row.0,
        description: template["description"].as_str().unwrap_or_default().to_string(),
        command: serde_json::from_value(template["command"].clone()).map_err(e)?,
        params: serde_json::from_str(&row.2).map_err(e)?,
        risk: row.3,
    })
}

async fn save(st: &Shared, a: &Action, insert: bool) -> R<()> {
    let template = json!({ "command": a.command, "description": a.description }).to_string();
    let params = serde_json::to_string(&a.params).map_err(e)?;
    let (id, label, risk) = (a.id.clone(), a.label.clone(), a.risk.clone());
    let changed = st
        .store
        .run(move |c| {
            if insert {
                c.execute(
                    "INSERT INTO custom_actions (id, label, template, params_schema, risk, placement, created_at, approved_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?6) ON CONFLICT(id) DO NOTHING",
                    (id, label, template, params, risk, now()),
                )
            } else {
                c.execute(
                    "UPDATE custom_actions SET label = ?2, template = ?3, params_schema = ?4, risk = ?5, approved_at = ?6 WHERE id = ?1",
                    (id, label, template, params, risk, now()),
                )
            }
        })
        .await
        .map_err(e)?;
    if changed == 0 {
        return Err(if insert { format!("ID tombol `{}` sudah dipakai.", a.id) } else { format!("Tombol `{}` tidak ditemukan.", a.id) });
    }
    st.hub.publish("actions.updated", json!({ "id": a.id }));
    Ok(())
}

fn label_of(input: &Value) -> R<String> {
    let label = input["label"].as_str().unwrap_or_default().trim().to_string();
    if label.is_empty() || label.chars().count() > 40 {
        return Err("Label tombol harus 1 sampai 40 karakter.".into());
    }
    Ok(label)
}

pub async fn create(st: &Shared, input: &Value) -> R<String> {
    let id = input["id"].as_str().unwrap_or_default();
    if !valid_id(id) {
        return Err("ID tombol hanya huruf kecil, angka, - dan _ (maks 40).".into());
    }
    let command: PolicyCommand = serde_json::from_value(input["command"].clone()).map_err(|err| format!("Perintah tidak valid: {err}"))?;
    let params: Vec<ParamSpec> = serde_json::from_value(input.get("params").cloned().unwrap_or(json!([]))).map_err(|err| format!("Parameter tidak valid: {err}"))?;
    let risk = risk_of(check_template(&command, &params)?);
    let a = Action { id: id.into(), label: label_of(input)?, description: input["description"].as_str().unwrap_or_default().chars().take(300).collect(), command, params, risk: risk.into() };
    save(st, &a, true).await?;
    let place = input["place"]["page"].as_str().map(|p| format!(" Tempatkan di halaman {p} dengan blok action_button {{action_id: \"{id}\"}}.")).unwrap_or_default();
    Ok(format!("Tombol \"{}\" ({id}, risiko {risk}) disimpan.{place}", a.label))
}

pub async fn update(st: &Shared, input: &Value) -> R<String> {
    let mut a = load(st, input["id"].as_str().unwrap_or_default()).await?;
    if input.get("label").is_some() {
        a.label = label_of(input)?;
    }
    if let Some(d) = input["description"].as_str() {
        a.description = d.chars().take(300).collect();
    }
    if let Some(c) = input.get("command") {
        a.command = serde_json::from_value(c.clone()).map_err(|err| format!("Perintah tidak valid: {err}"))?;
    }
    if let Some(p) = input.get("params") {
        a.params = serde_json::from_value(p.clone()).map_err(|err| format!("Parameter tidak valid: {err}"))?;
    }
    a.risk = risk_of(check_template(&a.command, &a.params)?).into();
    save(st, &a, false).await?;
    Ok(format!("Tombol \"{}\" diperbarui (risiko {}).", a.label, a.risk))
}

pub async fn delete(st: &Shared, id: &str) -> R<String> {
    let a = load(st, id).await?;
    let id2 = id.to_string();
    st.store
        .run(move |c| {
            c.execute("UPDATE action_runs SET action_id = NULL WHERE action_id = ?1", [&id2])?;
            c.execute("UPDATE recommendations SET action_id = NULL WHERE action_id = ?1", [&id2])?;
            c.execute("DELETE FROM custom_actions WHERE id = ?1", [&id2])
        })
        .await
        .map_err(e)?;
    st.hub.publish("actions.updated", json!({ "id": id }));
    Ok(format!("Tombol \"{}\" dihapus. Blok action_button yang memakainya akan tampil nonaktif.", a.label))
}

/// Nama pelanggan dan profil untuk memeriksa parameter Customer/Profile.
pub async fn fill_context(st: &Shared, a: &Action) -> FillContext {
    use panel_core::policy::command::ParamKind;
    let mut ctx = FillContext::default();
    if a.params.iter().any(|p| matches!(p.kind, ParamKind::Customer)) {
        ctx.customers = print(st, "/user-manager/user", &[], &["name"]).await.unwrap_or_default().into_iter().filter_map(|r| r.get("name").cloned()).collect();
    }
    if a.params.iter().any(|p| matches!(p.kind, ParamKind::Profile)) {
        ctx.profiles = profiles(st).await;
    }
    ctx
}

/// Profil User Manager saja (untuk ganti paket pelanggan).
pub async fn um_profiles(st: &Shared) -> R<Vec<String>> {
    Ok(print(st, "/user-manager/profile", &[], &["name"]).await?.into_iter().filter_map(|r| r.get("name").cloned()).collect())
}

pub async fn profiles(st: &Shared) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for path in ["/user-manager/profile", "/ppp/profile"] {
        out.extend(print(st, path, &[], &["name"]).await.unwrap_or_default().into_iter().filter_map(|r| r.get("name").cloned()));
    }
    out
}

/// Jalankan tombol (izin sudah dicek pemanggil). Mengembalikan (pesan, id eksekusi).
pub async fn run(st: &Shared, id: &str, values: &Value, actor: &str) -> R<(String, String)> {
    let a = load(st, id).await?;
    let values: BTreeMap<String, Input> = serde_json::from_value(values.clone()).map_err(|_| "Nilai parameter harus teks atau angka.".to_string())?;
    let ctx = fill_context(st, &a).await;
    let cmd = a.command.fill(&a.params, &values, &ctx).map_err(e)?;
    let rendered = cmd.render().unwrap_or_default();
    let outcome = run_command(st, &cmd).await;
    let run_id = random_token();
    let (msg, before, ok) = match outcome {
        Ok((m, b)) => (m, b, true),
        Err(m) => {
            let (text, partial) = Partial::split(&m);
            (text, partial.unwrap_or(Value::Null), false)
        }
    };
    // Gagal di tengah tapi sebagian objek berubah: "partial" tetap bisa dikembalikan.
    let status = match (ok, before.is_null()) {
        (true, _) => "done",
        (false, false) => "partial",
        (false, true) => "failed",
    };
    let (rid, aid, params, cmd_text, snap, result) = (run_id.clone(), a.id.clone(), serde_json::to_string(&values).unwrap_or_default(), rendered.clone(),
        (!before.is_null()).then(|| before.to_string()), status.to_string());
    let (label, actor, msg2) = (a.label.clone(), actor.to_string(), msg.clone());
    let _ = st
        .store
        .run(move |c| {
            c.execute(
                "INSERT INTO action_runs (id, action_id, params, command, before_snapshot, result, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (&rid, aid, params, &cmd_text, snap, &result, now()),
            )?;
            c.execute(
                "INSERT INTO audit (id, at, actor, account, action, before, after, status) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)",
                (format!("run-{rid}"), now(), actor, format!("action.{label}"), cmd_text.clone(), msg2, result),
            )
        })
        .await;
    if !ok {
        return Err(msg);
    }
    Ok((format!("{} ({rendered}) {msg}", a.label), run_id))
}

/// Kembalikan nilai lama dari satu eksekusi tombol.
pub async fn revert_run(st: &Shared, run_id: &str, actor: &str) -> R<String> {
    let Some(_busy) = crate::inflight::claim(format!("revert:{run_id}")) else {
        return Err("Pengembalian ini sedang diproses.".into());
    };
    let rid = run_id.to_string();
    let row = st
        .store
        .run(move |c| {
            c.query_row("SELECT before_snapshot, result FROM action_runs WHERE id = ?1", [rid], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))
                .optional()
        })
        .await
        .map_err(e)?
        .ok_or("Eksekusi tidak ditemukan.")?;
    if !matches!(row.1.as_deref(), Some("done" | "partial")) {
        return Err("Hanya eksekusi yang berhasil dan belum dikembalikan yang bisa dikembalikan.".into());
    }
    let data: Value = row.0.as_deref().and_then(|s| serde_json::from_str(s).ok()).ok_or("Eksekusi ini tidak menyimpan nilai lama (mis. remove), jadi tidak bisa dikembalikan.")?;
    let msg = apply_revert(st, &data).await?;
    let (rid, actor, m2) = (run_id.to_string(), actor.to_string(), msg.clone());
    let _ = st
        .store
        .run(move |c| {
            c.execute("UPDATE action_runs SET result = 'reverted' WHERE id = ?1", [&rid])?;
            c.execute(
                "INSERT INTO audit (id, at, actor, account, action, before, after, status) VALUES (?1, ?2, ?3, NULL, 'action.revert', NULL, ?4, 'done')",
                (format!("revert-{rid}"), now(), actor, m2),
            )
        })
        .await;
    Ok(msg)
}

/// Eksekusi tombol terakhir, untuk dialog tombol dan agen.
pub async fn runs(st: &Shared, action_id: Option<String>, limit: i64) -> R<Vec<Value>> {
    st.store
        .run(move |c| {
            let mut s = c.prepare(
                "SELECT r.id, r.action_id, a.label, r.command, r.result, r.at, r.before_snapshot IS NOT NULL FROM action_runs r
                 LEFT JOIN custom_actions a ON a.id = r.action_id WHERE ?1 IS NULL OR r.action_id = ?1 ORDER BY r.at DESC LIMIT ?2",
            )?;
            s.query_map((action_id, limit), |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "action_id": r.get::<_, Option<String>>(1)?, "label": r.get::<_, Option<String>>(2)?,
                           "command": r.get::<_, String>(3)?, "result": r.get::<_, Option<String>>(4)?, "at": r.get::<_, i64>(5)?,
                           "revertible": r.get::<_, bool>(6)? }))
            })?
            .collect()
        })
        .await
        .map_err(e)
}

// ---------- Endpoint untuk tombol, saran, dan topologi ----------

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/actions/{id}", get(action_detail))
        .route("/api/actions/{id}/run", post(action_run))
        .route("/api/action-runs/{id}/revert", post(action_revert))
        .route("/api/recommendations/{id}/accept", post(recommendation_accept))
        .route("/api/recommendations/{id}/dismiss", post(recommendation_dismiss))
        .route("/api/topology", get(topology))
}

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

async fn action_detail(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    use panel_core::policy::command::ParamKind;
    let a = match load(&st, &id).await {
        Ok(a) => a,
        Err(m) => return fail(StatusCode::NOT_FOUND, m),
    };
    let mut out = a.to_json();
    if a.params.iter().any(|p| matches!(p.kind, ParamKind::Customer)) {
        let names: Vec<String> = print(&st, "/user-manager/user", &[], &["name"]).await.unwrap_or_default().into_iter().filter_map(|r| r.get("name").cloned()).collect();
        out["customers"] = json!(names);
    }
    if a.params.iter().any(|p| matches!(p.kind, ParamKind::Profile)) {
        out["profiles"] = json!(profiles(&st).await);
    }
    out["confirm"] = json!(permission_of(&a.risk).confirmation_phrase(&a.label));
    out["runs"] = json!(runs(&st, Some(id), 5).await.unwrap_or_default());
    Json(out).into_response()
}

#[derive(serde::Deserialize)]
struct RunBody {
    #[serde(default)]
    values: Value,
    typed: Option<String>,
    #[serde(default)]
    confirmed: bool,
}

/// Tombol di halaman. Baca langsung jalan; ubah butuh konfirmasi tiap kali; kritis butuh ketik label.
async fn action_run(State(st): State<Shared>, Path(id): Path<String>, Json(b): Json<RunBody>) -> Response {
    let a = match load(&st, &id).await {
        Ok(a) => a,
        Err(m) => return fail(StatusCode::NOT_FOUND, m),
    };
    let perm = permission_of(&a.risk);
    if !perm.is_satisfied(b.confirmed, b.typed.as_deref(), &a.label) {
        let msg = match perm.confirmation_phrase(&a.label) {
            Some(p) => format!("Ketik \"{p}\" untuk menjalankan tombol kritis ini."),
            None => "Konfirmasi dulu sebelum menjalankan tombol ini.".into(),
        };
        return fail(StatusCode::FORBIDDEN, msg);
    }
    let values = if b.values.is_null() { json!({}) } else { b.values };
    let Some(_busy) = crate::inflight::claim(format!("action:{id}")) else {
        return fail(StatusCode::CONFLICT, "Tombol ini masih dijalankan. Tunggu hasilnya dulu.");
    };
    match run(&st, &id, &values, "admin").await {
        Ok((msg, run_id)) => Json(json!({ "ok": true, "message": msg, "run_id": run_id })).into_response(),
        Err(m) => fail(StatusCode::BAD_REQUEST, m),
    }
}

async fn action_revert(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match revert_run(&st, &id, "admin").await {
        Ok(msg) => Json(json!({ "ok": true, "message": msg })).into_response(),
        Err(m) => fail(StatusCode::CONFLICT, m),
    }
}

#[derive(serde::Deserialize, Default)]
struct AcceptBody {
    typed: Option<String>,
}

/// Terapkan saran: bila saran membawa perintah perbaikan, jalankan dengan izin sesuai risikonya.
async fn recommendation_accept(State(st): State<Shared>, Path(id): Path<String>, body: Option<Json<AcceptBody>>) -> Response {
    let Some(_busy) = crate::inflight::claim(format!("recommendation:{id}")) else {
        return fail(StatusCode::CONFLICT, "Saran ini sedang diterapkan.");
    };
    let id2 = id.clone();
    let row = st
        .store
        .run(move |c| c.query_row("SELECT evidence, status FROM recommendations WHERE id = ?1", [id2], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))).optional())
        .await;
    let Ok(Some((evidence, status))) = row else {
        return fail(StatusCode::NOT_FOUND, "Saran tidak ditemukan.");
    };
    if status != "open" {
        return fail(StatusCode::CONFLICT, "Saran ini sudah diproses.");
    }
    let ev: Value = evidence.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
    let mut message = "Saran ditandai diterapkan.".to_string();
    if let Some(raw) = ev["fix"].get("command") {
        let cmd: PolicyCommand = match serde_json::from_value(raw.clone()) {
            Ok(c) => c,
            Err(err) => return fail(StatusCode::BAD_REQUEST, format!("Perintah saran tidak valid: {err}")),
        };
        let title = ev["title"].as_str().unwrap_or("saran");
        let perm = cmd.permission();
        if !perm.is_satisfied(true, body.and_then(|b| b.0.typed).as_deref(), title) {
            return fail(StatusCode::FORBIDDEN, format!("Ketik \"{}\" untuk menjalankan perintah kritis ini.", perm.confirmation_phrase(title).unwrap_or_default()));
        }
        match run_command_text(&st, &cmd).await {
            Ok((m, _)) => message = format!("{} {m}", cmd.render().unwrap_or_default()),
            Err(m) => return fail(StatusCode::BAD_REQUEST, m),
        }
    }
    let (id2, m2) = (id.clone(), message.clone());
    let _ = st
        .store
        .run(move |c| {
            c.execute("UPDATE recommendations SET status = 'accepted' WHERE id = ?1", [&id2])?;
            c.execute(
                "INSERT INTO audit (id, at, actor, account, action, before, after, status) VALUES (?1, ?2, 'admin', NULL, 'recommendation.accept', NULL, ?3, 'done')",
                (format!("rec-{id2}"), now(), m2),
            )
        })
        .await;
    st.hub.publish("recommendations.updated", json!({ "id": id }));
    Json(json!({ "ok": true, "message": message })).into_response()
}

async fn recommendation_dismiss(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let id2 = id.clone();
    let _ = st.store.run(move |c| c.execute("UPDATE recommendations SET status = 'dismissed' WHERE id = ?1 AND status = 'open'", [id2])).await;
    st.hub.publish("recommendations.updated", json!({ "id": id }));
    Json(json!({ "ok": true })).into_response()
}

async fn topology(State(st): State<Shared>) -> Response {
    match panel_agent::router_tools::topology(&crate::agent_host::ApiBridge(&st)).await {
        Ok(v) => Json(v).into_response(),
        Err(m) => fail(StatusCode::BAD_GATEWAY, m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AppState;
    use crate::routeros::{RouterOs, RouterTarget, fake};
    use panel_core::auth::{LoginLimiter, Sessions};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    async fn state(tables: fake::Tables) -> Shared {
        let port = fake::start(tables).await;
        let config: panel_core::config::Config = toml::from_str(&format!(
            "secret_key = \"{}\"\nadmin_password_hash = \"$argon2id$x\"\n[router]\nhost = \"127.0.0.1\"\nport = {port}\nuser = \"u\"\npassword_encrypted = \"x\"\n",
            panel_core::crypto::Cipher::generate_key()
        ))
        .unwrap();
        Arc::new(AppState {
            sessions: Sessions::new(Duration::from_secs(60)),
            limiter: LoginLimiter::new(5, Duration::from_secs(60)),
            store: crate::store::Store::open_in_memory().unwrap(),
            hub: Default::default(),
            router: Arc::new(RouterOs::new(RouterTarget { host: "127.0.0.1".into(), port, user: "u".into(), password: "p".into(), tls_fingerprint: None })),
            quota: Default::default(),
            sync_lock: Default::default(),
            runs: Default::default(),
            config,
        })
    }

    #[tokio::test]
    async fn button_run_and_revert() {
        let mut t = std::collections::HashMap::new();
        t.insert("/queue/simple/print".to_string(), vec![row(&[(".id", "*1"), ("name", "budi"), ("max-limit", "5M/5M"), ("disabled", "false")])]);
        t.insert("/user-manager/user/print".to_string(), vec![row(&[(".id", "*9"), ("name", "budi")])]);
        let tables = Arc::new(Mutex::new(t));
        let st = state(tables.clone()).await;
        let input = json!({
            "id": "naik-kecepatan", "label": "Naikkan kecepatan",
            "command": { "path": ["queue", "simple"], "verb": "set", "find": { "name": { "param": "user" } }, "args": { "max-limit": { "param": "limit" } } },
            "params": [ { "name": "user", "label": "Pelanggan", "type": "customer" }, { "name": "limit", "label": "Limit", "type": "choice", "values": ["10M/10M"] } ]
        });
        create(&st, &input).await.unwrap();
        assert!(create(&st, &input).await.unwrap_err().contains("sudah dipakai"));
        let a = load(&st, "naik-kecepatan").await.unwrap();
        assert_eq!(a.risk, "write");
        assert!(run(&st, "naik-kecepatan", &json!({ "user": "andi", "limit": "10M/10M" }), "admin").await.unwrap_err().contains("tidak ditemukan"));
        let (_, run_id) = run(&st, "naik-kecepatan", &json!({ "user": "budi", "limit": "10M/10M" }), "admin").await.unwrap();
        let limit = || tables.lock().unwrap()["/queue/simple/print"][0]["max-limit"].clone();
        assert_eq!(limit(), "10M/10M");
        revert_run(&st, &run_id, "admin").await.unwrap();
        assert_eq!(limit(), "5M/5M");
        assert!(revert_run(&st, &run_id, "admin").await.is_err(), "tidak bisa dikembalikan dua kali");
        delete(&st, "naik-kecepatan").await.unwrap();
        assert!(load(&st, "naik-kecepatan").await.is_err());
    }

    #[tokio::test]
    async fn radius_attributes_merge() {
        let mut t = std::collections::HashMap::new();
        t.insert("/user-manager/user/print".to_string(), vec![row(&[(".id", "*9"), ("name", "budi"), ("attributes", "Mikrotik-Rate-Limit:5M/5M,Framed-Pool:p1")])]);
        let tables = Arc::new(Mutex::new(t));
        let st = state(tables.clone()).await;
        let attrs = json!({ "Mikrotik-Rate-Limit": "10M/10M", "Framed-Pool": "", "shared-users": 2 });
        set_radius(&st, "budi", attrs.as_object().unwrap()).await.unwrap();
        let u = tables.lock().unwrap()["/user-manager/user/print"][0].clone();
        assert_eq!(u["attributes"], "Mikrotik-Rate-Limit:10M/10M");
        assert_eq!(u["shared-users"], "2");
        assert!(set_radius(&st, "budi", json!({ "bad key!": "x" }).as_object().unwrap()).await.is_err());
    }
}
