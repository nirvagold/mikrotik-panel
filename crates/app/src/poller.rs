//! Sinkronisasi berkala dengan router.

use std::time::Duration;

use serde_json::json;

use crate::http::Shared;
use crate::telemetry;

/// Satu kali sinkronisasi. Dikunci supaya tombol "Perbarui" tidak bentrok dengan poller.
pub async fn sync_once(state: &Shared) -> anyhow::Result<()> {
    let _guard = state.sync_lock.lock().await;
    let last = telemetry::last_sample(&state.store).await;
    let sample = telemetry::fetch(&state.router, last).await?;
    let poll = state.config.router.poll_seconds;
    let sampled_at = sample.now.timestamp();
    let online = sample.customers.iter().filter(|c| c.online).count();
    let total = sample.customers.len();
    state
        .store
        .run(move |c| telemetry::record(c, &sample, poll))
        .await?;
    state.hub.publish(
        "telemetry.update",
        json!({ "sampled_at": sampled_at, "online": online, "customers": total }),
    );
    Ok(())
}

/// Penjelasan ramah untuk kegagalan sinkronisasi (bila penyebabnya error router).
pub fn hint_of(e: &anyhow::Error) -> String {
    e.downcast_ref::<crate::routeros::RouterError>()
        .map(|r| r.hint())
        .unwrap_or_else(|| "Sinkronisasi dengan router gagal. Coba lagi sebentar lagi.".into())
}

pub fn spawn(state: Shared) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(state.config.router.poll_seconds));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut failing = false;
        loop {
            tick.tick().await;
            match sync_once(&state).await {
                Ok(()) if failing => {
                    failing = false;
                    tracing::info!("sinkronisasi router pulih");
                    state.hub.publish("router.status", json!({ "ok": true }));
                }
                Ok(()) => {}
                Err(e) => {
                    if !failing {
                        tracing::warn!("sinkronisasi router gagal: {e}");
                    }
                    failing = true;
                    state.hub.publish(
                        "router.status",
                        json!({ "ok": false, "error": e.to_string(), "hint": hint_of(&e) }),
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use panel_core::auth::{LoginLimiter, Sessions};

    use super::*;
    use crate::http::AppState;
    use crate::routeros::{RouterOs, RouterTarget, Row, fake};

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn sync_and_quota_against_fake_router() {
        let started = (telemetry::now_wib() - chrono::Duration::hours(1))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let mut t: HashMap<String, Vec<Row>> = HashMap::new();
        t.insert(
            "/system/resource/print".into(),
            vec![row(&[("cpu-load", "3"), ("version", "7.16")])],
        );
        t.insert(
            "/system/identity/print".into(),
            vec![row(&[("name", "core")])],
        );
        t.insert(
            "/user-manager/user/print".into(),
            vec![row(&[("name", "budi")])],
        );
        t.insert(
            "/user-manager/user-profile/print".into(),
            vec![row(&[
                (".id", "*1"),
                ("user", "budi"),
                ("profile", "p-budi"),
                ("state", "running-active"),
            ])],
        );
        t.insert(
            "/user-manager/profile-limitation/print".into(),
            vec![row(&[
                (".id", "*2"),
                ("profile", "p-budi"),
                ("limitation", "l-budi"),
            ])],
        );
        t.insert(
            "/user-manager/limitation/print".into(),
            vec![row(&[
                (".id", "*3"),
                ("name", "l-budi"),
                ("transfer-limit", "10737418240"),
                ("reset-counters-start-time", "2026-01-01 00:00:00"),
                ("reset-counters-interval", "monthly"),
            ])],
        );
        t.insert(
            "/ppp/active/print".into(),
            vec![row(&[
                ("name", "budi"),
                ("service", "pppoe"),
                ("address", "10.0.0.2"),
            ])],
        );
        t.insert(
            "/interface/print".into(),
            vec![row(&[
                ("name", "ether1"),
                ("type", "ether"),
                ("rx-byte", "5"),
            ])],
        );
        t.insert(
            "/user-manager/session/print".into(),
            vec![row(&[
                ("acct-session-id", "a1"),
                ("user", "budi"),
                ("started", &started),
                ("download", "100"),
                ("upload", "10"),
            ])],
        );
        let tables = Arc::new(Mutex::new(t));
        let port = fake::start(tables.clone()).await;
        let config: panel_core::config::Config = toml::from_str(&format!(
            "secret_key = \"{}\"\nadmin_password_hash = \"$argon2id$x\"\n[router]\nhost = \"127.0.0.1\"\nport = {port}\nuser = \"u\"\npassword_encrypted = \"x\"\n",
            panel_core::crypto::Cipher::generate_key()
        ))
        .unwrap();
        let state: Shared = Arc::new(AppState {
            sessions: Sessions::new(Duration::from_secs(60)),
            limiter: LoginLimiter::new(5, Duration::from_secs(60)),
            store: crate::store::Store::open_in_memory().unwrap(),
            hub: Default::default(),
            router: Arc::new(RouterOs::new(RouterTarget {
                host: "127.0.0.1".into(),
                port,
                user: "u".into(),
                password: "p".into(),
                tls_fingerprint: None,
            })),
            quota: Default::default(),
            sync_lock: Default::default(),
            runs: Default::default(),
            config,
        });
        let mut events = state.hub.subscribe();
        sync_once(&state).await.unwrap();
        assert_eq!(events.recv().await.unwrap().kind, "telemetry.update");
        let dash = state
            .store
            .run(|c| telemetry::dashboard(c, 7, None))
            .await
            .unwrap();
        let budi = &dash["customers"][0];
        assert_eq!(budi["online"], true);
        assert_eq!(budi["used"], 110);
        assert_eq!(budi["remaining"], 10737418240u64 - 110);
        assert_eq!(dash["resources"]["version"], "7.16");

        // Tambah 5 GiB: pratinjau lalu terapkan, nilai di router berubah dan terverifikasi.
        let op = state.quota.preview(&state.router, "budi", 5).await.unwrap();
        assert_eq!(op.after, 10737418240 + 5 * telemetry::GIB);
        let op = state.quota.take(&op.id, "budi").unwrap();
        assert_eq!(state.quota.apply(&state.router, &op).await, "Terverifikasi");
        let now_limit =
            tables.lock().unwrap()["/user-manager/limitation/print"][0]["transfer-limit"].clone();
        assert_eq!(now_limit, (10737418240u64 + 5 * telemetry::GIB).to_string());
        // Menerapkan ulang pratinjau yang sama ditolak (nilai lama sudah berubah).
        assert_eq!(
            state.quota.apply(&state.router, &op).await,
            "Ditolak: profil atau batas kuota sudah berubah"
        );
        assert!(state.quota.take(&op.id, "budi").is_err());
    }
}
