//! Tambah kuota pelanggan: pratinjau dulu, lalu terapkan dengan pemeriksaan ulang di router.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::routeros::{Command, RouterError, RouterOs, Row};
use crate::telemetry::{self, GIB};

const PREVIEW_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize)]
pub struct PendingOp {
    pub id: String,
    pub account: String,
    pub profile: String,
    pub limitation: String,
    pub field: String,
    pub before: u64,
    pub after: u64,
    #[serde(skip)]
    created: Instant,
}

#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    #[error("Jumlah top-up harus 1–1000 GiB.")]
    BadAmount,
    #[error("Pelanggan tidak ditemukan di User Manager.")]
    NotFound,
    #[error("Top-up hanya tersedia untuk profil dengan satu batas kuota khusus pelanggan ini.")]
    NotEditable,
    #[error("Pratinjau sudah kedaluwarsa atau tidak dikenal. Buat pratinjau baru.")]
    Expired,
    #[error(transparent)]
    Router(#[from] RouterError),
}

#[derive(Default)]
pub struct Quota {
    pending: Mutex<HashMap<String, PendingOp>>,
    apply_lock: tokio::sync::Mutex<()>,
}

impl Quota {
    pub async fn preview(
        &self,
        router: &RouterOs,
        account: &str,
        amount_gib: u64,
    ) -> Result<PendingOp, QuotaError> {
        if !(1..=1000).contains(&amount_gib) {
            return Err(QuotaError::BadAmount);
        }
        let data = telemetry::customer_data(router).await?;
        let c = telemetry::customers_from(&data)
            .into_iter()
            .find(|c| c.name == account)
            .ok_or(QuotaError::NotFound)?;
        let (Some(profile), Some(limitation), Some(field), Some(before), true) =
            (c.profile, c.limitation, c.field, c.limit, c.editable)
        else {
            return Err(QuotaError::NotEditable);
        };
        let op = PendingOp {
            id: panel_core::crypto::random_token(),
            account: account.to_string(),
            profile,
            limitation,
            field,
            before,
            after: before + amount_gib * GIB,
            created: Instant::now(),
        };
        let mut map = self.pending.lock().unwrap();
        map.retain(|_, o| o.created.elapsed() < PREVIEW_TTL);
        map.insert(op.id.clone(), op.clone());
        Ok(op)
    }

    pub fn take(&self, id: &str, account: &str) -> Result<PendingOp, QuotaError> {
        let mut map = self.pending.lock().unwrap();
        match map.remove(id) {
            Some(op) if op.account == account && op.created.elapsed() < PREVIEW_TTL => Ok(op),
            _ => Err(QuotaError::Expired),
        }
    }

    /// Sedang menerapkan top-up.
    pub fn busy(&self) -> bool {
        self.apply_lock.try_lock().is_err()
    }

    /// Terapkan perubahan. Hasil: "Terverifikasi", "Ditolak: ..." atau "Hasil belum terverifikasi".
    pub async fn apply(&self, router: &RouterOs, op: &PendingOp) -> String {
        let _guard = self.apply_lock.lock().await;
        match apply_checked(router, op).await {
            Ok(true) => "Terverifikasi".into(),
            Ok(false) => "Ditolak: profil atau batas kuota sudah berubah".into(),
            Err(RouterError::Trap(m)) => format!("Ditolak router: {m}"),
            Err(e) => {
                tracing::warn!("perubahan kuota tidak terverifikasi: {e}");
                "Hasil belum terverifikasi".into()
            }
        }
    }
}

async fn rows(
    router: &RouterOs,
    path: &str,
    key: &str,
    value: &str,
    props: &[&str],
) -> Result<Vec<Row>, RouterError> {
    router
        .call(&Command::new(path)?.filter(key, value)?.proplist(props)?)
        .await
}

async fn apply_checked(router: &RouterOs, op: &PendingOp) -> Result<bool, RouterError> {
    let assignments = rows(
        router,
        "/user-manager/user-profile/print",
        "profile",
        &op.profile,
        &["user"],
    )
    .await?;
    let links = rows(
        router,
        "/user-manager/profile-limitation/print",
        "profile",
        &op.profile,
        &["limitation"],
    )
    .await?;
    let owners = rows(
        router,
        "/user-manager/profile-limitation/print",
        "limitation",
        &op.limitation,
        &["profile"],
    )
    .await?;
    let limits = rows(
        router,
        "/user-manager/limitation/print",
        "name",
        &op.limitation,
        &[".id", &op.field],
    )
    .await?;
    let ok = assignments.len() == 1
        && assignments[0].get("user") == Some(&op.account)
        && links.len() == 1
        && links[0].get("limitation") == Some(&op.limitation)
        && owners.len() == 1
        && limits.len() == 1
        && telemetry::number(limits[0].get(&op.field)) == op.before;
    if !ok {
        return Ok(false);
    }
    let id = limits[0].get(".id").cloned().unwrap_or_default();
    router
        .call(
            &Command::new("/user-manager/limitation/set")?
                .arg(".id", &id)?
                .arg(&op.field, op.after.to_string())?,
        )
        .await?;
    let check = rows(
        router,
        "/user-manager/limitation/print",
        "name",
        &op.limitation,
        &[&op.field],
    )
    .await?;
    Ok(check.len() == 1 && telemetry::number(check[0].get(&op.field)) == op.after)
}
