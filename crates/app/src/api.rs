//! API data panel: dashboard, pelanggan, kuota, router, riwayat.

use std::collections::{HashSet, VecDeque};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http::Shared;
use crate::quota::QuotaError;
use crate::routeros::Command;
use crate::{poller, telemetry};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/dashboard", get(dashboard))
        .route("/api/customers", get(customers))
        .route("/api/customers/{account}", get(customer))
        .route("/api/customers/{account}/logs", get(logs))
        .route(
            "/api/customers/{account}/quota/preview",
            post(quota_preview),
        )
        .route("/api/customers/{account}/quota/apply", post(quota_apply))
        .route("/api/router", get(router_info))
        .route("/api/router/refresh", post(refresh))
        .route("/api/audit", get(audit))
        .route("/api/router/logs", get(router_logs))
        .route("/api/recommendations", get(recommendations))
        .route("/api/actions", get(actions))
}

fn fail(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("{e}");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Terjadi kesalahan di server panel.",
    )
}

#[derive(Deserialize)]
struct Period {
    days: Option<i64>,
}

fn days(p: &Period) -> i64 {
    if p.days == Some(30) { 30 } else { 7 }
}

async fn load(state: &Shared, days: i64, account: Option<String>) -> Result<Value, Response> {
    state
        .store
        .run(move |c| telemetry::dashboard(c, days, account.as_deref()))
        .await
        .map_err(internal)
}

async fn dashboard(State(state): State<Shared>, Query(p): Query<Period>) -> Response {
    match load(&state, days(&p), None).await {
        Ok(v) => Json(v).into_response(),
        Err(r) => r,
    }
}

async fn customers(State(state): State<Shared>) -> Response {
    match load(&state, 1, None).await {
        Ok(v) => Json(json!({ "sampled_at": v["sampled_at"], "customers": v["customers"] }))
            .into_response(),
        Err(r) => r,
    }
}

async fn customer(
    State(state): State<Shared>,
    Path(account): Path<String>,
    Query(p): Query<Period>,
) -> Response {
    let v = match load(&state, days(&p), Some(account.clone())).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let row = v["customers"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["name"] == account.as_str()))
        .cloned();
    match row {
        Some(row) => {
            Json(json!({ "sampled_at": v["sampled_at"], "customer": row, "history": v["history"] }))
                .into_response()
        }
        None => fail(StatusCode::NOT_FOUND, "Pelanggan tidak ditemukan."),
    }
}

async fn logs(State(state): State<Shared>, Path(account): Path<String>) -> Response {
    let wanted: HashSet<&str> = ["ppp", "pppoe", "radius"].into();
    let mut last = VecDeque::with_capacity(20);
    let cmd =
        match Command::new("/log/print").and_then(|c| c.proplist(&["time", "topics", "message"])) {
            Ok(c) => c,
            Err(e) => return internal(e),
        };
    let result = state
        .router
        .each(&cmd, &mut |row| {
            let msg = row.get("message").map(String::as_str).unwrap_or("");
            let topics = row.get("topics").map(String::as_str).unwrap_or("");
            if msg.contains(account.as_str()) && topics.split(',').any(|t| wanted.contains(t)) {
                if last.len() == 20 {
                    last.pop_front();
                }
                last.push_back(row);
            }
        })
        .await;
    match result {
        Ok(_) => Json(json!({ "logs": last })).into_response(),
        Err(e) => fail(
            StatusCode::BAD_GATEWAY,
            format!("Log gagal dibaca dari MikroTik: {e}"),
        ),
    }
}

fn quota_fail(e: QuotaError) -> Response {
    let status = match e {
        QuotaError::Router(_) => StatusCode::BAD_GATEWAY,
        QuotaError::NotFound => StatusCode::NOT_FOUND,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    fail(status, e.to_string())
}

#[derive(Deserialize)]
struct PreviewBody {
    amount_gib: u64,
}

async fn quota_preview(
    State(state): State<Shared>,
    Path(account): Path<String>,
    Json(b): Json<PreviewBody>,
) -> Response {
    match state
        .quota
        .preview(&state.router, &account, b.amount_gib)
        .await
    {
        Ok(op) => Json(op).into_response(),
        Err(e) => quota_fail(e),
    }
}

#[derive(Deserialize)]
struct ApplyBody {
    id: String,
}

async fn quota_apply(
    State(state): State<Shared>,
    Path(account): Path<String>,
    Json(b): Json<ApplyBody>,
) -> Response {
    let op = match state.quota.take(&b.id, &account) {
        Ok(op) => op,
        Err(e) => return quota_fail(e),
    };
    let audit_id = op.id.clone();
    let (acc, before, after) = (
        op.account.clone(),
        op.before.to_string(),
        op.after.to_string(),
    );
    let at = crate::store::now();
    let inserted = state
        .store
        .run(move |c| {
            c.execute(
                "INSERT INTO audit (id, at, actor, account, action, before, after, status)
                 VALUES (?1, ?2, 'admin', ?3, 'quota.increase', ?4, ?5, 'Menunggu verifikasi')",
                (&audit_id, at, &acc, &before, &after),
            )
        })
        .await;
    if let Err(e) = inserted {
        return internal(e);
    }
    let status = state.quota.apply(&state.router, &op).await;
    let (id, s) = (op.id.clone(), status.clone());
    if let Err(e) = state
        .store
        .run(move |c| c.execute("UPDATE audit SET status = ?1 WHERE id = ?2", (&s, &id)))
        .await
    {
        return internal(e);
    }
    if status == "Terverifikasi" {
        let st = state.clone();
        tokio::spawn(async move {
            let _ = poller::sync_once(&st).await;
        });
    }
    Json(json!({ "status": status, "operation": op })).into_response()
}

async fn router_info(State(state): State<Shared>) -> Response {
    match load(&state, 1, None).await {
        Ok(v) => Json(json!({ "sampled_at": v["sampled_at"], "resources": v["resources"], "interfaces": v["interfaces"] }))
            .into_response(),
        Err(r) => r,
    }
}

async fn refresh(State(state): State<Shared>) -> Response {
    match poller::sync_once(&state).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => fail(
            StatusCode::BAD_GATEWAY,
            format!("MikroTik belum merespons: {e}"),
        ),
    }
}

async fn audit(State(state): State<Shared>) -> Response {
    let rows = state
        .store
        .run(|c| {
            let mut s = c.prepare(
                "SELECT id, at, actor, account, action, before, after, status FROM audit ORDER BY at DESC LIMIT 100",
            )?;
            let v = s
                .query_map([], |r| {
                    Ok(json!({
                        "id": r.get::<_, String>(0)?, "at": r.get::<_, i64>(1)?, "actor": r.get::<_, String>(2)?,
                        "account": r.get::<_, Option<String>>(3)?, "action": r.get::<_, String>(4)?,
                        "before": r.get::<_, Option<String>>(5)?, "after": r.get::<_, Option<String>>(6)?,
                        "status": r.get::<_, String>(7)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(v)
        })
        .await;
    match rows {
        Ok(v) => Json(json!({ "audit": v })).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct LogQuery {
    limit: Option<u64>,
    topics: Option<String>,
}

async fn router_logs(State(state): State<Shared>, Query(q): Query<LogQuery>) -> Response {
    let mut input = json!({ "limit": q.limit.unwrap_or(50) });
    if let Some(t) = q.topics.filter(|t| !t.is_empty()) {
        input["topics"] = json!(t.split(',').collect::<Vec<_>>());
    }
    match crate::agent_host::run_tool(&state, "get_logs", &input).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, format!("Log gagal dibaca dari MikroTik: {e}")),
    }
}

async fn recommendations(State(state): State<Shared>) -> Response {
    let rows = state
        .store
        .run(|c| {
            let mut s = c.prepare("SELECT id, kind, finding, evidence, status, action_id, created_at FROM recommendations ORDER BY created_at DESC LIMIT 50")?;
            s.query_map([], |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?, "finding": r.get::<_, String>(2)?,
                           "evidence": r.get::<_, Option<String>>(3)?, "status": r.get::<_, String>(4)?,
                           "action_id": r.get::<_, Option<String>>(5)?, "created_at": r.get::<_, i64>(6)? }))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .await;
    match rows {
        Ok(v) => Json(json!({ "recommendations": v })).into_response(),
        Err(e) => internal(e),
    }
}

async fn actions(State(state): State<Shared>) -> Response {
    let rows = state
        .store
        .run(|c| {
            let mut s = c.prepare("SELECT id, label, risk, placement, created_at, approved_at FROM custom_actions ORDER BY created_at DESC")?;
            s.query_map([], |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "label": r.get::<_, String>(1)?, "risk": r.get::<_, String>(2)?,
                           "placement": r.get::<_, Option<String>>(3)?, "created_at": r.get::<_, i64>(4)?,
                           "approved_at": r.get::<_, Option<i64>>(5)? }))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .await;
    match rows {
        Ok(v) => Json(json!({ "actions": v })).into_response(),
        Err(e) => internal(e),
    }
}
