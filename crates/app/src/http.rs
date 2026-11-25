//! Server HTTP: autentikasi, CSRF, header keamanan, health check, dan kanal SSE.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use panel_core::auth::{self, CSRF_HEADER, LoginLimiter, SESSION_COOKIE, Sessions};
use panel_core::config::Config;
use panel_core::crypto;
use serde::Deserialize;
use serde_json::json;
use tower_http::limit::RequestBodyLimitLayer;

use crate::events::Hub;
use crate::routeros::RouterOs;
use crate::store::Store;

pub struct AppState {
    pub config: Config,
    pub sessions: Sessions,
    pub limiter: LoginLimiter,
    pub store: Store,
    pub hub: Hub,
    pub router: Arc<RouterOs>,
    pub quota: crate::quota::Quota,
    pub sync_lock: tokio::sync::Mutex<()>,
}

pub type Shared = Arc<AppState>;

pub fn app(state: Shared) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/session", get(session_info))
        .route("/api/events", get(events))
        .merge(crate::api::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ));
    Router::new()
        .merge(api)
        .fallback(get(spa))
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Semua /api/* butuh sesi kecuali login dan health; semua metode selain GET butuh token CSRF.
async fn require_session(State(state): State<Shared>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if matches!(path, "/api/login" | "/api/health") {
        return next.run(req).await;
    }
    let Some(session) =
        cookie_value(req.headers(), SESSION_COOKIE).and_then(|t| state.sessions.get(&t))
    else {
        return error(
            StatusCode::UNAUTHORIZED,
            "Sesi panel berakhir. Silakan masuk kembali.",
        );
    };
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let sent = req
            .headers()
            .get(CSRF_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !auth::tokens_match(sent, &session.csrf) {
            return error(
                StatusCode::FORBIDDEN,
                "Token keamanan tidak cocok. Muat ulang halaman.",
            );
        }
    }
    next.run(req).await
}

async fn security_headers(req: Request, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; \
             connect-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'",
        ),
    );
    if is_api {
        h.entry(header::CACHE_CONTROL)
            .or_insert(HeaderValue::from_static("no-store"));
    }
    res
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(
    State(state): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    let forwarded = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    let ip = auth::client_ip(peer.ip(), forwarded, &state.config.trusted_proxies);
    if let Some(wait) = state.limiter.locked(ip) {
        let minutes = wait.as_secs().div_ceil(60);
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("Terlalu banyak percobaan. Coba lagi dalam {minutes} menit."),
        );
    }
    let hash = state.config.admin_password_hash.clone();
    let password = body.password;
    let ok = tokio::task::spawn_blocking(move || crypto::verify_password(&password, &hash))
        .await
        .unwrap_or(false);
    let at = crate::store::now();
    let ip_text = ip.to_string();
    let _ = state
        .store
        .run(move |c| {
            c.execute(
                "INSERT INTO login_attempts (ip, at, ok) VALUES (?1, ?2, ?3)",
                (ip_text, at, ok),
            )
        })
        .await;
    if !ok {
        state.limiter.failed(ip);
        return error(StatusCode::UNAUTHORIZED, "Kata sandi salah.");
    }
    state.limiter.succeeded(ip);
    let (token, csrf) = state.sessions.create();
    let secure = if state.config.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
        state.sessions.ttl().as_secs()
    );
    let mut res = Json(json!({ "csrf": csrf })).into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("cookie ascii"),
    );
    res
}

async fn logout(State(state): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_value(&headers, SESSION_COOKIE) {
        state.sessions.remove(&token);
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"
        ))
        .unwrap(),
    );
    res
}

async fn session_info(State(state): State<Shared>, headers: HeaderMap) -> Response {
    match cookie_value(&headers, SESSION_COOKIE).and_then(|t| state.sessions.get(&t)) {
        Some(s) => Json(json!({ "csrf": s.csrf })).into_response(),
        None => error(StatusCode::UNAUTHORIZED, "Belum masuk."),
    }
}

/// Dipakai systemd dan proses deploy: 200 bila database sehat, 503 bila tidak.
/// Status router dilaporkan tapi tidak membuat health gagal, supaya panel tetap bisa
/// dipasang/di-rollback saat router sedang mati.
async fn health(State(state): State<Shared>) -> Response {
    let db = state
        .store
        .run(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)))
        .await;
    let router = tokio::time::timeout(Duration::from_secs(3), state.router.ping()).await;
    let router = match router {
        Ok(Ok(identity)) => json!({ "ok": true, "identity": identity }),
        Ok(Err(e)) => json!({ "ok": false, "error": e.to_string() }),
        Err(_) => json!({ "ok": false, "error": "router tidak menjawab dalam 3 detik" }),
    };
    let db_ok = db.is_ok();
    let status = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = json!({ "ok": db_ok, "version": env!("CARGO_PKG_VERSION"), "database": { "ok": db_ok }, "router": router });
    (status, Json(body)).into_response()
}

async fn events(State(state): State<Shared>) -> impl IntoResponse {
    state.hub.sse()
}

/// Sementara: halaman pengganti sampai SPA Preact (tahap 3) ditanam di binary.
async fn spa() -> Html<&'static str> {
    Html(
        "<!doctype html><html lang=\"id\"><meta charset=\"utf-8\"><title>Panel MikroTik</title>\
         <body><p>Panel MikroTik (Rust) berjalan. Antarmuka baru menyusul di tahap 3.</p></body></html>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routeros::RouterTarget;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn state() -> Shared {
        let config: Config = toml::from_str(&format!(
            r#"
            secret_key = "{}"
            admin_password_hash = "{}"
            cookie_secure = false
            trusted_proxies = ["127.0.0.1"]
            [router]
            host = "127.0.0.1"
            port = 1
            user = "panel"
            password_encrypted = "x"
            "#,
            crypto::Cipher::generate_key(),
            crypto::hash_password("rahasia").unwrap()
        ))
        .unwrap();
        Arc::new(AppState {
            sessions: Sessions::new(Duration::from_secs(3600)),
            limiter: LoginLimiter::new(3, Duration::from_secs(900)),
            store: Store::open_in_memory().unwrap(),
            hub: Hub::default(),
            router: Arc::new(RouterOs::new(RouterTarget {
                host: "127.0.0.1".into(),
                port: 1,
                user: "panel".into(),
                password: "x".into(),
                tls_fingerprint: None,
            })),
            quota: Default::default(),
            sync_lock: Default::default(),
            config,
        })
    }

    fn req(method: &str, uri: &str, body: &str, extra: &[(&str, &str)]) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let mut r = b.body(Body::from(body.to_string())).unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
        r
    }

    #[tokio::test]
    async fn login_session_csrf_flow() {
        let app = app(state());
        let res = app
            .clone()
            .oneshot(req("GET", "/api/session", "", &[]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let res = app
            .clone()
            .oneshot(req("POST", "/api/login", r#"{"password":"salah"}"#, &[]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let res = app
            .clone()
            .oneshot(req("POST", "/api/login", r#"{"password":"rahasia"}"#, &[]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cookie = res.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
        let sid = cookie.split(';').next().unwrap().to_string();
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let csrf = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["csrf"]
            .as_str()
            .unwrap()
            .to_string();

        let res = app
            .clone()
            .oneshot(req("GET", "/api/session", "", &[("cookie", &sid)]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // POST tanpa CSRF ditolak, dengan CSRF diterima.
        let res = app
            .clone()
            .oneshot(req("POST", "/api/logout", "", &[("cookie", &sid)]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let res = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/logout",
                "",
                &[("cookie", &sid), (CSRF_HEADER, &csrf)],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let res = app
            .clone()
            .oneshot(req("GET", "/api/session", "", &[("cookie", &sid)]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn lockout_uses_forwarded_ip_from_trusted_proxy() {
        let app = app(state());
        for _ in 0..3 {
            let r = req(
                "POST",
                "/api/login",
                r#"{"password":"x"}"#,
                &[("x-forwarded-for", "198.51.100.1")],
            );
            app.clone().oneshot(r).await.unwrap();
        }
        let blocked = req(
            "POST",
            "/api/login",
            r#"{"password":"rahasia"}"#,
            &[("x-forwarded-for", "198.51.100.1")],
        );
        assert_eq!(
            app.clone().oneshot(blocked).await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        // Pengunjung lain di balik proxy yang sama tidak ikut terkunci.
        let other = req(
            "POST",
            "/api/login",
            r#"{"password":"rahasia"}"#,
            &[("x-forwarded-for", "198.51.100.2")],
        );
        assert_eq!(app.oneshot(other).await.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_reports_db_and_router() {
        let res = app(state())
            .oneshot(req("GET", "/api/health", "", &[]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().contains_key("content-security-policy"));
        let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["database"]["ok"], true);
        assert_eq!(v["router"]["ok"], false);
    }
}
