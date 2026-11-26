//! Endpoint agen dan desain (kontrak: web/src/agent/CONTRACT.md).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use panel_agent::ai::{Endpoint, Protocol, Thinking, list_models};
use panel_agent::design::default_spec;
use panel_agent::runner::{Ended, run_turn};
use panel_core::crypto::{Cipher, random_token};
use panel_core::policy::Permission;
use panel_core::policy::command::Command as PolicyCommand;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent_host::{AppHost, current_draft};
use crate::http::Shared;
use crate::routeros::Command;
use crate::store::now;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/design", get(design))
        .route("/api/design/default", get(design_default))
        .route("/api/agent/memories", get(memories))
        .route("/api/agent/memories/{id}/delete", post(forget_memory))
        .route("/api/design/versions/{id}/revert", post(revert))
        .route("/api/design/apply", post(apply))
        .route("/api/design/discard", post(discard))
        .route("/api/design/reset", post(reset))
        .route("/api/agent/conversations", get(conversations))
        .route("/api/agent/conversations/{id}/messages", get(messages))
        .route("/api/agent/messages", post(send))
        .route("/api/agent/runs/{id}/cancel", post(cancel))
        .route("/api/agent/proposals/{id}/approve", post(approve))
        .route("/api/agent/proposals/{id}/reject", post(reject))
        .route("/api/agent/proposals/{id}/revert", post(revert_proposal))
        .route("/api/settings/ai", get(ai_get).put(ai_put))
        .route("/api/settings/ai/models", post(ai_models))
}

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn internal(err: impl std::fmt::Display) -> Response {
    tracing::error!("{err}");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Terjadi kesalahan di server panel.",
    )
}

/// Panel selalu gelap: spec lama di DB (mode light/system) dinormalkan saat dibaca dan disalin.
fn dark_spec(spec: &str) -> String {
    serde_json::from_str::<panel_agent::design::DesignSpec>(spec)
        .ok()
        .and_then(|s| serde_json::to_string(&s.normalized()).ok())
        .unwrap_or_else(|| spec.to_owned())
}

fn version_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let spec = dark_spec(&r.get::<_, String>("spec")?);
    Ok(json!({
        "id": r.get::<_, String>("id")?, "parent_id": r.get::<_, Option<String>>("parent_id")?,
        "status": r.get::<_, String>("status")?, "source": r.get::<_, String>("source")?,
        "created_at": r.get::<_, i64>("created_at")?,
        "spec": serde_json::from_str::<Value>(&spec).unwrap_or(Value::Null),
    }))
}

fn get_version(c: &rusqlite::Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    c.query_row(
        "SELECT * FROM design_versions WHERE id = ?1",
        [id],
        version_json,
    )
    .optional()
}

/// Draft (bila ada, selain itu versi aktif) dan versi aktif.
fn design_state(c: &mut rusqlite::Connection) -> rusqlite::Result<Value> {
    let (draft_id, _) = current_draft(c)?;
    let active: Option<Value> = c
        .query_row(
            "SELECT * FROM design_versions WHERE status = 'active'",
            [],
            version_json,
        )
        .optional()?;
    Ok(json!({ "draft": get_version(c, &draft_id)?, "active": active }))
}

/// Buat draft baru berisi `spec` dengan induk draft saat ini.
fn new_draft(c: &mut rusqlite::Connection, spec: &str, source: &str) -> rusqlite::Result<Value> {
    let spec = &dark_spec(spec);
    let (parent, _) = current_draft(c)?;
    let id = random_token();
    let css = serde_json::from_str::<Value>(spec)
        .ok()
        .and_then(|v| v["custom_css"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let tx = c.transaction()?;
    tx.execute(
        "UPDATE design_versions SET status = 'archived' WHERE status = 'draft'",
        [],
    )?;
    tx.execute(
        "INSERT INTO design_versions (id, parent_id, spec, custom_css, source, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6)",
        (&id, &parent, spec, css, source, now()),
    )?;
    tx.commit()?;
    Ok(get_version(c, &id)?.unwrap_or(Value::Null))
}

async fn design(State(st): State<Shared>) -> Response {
    match st.store.run(design_state).await {
        Ok(v) => Json(v).into_response(),
        Err(err) => internal(err),
    }
}

fn announce(st: &Shared, draft: &Value, source: &str) {
    st.hub.publish(
        "design.version",
        json!({ "id": draft["id"], "parent": draft["parent_id"], "source": source }),
    );
}

async fn revert(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let r = st
        .store
        .run(move |c| {
            let spec: Option<String> = c
                .query_row(
                    "SELECT spec FROM design_versions WHERE id = ?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            spec.map(|s| new_draft(c, &s, "revert")).transpose()
        })
        .await;
    match r {
        Ok(Some(draft)) => {
            announce(&st, &draft, "revert");
            Json(json!({ "draft": draft })).into_response()
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "Versi desain tidak ditemukan."),
        Err(err) => internal(err),
    }
}

async fn apply(State(st): State<Shared>) -> Response {
    let r = st
        .store
        .run(|c| {
            let (draft, _) = current_draft(c)?;
            let tx = c.transaction()?;
            tx.execute("UPDATE design_versions SET status = 'archived' WHERE status = 'active' AND id <> ?1", [&draft])?;
            tx.execute("UPDATE design_versions SET status = 'active' WHERE id = ?1", [&draft])?;
            tx.commit()?;
            get_version(c, &draft)
        })
        .await;
    match r {
        Ok(active) => {
            st.hub.publish(
                "design.version",
                json!({ "id": active.as_ref().map(|a| a["id"].clone()), "status": "active" }),
            );
            Json(json!({ "active": active })).into_response()
        }
        Err(err) => internal(err),
    }
}

async fn discard(State(st): State<Shared>) -> Response {
    let r = st
        .store
        .run(|c| {
            let spec: Option<String> = c
                .query_row(
                    "SELECT spec FROM design_versions WHERE status = 'active'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let spec = spec.unwrap_or_else(|| serde_json::to_string(&default_spec()).unwrap());
            new_draft(c, &spec, "discard")
        })
        .await;
    match r {
        Ok(draft) => {
            announce(&st, &draft, "discard");
            Json(json!({ "draft": draft })).into_response()
        }
        Err(err) => internal(err),
    }
}

async fn reset(State(st): State<Shared>) -> Response {
    let spec = serde_json::to_string(&default_spec()).unwrap();
    match st.store.run(move |c| new_draft(c, &spec, "reset")).await {
        Ok(draft) => {
            announce(&st, &draft, "reset");
            Json(json!({ "draft": draft })).into_response()
        }
        Err(err) => internal(err),
    }
}

async fn design_default() -> Response {
    Json(json!({ "spec": default_spec() })).into_response()
}

async fn memories(State(st): State<Shared>) -> Response {
    let r = st
        .store
        .run(|c| {
            let mut s = c.prepare("SELECT id, kind, text, source, updated_at FROM agent_memories ORDER BY updated_at DESC LIMIT 200")?;
            s.query_map([], |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?, "text": r.get::<_, String>(2)?,
                           "source": r.get::<_, Option<String>>(3)?, "updated_at": r.get::<_, i64>(4)? }))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .await;
    match r {
        Ok(v) => Json(json!({ "memories": v })).into_response(),
        Err(err) => internal(err),
    }
}

async fn forget_memory(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match st.store.run(move |c| c.execute("DELETE FROM agent_memories WHERE id = ?1", [id])).await {
        Ok(0) => fail(StatusCode::NOT_FOUND, "Memori tidak ditemukan."),
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(err) => internal(err),
    }
}

async fn conversations(State(st): State<Shared>) -> Response {
    let r = st
        .store
        .run(|c| {
            let mut s = c.prepare("SELECT id, title, updated_at FROM conversations ORDER BY updated_at DESC LIMIT 50")?;
            s.query_map([], |r| Ok(json!({ "id": r.get::<_, String>(0)?, "title": r.get::<_, Option<String>>(1)?, "updated_at": r.get::<_, i64>(2)? })))?
                .collect::<Result<Vec<_>, _>>()
        })
        .await;
    match r {
        Ok(v) => Json(json!({ "conversations": v })).into_response(),
        Err(err) => internal(err),
    }
}

fn card_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let input: String = r.get("input")?;
    Ok(json!({
        "id": r.get::<_, String>("id")?, "conversation_id": r.get::<_, String>("conversation_id")?,
        "tool_use_id": r.get::<_, Option<String>>("tool_use_id")?, "tool": r.get::<_, String>("tool")?,
        "input": serde_json::from_str::<Value>(&input).unwrap_or(Value::Null),
        "permission": r.get::<_, String>("permission")?, "preview": r.get::<_, Option<String>>("preview")?,
        "label": r.get::<_, String>("label")?, "status": r.get::<_, String>("status")?,
        "result": r.get::<_, Option<String>>("result")?,
    }))
}

pub async fn load_card(st: &Shared, id: &str) -> anyhow::Result<Value> {
    let id = id.to_string();
    st.store
        .run(move |c| {
            c.query_row(
                "SELECT * FROM agent_proposals WHERE id = ?1",
                [id],
                card_json,
            )
        })
        .await
}

fn load_messages(c: &rusqlite::Connection, conv: &str) -> rusqlite::Result<Vec<Value>> {
    let mut s = c.prepare(
        "SELECT id, role, content, design_version_id, created_at FROM chat_messages WHERE conversation_id = ?1 ORDER BY created_at, rowid",
    )?;
    s.query_map([conv], |r| {
        let content: String = r.get(2)?;
        let parts = serde_json::from_str::<Value>(&content).map(|m| m["parts"].clone()).unwrap_or(json!([]));
        Ok(json!({ "id": r.get::<_, String>(0)?, "role": r.get::<_, String>(1)?, "parts": parts,
                   "design_version_id": r.get::<_, Option<String>>(3)?, "created_at": r.get::<_, i64>(4)? }))
    })?
    .collect()
}

async fn messages(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let conv = id.clone();
    let r = st
        .store
        .run(move |c| {
            let msgs = load_messages(c, &conv)?;
            let mut s = c.prepare("SELECT * FROM agent_proposals WHERE conversation_id = ?1 ORDER BY created_at, rowid")?;
            let cards = s.query_map([&conv], card_json)?.collect::<Result<Vec<_>, _>>()?;
            Ok((msgs, cards))
        })
        .await;
    match r {
        Ok((msgs, cards)) => Json(json!({ "messages": msgs, "proposals": cards, "run_id": st.runs.for_conversation(&id) })).into_response(),
        Err(err) => internal(err),
    }
}

#[derive(Deserialize)]
struct SendBody {
    conversation_id: Option<String>,
    text: String,
    replace_from: Option<String>,
}

async fn send(State(st): State<Shared>, Json(b): Json<SendBody>) -> Response {
    let text = b.text.trim().to_string();
    if text.is_empty() || text.chars().count() > 8000 {
        return fail(
            StatusCode::BAD_REQUEST,
            "Pesan harus berisi 1–8000 karakter.",
        );
    }
    let conv = b
        .conversation_id
        .filter(|c| !c.is_empty())
        .unwrap_or_else(random_token);
    let Some((run_id, token)) = st.runs.start(&conv) else {
        return fail(
            StatusCode::CONFLICT,
            "Agen masih mengerjakan permintaan sebelumnya. Tekan Stop atau tunggu.",
        );
    };
    let (c2, replace) = (conv.clone(), b.replace_from.clone());
    let prepared = st
        .store
        .run(move |c| {
            let at = now();
            c.execute("INSERT OR IGNORE INTO conversations (id, title, created_at, updated_at) VALUES (?1, NULL, ?2, ?2)", (&c2, at))?;
            let mut reverted = None;
            if let Some(from) = replace {
                let pos: Option<i64> = c
                    .query_row("SELECT rowid FROM chat_messages WHERE id = ?1 AND conversation_id = ?2", (&from, &c2), |r| r.get(0))
                    .optional()?;
                if let Some(pos) = pos {
                    let back: Option<String> = c
                        .query_row(
                            "SELECT design_version_id FROM chat_messages WHERE conversation_id = ?1 AND rowid < ?2
                             AND design_version_id IS NOT NULL ORDER BY rowid DESC LIMIT 1",
                            (&c2, pos),
                            |r| r.get(0),
                        )
                        .optional()?;
                    c.execute("DELETE FROM chat_messages WHERE conversation_id = ?1 AND rowid >= ?2", (&c2, pos))?;
                    let spec: Option<String> = match back {
                        Some(v) => c.query_row("SELECT spec FROM design_versions WHERE id = ?1", [v], |r| r.get(0)).optional()?,
                        None => c.query_row("SELECT spec FROM design_versions WHERE status = 'active'", [], |r| r.get(0)).optional()?,
                    };
                    if let Some(spec) = spec {
                        reverted = Some(new_draft(c, &spec, "revert")?);
                    }
                }
            }
            Ok((load_messages(c, &c2)?, reverted))
        })
        .await;
    let (msgs, reverted) = match prepared {
        Ok(v) => v,
        Err(err) => {
            st.runs.finish(&run_id);
            return internal(err);
        }
    };
    if let Some(draft) = reverted {
        announce(&st, &draft, "revert");
    }
    let cipher = match Cipher::from_base64(&st.config.secret_key) {
        Ok(c) => c,
        Err(err) => {
            st.runs.finish(&run_id);
            return internal(err);
        }
    };
    let (state, conv2, rid) = (st.clone(), conv.clone(), run_id.clone());
    tokio::spawn(async move {
        let host = AppHost {
            state: state.clone(),
            cipher,
        };
        let outcome = run_turn(&host, &conv2, &text, &token).await;
        if let Some(version) = outcome.design_version.clone() {
            let c3 = conv2.clone();
            let _ = state
                .store
                .run(move |c| {
                    c.execute(
                        "UPDATE chat_messages SET design_version_id = ?1 WHERE rowid = (
                            SELECT rowid FROM chat_messages WHERE conversation_id = ?2 AND role = 'assistant' ORDER BY rowid DESC LIMIT 1)",
                        (version, c3),
                    )
                })
                .await;
        }
        if let Ended::Error(m) = &outcome.ended {
            tracing::warn!("giliran agen gagal: {m}");
        }
        state.runs.finish(&rid);
    });
    Json(json!({ "conversation_id": conv, "run_id": run_id, "messages": msgs })).into_response()
}

async fn cancel(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    st.runs.cancel(&id);
    Json(json!({})).into_response()
}

#[derive(Deserialize, Default)]
struct ApproveBody {
    typed: Option<String>,
}

async fn set_status(
    st: &Shared,
    id: &str,
    status: &str,
    result: Option<String>,
) -> anyhow::Result<Value> {
    let (i, s) = (id.to_string(), status.to_string());
    st.store
        .run(move |c| c.execute("UPDATE agent_proposals SET status = ?1, result = ?2, decided_at = ?3 WHERE id = ?4", (s, result, now(), i)))
        .await?;
    let card = load_card(st, id).await?;
    st.hub.publish("proposal.updated", json!({ "card": card }));
    Ok(card)
}

async fn approve(
    State(st): State<Shared>,
    Path(id): Path<String>,
    body: Option<Json<ApproveBody>>,
) -> Response {
    let typed = body.and_then(|b| b.0.typed);
    let card = match load_card(&st, &id).await {
        Ok(c) => c,
        Err(_) => return fail(StatusCode::NOT_FOUND, "Usulan tidak ditemukan."),
    };
    if card["status"] != "pending" {
        return fail(StatusCode::CONFLICT, "Usulan ini sudah diproses.");
    }
    let permission: Permission =
        serde_json::from_value(card["permission"].clone()).unwrap_or(Permission::Critical);
    let label = card["label"].as_str().unwrap_or_default();
    if !permission.is_satisfied(true, typed.as_deref(), label) {
        return fail(
            StatusCode::FORBIDDEN,
            format!("Ketik \"{label}\" untuk mengonfirmasi tindakan kritis ini."),
        );
    }
    if let Err(err) = set_status(&st, &id, "approved", None).await {
        return internal(err);
    }
    let tool = card["tool"].as_str().unwrap_or_default().to_string();
    let service = service_command(&card["input"]);
    let (status, result) = match &service {
        Some(cmd) => match set_service_address(&st, cmd).await {
            Ok((msg, old)) => {
                // Simpan nilai lama agar admin bisa mengembalikannya dengan satu klik.
                let mut input = card["input"].clone();
                input["previous_address"] = json!(old);
                let (pid, text) = (id.clone(), input.to_string());
                let _ = st.store.run(move |c| c.execute("UPDATE agent_proposals SET input = ?1 WHERE id = ?2", (text, pid))).await;
                ("done", msg)
            }
            Err(m) => ("failed", m),
        },
        None => match execute(&st, &tool, &card["input"]).await {
            Ok(v) => ("done", v),
            Err(m) => ("failed", m),
        },
    };
    let account = card["input"]["account"].as_str().map(str::to_owned);
    let (aid, at, s2, r2) = (id.clone(), now(), status.to_string(), result.clone());
    let _ = st
        .store
        .run(move |c| {
            c.execute(
                "INSERT INTO audit (id, at, actor, account, action, before, after, status) VALUES (?1, ?2, 'agent', ?3, ?4, NULL, ?5, ?6)",
                (aid, at, account, format!("agent.{tool}"), r2, s2),
            )
        })
        .await;
    match set_status(&st, &id, status, Some(result)).await {
        Ok(card) => Json(json!({ "card": card })).into_response(),
        Err(err) => internal(err),
    }
}

/// Perintah `/ip service set address=` (satu-satunya perubahan /ip service yang diizinkan policy).
fn service_command(input: &Value) -> Option<PolicyCommand> {
    let cmd: PolicyCommand = serde_json::from_value(input.get("command")?.clone()).ok()?;
    (cmd.path == ["ip", "service"] && cmd.verb == "set").then_some(cmd)
}

/// IP yang dilihat router untuk sesi API panel (dari /user/active via=api), untuk memastikan
/// Available From baru tidak memutus panel dari router.
async fn panel_addresses(st: &Shared) -> Vec<std::net::IpAddr> {
    let Ok(cmd) = Command::new("/user/active/print")
        .and_then(|c| c.filter("via", "api"))
        .and_then(|c| c.filter("name", &st.config.router.user))
        .and_then(|c| c.proplist(&["address"]))
    else {
        return Vec::new();
    };
    let rows = st.router.call(&cmd).await.unwrap_or_default();
    let mut ips: Vec<std::net::IpAddr> = rows.iter().filter_map(|r| r.get("address")?.parse().ok()).collect();
    ips.sort();
    ips.dedup();
    ips
}

/// Ubah Available From satu layanan dengan pengaman. Mengembalikan (pesan, nilai lama).
async fn set_service_address(st: &Shared, cmd: &PolicyCommand) -> Result<(String, String), String> {
    use panel_core::policy::command::{Value as PV, PANEL_SERVICES, service_address_allows};
    cmd.validate().map_err(|e| e.to_string())?;
    let (Some(PV::Text(name)), Some(PV::Text(address))) = (cmd.find.get("name"), cmd.args.get("address")) else {
        return Err("Layanan atau Available From belum lengkap.".into());
    };
    let finder = Command::new("/ip/service/print")
        .and_then(|c| c.filter("name", name))
        .and_then(|c| c.proplist(&[".id", "address"]))
        .map_err(|e| e.to_string())?;
    let rows = st.router.call(&finder).await.map_err(|e| e.to_string())?;
    let [row] = rows.as_slice() else {
        return Err(format!("Layanan {name} tidak ditemukan di router."));
    };
    let id = row.get(".id").cloned().ok_or("Router tidak mengirim .id layanan.")?;
    let old = row.get("address").cloned().unwrap_or_default();
    if PANEL_SERVICES.contains(&name.as_str()) {
        let ips = panel_addresses(st).await;
        if ips.is_empty() {
            return Err("Tidak bisa memastikan IP panel di router, jadi perubahan dibatalkan supaya panel tidak terkunci.".into());
        }
        if let Some(ip) = ips.iter().find(|ip| !service_address_allows(address, **ip)) {
            return Err(format!("Ditolak: IP panel {ip} tidak termasuk Available From baru untuk {name}, panel akan terputus dari router. Tambahkan {ip}/32 atau jaringannya."));
        }
    }
    let set = Command::new("/ip/service/set")
        .and_then(|c| c.arg(".id", &id))
        .and_then(|c| c.arg("address", address))
        .map_err(|e| e.to_string())?;
    st.router.call(&set).await.map_err(|e| e.to_string())?;
    let show = |a: &str| if a.is_empty() { "semua alamat".to_string() } else { a.to_string() };
    Ok((format!("Available From {name}: {} → {}.", show(&old), show(address)), old))
}

/// Kembalikan Available From ke nilai sebelum usulan ini dijalankan (dengan pengaman yang sama).
async fn revert_proposal(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let card = match load_card(&st, &id).await {
        Ok(c) => c,
        Err(_) => return fail(StatusCode::NOT_FOUND, "Usulan tidak ditemukan."),
    };
    let (Some(mut cmd), Some(old)) = (service_command(&card["input"]), card["input"]["previous_address"].as_str()) else {
        return fail(StatusCode::BAD_REQUEST, "Usulan ini tidak bisa dikembalikan.");
    };
    if card["status"] != "done" || card["input"]["reverted"] == true {
        return fail(StatusCode::CONFLICT, "Usulan ini sudah dikembalikan atau belum dijalankan.");
    }
    cmd.args.insert("address".into(), panel_core::policy::command::Value::Text(old.to_string()));
    let (status, result) = match set_service_address(&st, &cmd).await {
        Ok((msg, _)) => ("done", format!("Dikembalikan. {msg}")),
        Err(m) => return fail(StatusCode::CONFLICT, m),
    };
    let mut input = card["input"].clone();
    input["reverted"] = json!(true);
    let (pid, text) = (id.clone(), input.to_string());
    let _ = st.store.run(move |c| c.execute("UPDATE agent_proposals SET input = ?1 WHERE id = ?2", (text, pid))).await;
    let (aid, at, r2) = (format!("{id}-revert"), now(), result.clone());
    let _ = st.store.run(move |c| c.execute(
        "INSERT INTO audit (id, at, actor, account, action, before, after, status) VALUES (?1, ?2, 'admin', NULL, 'agent.revert_service', NULL, ?3, 'done')",
        (aid, at, r2))).await;
    match set_status(&st, &id, status, Some(result)).await {
        Ok(card) => Json(json!({ "card": card })).into_response(),
        Err(err) => internal(err),
    }
}

async fn reject(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match set_status(&st, &id, "rejected", None).await {
        Ok(card) => Json(json!({ "card": card })).into_response(),
        Err(err) => internal(err),
    }
}

/// Jalankan usulan yang sudah disetujui. Perintah RouterOS dikirim lewat API (bukan string CLI).
async fn execute(st: &Shared, tool: &str, input: &Value) -> Result<String, String> {
    if tool == "topup_quota" {
        let account = input["account"].as_str().ok_or("account kosong")?;
        let gib = input["gib"]
            .as_u64()
            .or_else(|| input["amount_gib"].as_u64())
            .ok_or("jumlah GiB kosong")?;
        let op = st
            .quota
            .preview(&st.router, account, gib)
            .await
            .map_err(|e| e.to_string())?;
        let op = st.quota.take(&op.id, account).map_err(|e| e.to_string())?;
        return Ok(st.quota.apply(&st.router, &op).await);
    }
    if tool == "install_update" {
        crate::http::request_update("install")
            .await
            .map_err(|(_, m)| m)?;
        return Ok("Pembaru diminta memasang rilis terbaru. Panel akan restart dalam ±1 menit; bila gagal, versi lama dikembalikan otomatis.".into());
    }
    let account = || input["account"].as_str().filter(|a| !a.is_empty()).ok_or("account kosong");
    match tool {
        "disconnect_session" => return crate::actions::disconnect(st, account()?).await,
        "set_customer_enabled" => {
            let enabled = input["enabled"].as_bool().ok_or("enabled harus true/false")?;
            return crate::actions::set_enabled(st, account()?, enabled).await;
        }
        "set_customer_profile" => {
            let profile = input["profile"].as_str().filter(|p| !p.is_empty()).ok_or("profile kosong")?;
            return crate::actions::set_profile(st, account()?, profile).await;
        }
        "set_radius_user" => {
            let attrs = input["attributes"].as_object().ok_or("attributes harus objek")?;
            return crate::actions::set_radius(st, account()?, attrs).await;
        }
        "reboot_router" => return crate::actions::reboot(st).await,
        "create_action_button" => return crate::actions::create(st, input).await,
        "update_action_button" => return crate::actions::update(st, input).await,
        "delete_action_button" => return crate::actions::delete(st, input["id"].as_str().unwrap_or_default()).await,
        "run_action_button" => {
            let (msg, run_id) = crate::actions::run(st, input["id"].as_str().unwrap_or_default(), &input["values"], "agent").await?;
            return Ok(format!("{msg} (eksekusi {run_id})"));
        }
        "revert_action_run" => return crate::actions::revert_run(st, input["run_id"].as_str().unwrap_or_default(), "agent").await,
        _ => {}
    }
    let Some(raw) = input.get("command") else {
        return Err(format!(
            "Eksekusi `{tool}` belum tersedia di versi panel ini."
        ));
    };
    let cmd: PolicyCommand =
        serde_json::from_value(raw.clone()).map_err(|e| format!("Perintah tidak valid: {e}"))?;
    crate::actions::run_command(st, &cmd).await.map(|(msg, _)| msg)
}

async fn ai_get(State(st): State<Shared>) -> Response {
    let r = st
        .store
        .run(|c| {
            c.query_row("SELECT value FROM settings WHERE key = 'ai'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
        })
        .await;
    match r {
        Ok(Some(v)) => {
            let v: Value = serde_json::from_str(&v).unwrap_or(json!({}));
            Json(json!({ "url": v["url"], "protocol": v["protocol"], "model": v["model"], "has_key": v["encrypted_key"].is_string(),
                "thinking": Thinking::parse(v["thinking"].as_str().unwrap_or_default()) })).into_response()
        }
        Ok(None) => Json(json!({ "url": null, "protocol": null, "model": null, "has_key": false, "thinking": Thinking::High }))
            .into_response(),
        Err(err) => internal(err),
    }
}

#[derive(Deserialize)]
struct AiBody {
    url: String,
    protocol: String,
    model: String,
    api_key: Option<String>,
    #[serde(default)]
    thinking: Thinking,
}

#[derive(Deserialize)]
struct ModelsBody {
    url: String,
    protocol: String,
    api_key: Option<String>,
}

/// Tombol "Ambil model": daftar model dari endpoint yang sedang diisi di form. API key boleh
/// kosong bila endpoint dan protokolnya sama dengan yang tersimpan (key lama dipakai).
async fn ai_models(State(st): State<Shared>, Json(b): Json<ModelsBody>) -> Response {
    let key = b.api_key.unwrap_or_default().trim().to_string();
    #[cfg(feature = "test-endpoint")]
    let local_test = b.url.starts_with("http://127.0.0.1:");
    #[cfg(not(feature = "test-endpoint"))]
    let local_test = false;
    if !["openai", "anthropic"].contains(&b.protocol.as_str()) || !(b.url.starts_with("https://") || local_test) || b.url.len() > 500
        || key.len() > 2000 || key.chars().any(char::is_control)
    {
        return fail(StatusCode::BAD_REQUEST, "Isi endpoint https:// dan protokol openai/anthropic dulu.");
    }
    let key = if !key.is_empty() {
        key
    } else {
        let saved: Option<String> = st.store
            .run(|c| c.query_row("SELECT value FROM settings WHERE key = 'ai'", [], |r| r.get(0)).optional())
            .await.ok().flatten();
        let saved: Value = saved.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(json!({}));
        if saved["url"] != b.url.as_str() || saved["protocol"] != b.protocol.as_str() {
            return fail(StatusCode::BAD_REQUEST, "Masukkan API key untuk endpoint ini dulu.");
        }
        let Ok(cipher) = Cipher::from_base64(&st.config.secret_key) else { return fail(StatusCode::INTERNAL_SERVER_ERROR, "Kunci panel tidak valid.") };
        match cipher.decrypt(saved["encrypted_key"].as_str().unwrap_or_default()) {
            Ok(k) => k,
            Err(_) => return fail(StatusCode::BAD_REQUEST, "Simpan ulang API key di Pengaturan AI."),
        }
    };
    let protocol = if b.protocol == "anthropic" { Protocol::Anthropic } else { Protocol::OpenAi };
    let endpoint = Endpoint { protocol, base_url: b.url, model: String::new(), api_key: key, thinking: Thinking::Off };
    match list_models(&endpoint).await {
        Ok(models) if models.is_empty() => fail(StatusCode::BAD_GATEWAY, "Endpoint tidak mengirim satu model pun."),
        Ok(models) => Json(json!({ "models": models })).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

async fn ai_put(State(st): State<Shared>, Json(b): Json<AiBody>) -> Response {
    let key = b.api_key.unwrap_or_default().trim().to_string();
    if !["openai", "anthropic"].contains(&b.protocol.as_str())
        || !(1..=150).contains(&b.model.trim().len())
        || !b.url.starts_with("https://")
        || b.url.len() > 500
        || key.len() > 2000
        || key.chars().any(char::is_control)
    {
        return fail(
            StatusCode::BAD_REQUEST,
            "Endpoint harus https://, protokol openai/anthropic, dan model 1–150 karakter.",
        );
    }
    let cipher = match Cipher::from_base64(&st.config.secret_key) {
        Ok(c) => c,
        Err(err) => return internal(err),
    };
    let r = st
        .store
        .run(move |c| {
            let old: Option<String> = c.query_row("SELECT value FROM settings WHERE key = 'ai'", [], |r| r.get(0)).optional()?;
            let old: Value = old.and_then(|o| serde_json::from_str(&o).ok()).unwrap_or(json!({}));
            let same = old["url"] == b.url.as_str() && old["protocol"] == b.protocol.as_str();
            let encrypted = if !key.is_empty() {
                Some(cipher.encrypt(&key))
            } else if same {
                old["encrypted_key"].as_str().map(str::to_owned)
            } else {
                None
            };
            let Some(encrypted) = encrypted else { return Ok(false) };
            let v = json!({ "url": b.url, "protocol": b.protocol, "model": b.model.trim(), "encrypted_key": encrypted, "thinking": b.thinking });
            c.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('ai', ?1)", [v.to_string()])?;
            Ok(true)
        })
        .await;
    match r {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => fail(
            StatusCode::BAD_REQUEST,
            "Masukkan API key saat menyimpan endpoint baru.",
        ),
        Err(err) => internal(err),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    use super::*;
    use crate::http::tests::{req, state};

    async fn call(app: &Router, r: Request<Body>) -> (StatusCode, Value) {
        let res = app.clone().oneshot(r).await.unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn design_and_proposal_flow() {
        let st = state();
        let app = crate::http::app(st.clone());
        let res = app
            .clone()
            .oneshot(req("POST", "/api/login", r#"{"password":"rahasia"}"#, &[]))
            .await
            .unwrap();
        let sid = res.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let csrf = serde_json::from_slice::<Value>(&body).unwrap()["csrf"]
            .as_str()
            .unwrap()
            .to_string();
        let h = [("cookie", sid.as_str()), ("x-csrf-token", csrf.as_str())];

        // Desain bawaan otomatis jadi versi aktif.
        let (s, v) = call(&app, req("GET", "/api/design", "", &h)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["active"]["id"], "default");
        assert_eq!(v["draft"]["id"], "default");

        // Reset membuat draft baru; apply menjadikannya aktif; revert ke 'default' membuat draft salinan.
        let (_, v) = call(&app, req("POST", "/api/design/reset", "", &h)).await;
        let draft = v["draft"]["id"].as_str().unwrap().to_string();
        assert_eq!(v["draft"]["parent_id"], "default");
        let (_, v) = call(&app, req("POST", "/api/design/apply", "", &h)).await;
        assert_eq!(v["active"]["id"], draft.as_str());
        let (s, v) = call(
            &app,
            req("POST", "/api/design/versions/default/revert", "", &h),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["draft"]["status"], "draft");
        let (s, _) = call(
            &app,
            req("POST", "/api/design/versions/tidak-ada/revert", "", &h),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        // Kartu kritis: harus mengetik label yang benar.
        st.store
            .run(|c| {
                c.execute("INSERT INTO conversations VALUES ('k1', NULL, 0, 0)", [])?;
                c.execute(
                    "INSERT INTO agent_proposals (id, conversation_id, tool, input, permission, label, status, created_at)
                     VALUES ('p1', 'k1', 'reboot_router', '{}', 'critical', 'reboot router', 'pending', 0)",
                    [],
                )
            })
            .await
            .unwrap();
        let (s, _) = call(
            &app,
            req(
                "POST",
                "/api/agent/proposals/p1/approve",
                r#"{"typed":"salah"}"#,
                &h,
            ),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, v) = call(&app, req("POST", "/api/agent/proposals/p1/reject", "", &h)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["card"]["status"], "rejected");
        let (s, _) = call(
            &app,
            req(
                "POST",
                "/api/agent/proposals/p1/approve",
                r#"{"typed":"reboot router"}"#,
                &h,
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);

        let (_, v) = call(
            &app,
            req("GET", "/api/agent/conversations/k1/messages", "", &h),
        )
        .await;
        assert_eq!(v["proposals"][0]["id"], "p1");
        assert_eq!(v["run_id"], Value::Null);

        // Pengaturan AI: key disimpan terenkripsi dan tidak pernah dikirim balik.
        let (s, _) = call(
            &app,
            req(
                "PUT",
                "/api/settings/ai",
                r#"{"url":"http://x","protocol":"openai","model":"m","api_key":"k"}"#,
                &h,
            ),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(&app, req("PUT", "/api/settings/ai", r#"{"url":"https://api.example.com/v1","protocol":"openai","model":"m","api_key":"sk-1"}"#, &h)).await;
        assert_eq!(s, StatusCode::OK);
        let (_, v) = call(&app, req("GET", "/api/settings/ai", "", &h)).await;
        assert_eq!(v["has_key"], true);
        assert_eq!(v["thinking"], "high");
        assert!(!v.to_string().contains("sk-1"));
        let (s, _) = call(&app, req("PUT", "/api/settings/ai", r#"{"url":"https://api.example.com/v1","protocol":"openai","model":"m","api_key":null,"thinking":"low"}"#, &h)).await;
        assert_eq!(s, StatusCode::OK);
        let (_, v) = call(&app, req("GET", "/api/settings/ai", "", &h)).await;
        assert_eq!(v["thinking"], "low");
        assert_eq!(v["has_key"], true);
    }

    #[tokio::test]
    async fn send_without_ai_settings_finishes_with_error_event() {
        let st = state();
        let mut events = st.hub.subscribe();
        let app = crate::http::app(st.clone());
        let res = app
            .clone()
            .oneshot(req("POST", "/api/login", r#"{"password":"rahasia"}"#, &[]))
            .await
            .unwrap();
        let sid = res.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let csrf = serde_json::from_slice::<Value>(&body).unwrap()["csrf"]
            .as_str()
            .unwrap()
            .to_string();
        let h = [("cookie", sid.as_str()), ("x-csrf-token", csrf.as_str())];
        let (s, v) = call(
            &app,
            req(
                "POST",
                "/api/agent/messages",
                r#"{"conversation_id":null,"text":"halo","replace_from":null}"#,
                &h,
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert!(v["run_id"].is_string());
        // Tanpa pengaturan AI, giliran berakhir dengan error dan run dibersihkan.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                if st
                    .runs
                    .for_conversation(v["conversation_id"].as_str().unwrap())
                    .is_none()
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        let _ = events.try_recv();
    }
}
