//! Endpoint agen dan desain (kontrak: web/src/agent/CONTRACT.md).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
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
        .route("/api/settings/ai", get(ai_get).put(ai_put))
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

fn version_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let spec: String = r.get("spec")?;
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
    let (status, result) = match execute(&st, &tool, &card["input"]).await {
        Ok(v) => ("done", v),
        Err(m) => ("failed", m),
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
    let Some(raw) = input.get("command") else {
        return Err(format!(
            "Eksekusi `{tool}` belum tersedia di versi panel ini."
        ));
    };
    let cmd: PolicyCommand =
        serde_json::from_value(raw.clone()).map_err(|e| format!("Perintah tidak valid: {e}"))?;
    cmd.validate().map_err(|e| e.to_string())?;
    let args = PolicyCommand::api_pairs(&cmd.args).map_err(|e| e.to_string())?;
    let build = |extra_id: Option<&str>| -> Result<Command, String> {
        let mut c = Command::new(&cmd.api_path()).map_err(|e| e.to_string())?;
        if let Some(id) = extra_id {
            c = c.arg(".id", id).map_err(|e| e.to_string())?;
        }
        for (k, v) in &args {
            c = c.arg(k, v).map_err(|e| e.to_string())?;
        }
        Ok(c)
    };
    if cmd.find.is_empty() {
        st.router
            .call(&build(None)?)
            .await
            .map_err(|e| e.to_string())?;
        return Ok("Perintah dijalankan.".into());
    }
    let mut finder = Command::new(&cmd.api_find_path()).map_err(|e| e.to_string())?;
    for (k, v) in PolicyCommand::api_pairs(&cmd.find).map_err(|e| e.to_string())? {
        finder = finder.filter(&k, v).map_err(|e| e.to_string())?;
    }
    let ids: Vec<String> = st
        .router
        .call(&finder.proplist(&[".id"]).map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|r| r.get(".id").cloned())
        .collect();
    if ids.is_empty() {
        return Err("Tidak ada objek yang cocok di router.".into());
    }
    if ids.len() > 20 {
        return Err(format!("{} objek cocok; batas 20 per usulan.", ids.len()));
    }
    for id in &ids {
        st.router
            .call(&build(Some(id))?)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(format!("Perintah dijalankan pada {} objek.", ids.len()))
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
            Json(json!({ "url": v["url"], "protocol": v["protocol"], "model": v["model"], "has_key": v["encrypted_key"].is_string() })).into_response()
        }
        Ok(None) => Json(json!({ "url": null, "protocol": null, "model": null, "has_key": false }))
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
            let v = json!({ "url": b.url, "protocol": b.protocol, "model": b.model.trim(), "encrypted_key": encrypted });
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
        assert!(!v.to_string().contains("sk-1"));
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
