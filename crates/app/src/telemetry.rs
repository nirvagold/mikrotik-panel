//! Telemetri: membaca data pelanggan dan sesi dari router, menghitung trafik harian per akun,
//! dan sisa kuota per siklus. Perbaikan terhadap panel lama:
//! - sesi di-stream dari router, tanpa batas 8 MB;
//! - delta dihitung dari counter terakhir per sesi, jadi jeda sinkronisasi tidak menghilangkan trafik;
//! - delta yang melewati tengah malam dibagi proporsional ke tiap hari (WIB);
//! - reset kuota tanggal 29–31 jatuh di hari terakhir bulan yang lebih pendek.

use std::collections::{BTreeMap, HashMap};

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Timelike,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::routeros::{Command, RouterError, RouterOs, Row};
use crate::store::Store;

pub const GIB: u64 = 1024 * 1024 * 1024;

pub fn wib() -> FixedOffset {
    FixedOffset::east_opt(7 * 3600).expect("offset WIB")
}

pub fn now_wib() -> DateTime<FixedOffset> {
    chrono::Utc::now().with_timezone(&wib())
}

pub fn number(value: Option<&String>) -> u64 {
    value
        .map(|v| v.trim_end_matches('B'))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Waktu dari RouterOS (v7 "2025-11-20 10:00:00", juga format v6 "nov/20/2025 10:00:00").
pub fn parse_time(value: Option<&String>) -> Option<DateTime<FixedOffset>> {
    let text = value?.trim();
    if text.is_empty() {
        return None;
    }
    let normalized = capitalize_month(text);
    [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%b/%d/%Y %H:%M:%S",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(&normalized, f).ok())
    .and_then(|naive| wib().from_local_datetime(&naive).single())
}

fn capitalize_month(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string() + chars.as_str(),
        _ => text.to_string(),
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
        .day()
}

fn at_day(
    year: i32,
    month: u32,
    day: u32,
    base: &DateTime<FixedOffset>,
) -> Option<DateTime<FixedOffset>> {
    let d = day.min(days_in_month(year, month));
    wib()
        .with_ymd_and_hms(year, month, d, base.hour(), base.minute(), base.second())
        .single()
}

/// Awal siklus kuota bulanan yang sedang berjalan.
pub fn cycle_start(
    now: DateTime<FixedOffset>,
    anchor: Option<&String>,
) -> Option<DateTime<FixedOffset>> {
    let base = parse_time(anchor)?;
    if base > now {
        return None;
    }
    let candidate = at_day(now.year(), now.month(), base.day(), &base)?;
    if candidate <= now {
        return Some(candidate);
    }
    let (y, m) = if now.month() == 1 {
        (now.year() - 1, 12)
    } else {
        (now.year(), now.month() - 1)
    };
    at_day(y, m, base.day(), &base)
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct Customer {
    pub name: String,
    pub disabled: bool,
    pub profile: Option<String>,
    pub limitation: Option<String>,
    pub field: Option<String>,
    pub limit: Option<u64>,
    pub reset: Option<String>,
    pub interval: Option<String>,
    pub online: bool,
    pub address: Option<String>,
    pub uptime: Option<String>,
    pub editable: bool,
    pub used: Option<u64>,
    pub remaining: Option<u64>,
    pub approximate: bool,
    pub cycle_start: Option<String>,
}

pub struct CustomerData {
    pub users: Vec<Row>,
    pub profiles: Vec<Row>,
    pub links: Vec<Row>,
    pub limits: Vec<Row>,
    pub active: Vec<Row>,
}

const LIMIT_FIELDS: [&str; 3] = ["download-limit", "upload-limit", "transfer-limit"];

pub async fn customer_data(router: &RouterOs) -> Result<CustomerData, RouterError> {
    let q = |path: &str, props: &[&str]| -> Result<Command, RouterError> {
        Command::new(path)?.proplist(props)
    };
    Ok(CustomerData {
        users: router
            .call(&q("/user-manager/user/print", &["name", "disabled"])?)
            .await?,
        profiles: router
            .call(&q(
                "/user-manager/user-profile/print",
                &[".id", "user", "profile", "state", "end-time"],
            )?)
            .await?,
        links: router
            .call(&q(
                "/user-manager/profile-limitation/print",
                &[".id", "profile", "limitation"],
            )?)
            .await?,
        limits: router
            .call(&q(
                "/user-manager/limitation/print",
                &[
                    ".id",
                    "name",
                    "download-limit",
                    "upload-limit",
                    "transfer-limit",
                    "reset-counters-start-time",
                    "reset-counters-interval",
                ],
            )?)
            .await?,
        active: router
            .call(
                &q(
                    "/ppp/active/print",
                    &["name", "service", "address", "uptime"],
                )?
                .filter("service", "pppoe")?,
            )
            .await?,
    })
}

fn get<'a>(row: &'a Row, key: &str) -> Option<&'a String> {
    row.get(key)
}

pub fn customers_from(data: &CustomerData) -> Vec<Customer> {
    let limits: HashMap<&str, &Row> = data
        .limits
        .iter()
        .filter_map(|r| Some((get(r, "name")?.as_str(), r)))
        .collect();
    let active: HashMap<&str, &Row> = data
        .active
        .iter()
        .filter_map(|r| Some((get(r, "name")?.as_str(), r)))
        .collect();
    let mut out: Vec<Customer> = data
        .users
        .iter()
        .filter_map(|user| {
            let name = get(user, "name")?.clone();
            let running: Vec<&Row> = data
                .profiles
                .iter()
                .filter(|p| {
                    get(p, "user") == Some(&name)
                        && get(p, "state").is_some_and(|s| s.starts_with("running"))
                })
                .collect();
            let profile = (running.len() == 1)
                .then(|| get(running[0], "profile").cloned())
                .flatten();
            let own_links: Vec<&Row> = data
                .links
                .iter()
                .filter(|l| profile.is_some() && get(l, "profile") == profile.as_ref())
                .collect();
            let limitation = (own_links.len() == 1)
                .then(|| get(own_links[0], "limitation").cloned())
                .flatten();
            let limit_row = limitation.as_deref().and_then(|n| limits.get(n)).copied();
            let enabled: Vec<&str> = LIMIT_FIELDS
                .iter()
                .copied()
                .filter(|f| limit_row.is_some_and(|r| number(get(r, f)) > 0))
                .collect();
            let field = (enabled.len() == 1).then(|| enabled[0].to_string());
            let limit = field
                .as_deref()
                .and_then(|f| limit_row.map(|r| number(get(r, f))));
            let session = active.get(name.as_str()).copied();
            let editable = field.is_some()
                && data
                    .profiles
                    .iter()
                    .filter(|p| get(p, "profile") == profile.as_ref())
                    .count()
                    == 1
                && data
                    .links
                    .iter()
                    .filter(|l| get(l, "limitation") == limitation.as_ref())
                    .count()
                    == 1;
            Some(Customer {
                disabled: get(user, "disabled").is_some_and(|v| v == "true" || v == "yes"),
                profile,
                limitation,
                field,
                limit,
                reset: limit_row.and_then(|r| get(r, "reset-counters-start-time").cloned()),
                interval: limit_row.and_then(|r| get(r, "reset-counters-interval").cloned()),
                online: session.is_some(),
                address: session.and_then(|s| get(s, "address").cloned()),
                uptime: session.and_then(|s| get(s, "uptime").cloned()),
                editable,
                used: None,
                remaining: None,
                approximate: false,
                cycle_start: None,
                name,
            })
        })
        .collect();
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// Satu baris sesi accounting User Manager.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub key: String,
    pub user: String,
    pub started: DateTime<FixedOffset>,
    pub ended: Option<DateTime<FixedOffset>>,
    pub download: u64,
    pub upload: u64,
}

impl SessionRow {
    pub fn from_row(row: &Row) -> Option<Self> {
        let user = get(row, "user")?.clone();
        let started = parse_time(get(row, "started"))?;
        let id = get(row, "acct-session-id").cloned().unwrap_or_default();
        Some(Self {
            key: format!("{user}|{}|{id}", started.format("%Y-%m-%d %H:%M:%S")),
            user,
            started,
            ended: parse_time(get(row, "ended")),
            download: number(get(row, "download")),
            upload: number(get(row, "upload")),
        })
    }
}

/// Akumulasi pemakaian kuota per pelanggan selama sesi di-stream.
pub struct UsageAccumulator {
    targets: HashMap<String, (DateTime<FixedOffset>, String)>,
    used: HashMap<String, (u64, bool)>,
}

impl UsageAccumulator {
    pub fn new(customers: &[Customer], now: DateTime<FixedOffset>) -> Self {
        let targets = customers
            .iter()
            .filter(|c| c.limit.is_some() && c.interval.as_deref() == Some("monthly"))
            .filter_map(|c| {
                Some((
                    c.name.clone(),
                    (cycle_start(now, c.reset.as_ref())?, c.field.clone()?),
                ))
            })
            .collect();
        Self {
            targets,
            used: HashMap::new(),
        }
    }

    pub fn add(&mut self, s: &SessionRow) {
        let Some((start, field)) = self.targets.get(&s.user) else {
            return;
        };
        let entry = self.used.entry(s.user.clone()).or_insert((0, false));
        if s.started >= *start {
            if field != "upload-limit" {
                entry.0 += s.download;
            }
            if field != "download-limit" {
                entry.0 += s.upload;
            }
        } else if s.ended.is_none_or(|e| e >= *start) {
            // Sesi melintasi awal siklus: porsi sebelum reset tidak bisa dipisahkan.
            entry.1 = true;
        }
    }

    pub fn apply(self, customers: &mut [Customer]) {
        for c in customers {
            if let Some((start, _)) = self.targets.get(&c.name) {
                let (used, approximate) = self.used.get(&c.name).copied().unwrap_or((0, false));
                c.used = Some(used);
                c.approximate = approximate;
                c.remaining = (!approximate).then(|| c.limit.unwrap_or(0).saturating_sub(used));
                c.cycle_start = Some(start.date_naive().to_string());
            }
        }
    }
}

/// Bagi `(dl, ul)` byte ke tiap hari WIB secara proporsional terhadap lama waktu di hari itu.
pub fn split_by_day(
    download: u64,
    upload: u64,
    from: DateTime<FixedOffset>,
    to: DateTime<FixedOffset>,
) -> Vec<(String, u64, u64)> {
    let total = (to - from).num_seconds();
    if total <= 0 || from.date_naive() == to.date_naive() {
        return vec![(to.date_naive().to_string(), download, upload)];
    }
    let mut parts = Vec::new();
    let (mut given_dl, mut given_ul) = (0u64, 0u64);
    let mut cursor = from;
    while cursor.date_naive() < to.date_naive() {
        let next_midnight = wib()
            .from_local_datetime(
                &(cursor.date_naive() + Duration::days(1))
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            )
            .single()
            .unwrap();
        let secs = (next_midnight - cursor).num_seconds() as u128;
        let dl = (download as u128 * secs / total as u128) as u64;
        let ul = (upload as u128 * secs / total as u128) as u64;
        parts.push((cursor.date_naive().to_string(), dl, ul));
        given_dl += dl;
        given_ul += ul;
        cursor = next_midnight;
    }
    parts.push((
        to.date_naive().to_string(),
        download - given_dl,
        upload - given_ul,
    ));
    parts
}

/// Hasil satu kali sinkronisasi.
pub struct Sample {
    pub resources: Row,
    pub customers: Vec<Customer>,
    pub interfaces: Vec<Row>,
    pub sessions: Vec<SessionRow>,
    pub now: DateTime<FixedOffset>,
}

/// Ambil semua data dari router. Sesi lama yang sudah selesai sebelum sampel terakhir tidak disimpan
/// di memori; hanya dihitung untuk pemakaian kuota.
pub async fn fetch(router: &RouterOs, last_sample: Option<i64>) -> Result<Sample, RouterError> {
    let now = now_wib();
    let resources = router
        .call(&Command::new("/system/resource/print")?.proplist(&[
            "cpu-load",
            "free-memory",
            "total-memory",
            "uptime",
            "version",
            "board-name",
        ])?)
        .await?
        .into_iter()
        .next()
        .unwrap_or_default();
    let data = customer_data(router).await?;
    let mut customers = customers_from(&data);
    let interfaces = router
        .call(
            &Command::new("/interface/print")?
                .proplist(&["name", "type", "running", "rx-byte", "tx-byte"])?
                .filter("type", "ether")?,
        )
        .await?;
    let mut usage = UsageAccumulator::new(&customers, now);
    let mut sessions = Vec::new();
    let cmd = Command::new("/user-manager/session/print")?.proplist(&[
        "acct-session-id",
        "user",
        "started",
        "ended",
        "download",
        "upload",
    ])?;
    router
        .each(&cmd, &mut |row| {
            let Some(s) = SessionRow::from_row(&row) else {
                return;
            };
            usage.add(&s);
            let relevant = match (s.ended, last_sample) {
                (None, _) => true,
                (Some(end), Some(last)) => end.timestamp() >= last,
                (Some(_), None) => false,
            };
            if relevant {
                sessions.push(s);
            }
        })
        .await?;
    usage.apply(&mut customers);
    Ok(Sample {
        resources,
        customers,
        interfaces,
        sessions,
        now,
    })
}

/// Tulis sampel ke database. Mengembalikan detik cakupan yang ditambahkan.
pub fn record(
    conn: &mut rusqlite::Connection,
    sample: &Sample,
    poll_seconds: u64,
) -> rusqlite::Result<i64> {
    let tx = conn.transaction()?;
    let now = sample.now;
    let stamp = now.timestamp();
    let last: Option<i64> = tx
        .query_row("SELECT sampled_at FROM snapshot WHERE id = 1", [], |r| {
            r.get(0)
        })
        .ok();
    if last.is_some_and(|l| l >= stamp) {
        return Ok(0);
    }
    let first_run = last.is_none();
    let mut counters: HashMap<String, (u64, u64, i64)> = HashMap::new();
    {
        let mut stmt =
            tx.prepare("SELECT session_id, download, upload, sampled_at FROM session_counters")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                    r.get(3)?,
                ),
            ))
        })?;
        for row in rows {
            let (k, v) = row?;
            counters.insert(k, v);
        }
    }
    let mut daily: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    for s in &sample.sessions {
        let prior = counters.remove(&s.key);
        let (base_dl, base_ul, from) = match prior {
            Some((dl, ul, at)) if s.download >= dl && s.upload >= ul => (
                dl,
                ul,
                wib()
                    .timestamp_opt(at, 0)
                    .single()
                    .unwrap_or(s.started)
                    .max(s.started),
            ),
            // Counter mundur (sesi diulang): hitung dari nol sejak sesi mulai.
            Some(_) => (0, 0, s.started),
            // Sesi yang sudah berjalan saat panel pertama kali menyala: jadikan patokan saja.
            None if first_run => (s.download, s.upload, now),
            None => (0, 0, s.started),
        };
        let to = s.ended.unwrap_or(now).min(now).max(from);
        let (dl, ul) = (s.download - base_dl, s.upload - base_ul);
        if dl > 0 || ul > 0 {
            for (day, d, u) in split_by_day(dl, ul, from, to) {
                let e = daily.entry((day, s.user.clone())).or_default();
                e.0 += d;
                e.1 += u;
            }
        }
        if s.ended.is_some() {
            tx.execute(
                "DELETE FROM session_counters WHERE session_id = ?1",
                [&s.key],
            )?;
        } else {
            tx.execute(
                "INSERT INTO session_counters (session_id, account, download, upload, sampled_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id) DO UPDATE SET download = excluded.download, upload = excluded.upload, sampled_at = excluded.sampled_at",
                (&s.key, &s.user, s.download as i64, s.upload as i64, stamp),
            )?;
        }
    }
    // Sesi yang hilang dari router tanpa tercatat selesai: buang counternya.
    for key in counters.keys() {
        tx.execute("DELETE FROM session_counters WHERE session_id = ?1", [key])?;
    }
    for ((day, account), (dl, ul)) in &daily {
        tx.execute(
            "INSERT INTO daily (day, account, download, upload) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(day, account) DO UPDATE SET download = download + excluded.download, upload = upload + excluded.upload",
            (day, account, *dl as i64, *ul as i64),
        )?;
    }
    let mut covered = 0;
    if let Some(prev) = last {
        let gap = stamp - prev;
        if gap > 0 && gap <= (poll_seconds as i64) * 3 {
            let from = wib().timestamp_opt(prev, 0).single().unwrap();
            // Bagi detik cakupan ke tiap hari dengan cara yang sama seperti trafik.
            for (day, secs, _) in split_by_day(gap as u64, 0, from, now) {
                tx.execute(
                    "INSERT INTO coverage (day, seconds) VALUES (?1, ?2)
                     ON CONFLICT(day) DO UPDATE SET seconds = MIN(86400, seconds + excluded.seconds)",
                    (&day, secs as i64),
                )?;
            }
            covered = gap;
        }
    }
    let snapshot = json!({
        "resources": sample.resources,
        "customers": sample.customers,
        "interfaces": sample.interfaces,
    });
    tx.execute(
        "INSERT INTO snapshot (id, sampled_at, data) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET sampled_at = excluded.sampled_at, data = excluded.data",
        (stamp, snapshot.to_string()),
    )?;
    // Simpan riwayat harian 400 hari.
    let cutoff = (now.date_naive() - Duration::days(400)).to_string();
    tx.execute("DELETE FROM daily WHERE day < ?1", [&cutoff])?;
    tx.execute("DELETE FROM coverage WHERE day < ?1", [&cutoff])?;
    tx.execute(
        "DELETE FROM login_attempts WHERE at < ?1",
        [stamp - 30 * 86400],
    )?;
    tx.commit()?;
    Ok(covered)
}

pub async fn last_sample(store: &Store) -> Option<i64> {
    store
        .run(|c| {
            c.query_row("SELECT sampled_at FROM snapshot WHERE id = 1", [], |r| {
                r.get(0)
            })
        })
        .await
        .ok()
}

/// Ringkasan dashboard: snapshot terakhir + riwayat harian `days` hari.
pub fn dashboard(
    conn: &rusqlite::Connection,
    days: i64,
    account: Option<&str>,
) -> rusqlite::Result<Value> {
    let today = now_wib().date_naive();
    let start = (today - Duration::days(days - 1)).to_string();
    let snapshot: Option<(i64, String)> = conn
        .query_row(
            "SELECT sampled_at, data FROM snapshot WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let mut coverage = HashMap::new();
    let mut stmt = conn.prepare("SELECT day, seconds FROM coverage WHERE day >= ?1")?;
    for row in stmt.query_map([&start], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (d, s) = row?;
        coverage.insert(d, s);
    }
    let mut totals = HashMap::new();
    let sql = if account.is_some() {
        "SELECT day, SUM(download), SUM(upload) FROM daily WHERE day >= ?1 AND account = ?2 GROUP BY day"
    } else {
        "SELECT day, SUM(download), SUM(upload) FROM daily WHERE day >= ?1 AND ?2 IS NULL GROUP BY day"
    };
    let mut stmt = conn.prepare(sql)?;
    for row in stmt.query_map((&start, account), |r| {
        Ok((
            r.get::<_, String>(0)?,
            (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?),
        ))
    })? {
        let (d, t) = row?;
        totals.insert(d, t);
    }
    let history: Vec<Value> = (0..days)
        .rev()
        .map(|n| {
            let date = (today - Duration::days(n)).to_string();
            let seconds = coverage.get(&date).copied().unwrap_or(0);
            let (dl, ul) = match totals.get(&date) {
                Some((d, u)) => (json!(d), json!(u)),
                None if seconds > 0 => (json!(0), json!(0)),
                None => (Value::Null, Value::Null),
            };
            json!({ "date": date, "download": dl, "upload": ul, "coverage": seconds, "full": n > 0 && seconds >= 85536 })
        })
        .collect();
    let (sampled_at, mut data) = match snapshot {
        Some((at, text)) => (json!(at), serde_json::from_str(&text).unwrap_or(json!({}))),
        None => (
            Value::Null,
            json!({ "resources": {}, "customers": [], "interfaces": [] }),
        ),
    };
    data["sampled_at"] = sampled_at;
    data["history"] = json!(history);
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<FixedOffset> {
        parse_time(Some(&s.to_string())).unwrap()
    }

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parses_v6_and_v7_times() {
        assert_eq!(t("2026-11-20 10:00:00"), t("nov/20/2026 10:00:00"));
        assert_eq!(
            t("2026-11-20 10:00:00").offset().local_minus_utc(),
            7 * 3600
        );
    }

    #[test]
    fn cycle_start_handles_day_31() {
        let anchor = Some("2026-01-31 00:00:00".to_string());
        // Februari 2027 hanya 28 hari: siklus mulai 28 Feb.
        assert_eq!(
            cycle_start(t("2027-02-28 12:00:00"), anchor.as_ref()).unwrap(),
            t("2027-02-28 00:00:00")
        );
        // Sebelum 28 Feb, siklus masih dari 31 Jan.
        assert_eq!(
            cycle_start(t("2027-02-27 12:00:00"), anchor.as_ref()).unwrap(),
            t("2027-01-31 00:00:00")
        );
        // Maret: kembali ke tanggal 31.
        assert_eq!(
            cycle_start(t("2027-03-30 12:00:00"), anchor.as_ref()).unwrap(),
            t("2027-02-28 00:00:00")
        );
        assert_eq!(
            cycle_start(t("2027-03-31 01:00:00"), anchor.as_ref()).unwrap(),
            t("2027-03-31 00:00:00")
        );
        // Januari melihat Desember tahun lalu.
        let a5 = Some("2026-01-05 08:00:00".to_string());
        assert_eq!(
            cycle_start(t("2027-01-03 00:00:00"), a5.as_ref()).unwrap(),
            t("2026-12-05 08:00:00")
        );
        assert!(cycle_start(t("2025-01-01 00:00:00"), a5.as_ref()).is_none());
    }

    #[test]
    fn splits_across_midnight() {
        let parts = split_by_day(
            1000,
            100,
            t("2026-11-24 23:30:00"),
            t("2026-11-25 00:30:00"),
        );
        assert_eq!(
            parts,
            vec![
                ("2026-11-24".into(), 500, 50),
                ("2026-11-25".into(), 500, 50)
            ]
        );
        let same = split_by_day(7, 3, t("2026-11-25 01:00:00"), t("2026-11-25 02:00:00"));
        assert_eq!(same, vec![("2026-11-25".into(), 7, 3)]);
    }

    fn data() -> CustomerData {
        CustomerData {
            users: vec![
                row(&[("name", "budi")]),
                row(&[("name", "sari"), ("disabled", "true")]),
            ],
            profiles: vec![
                row(&[
                    ("user", "budi"),
                    ("profile", "p-budi"),
                    ("state", "running-active"),
                ]),
                row(&[("user", "sari"), ("profile", "paket"), ("state", "running")]),
                row(&[("user", "lain"), ("profile", "paket"), ("state", "running")]),
            ],
            links: vec![
                row(&[("profile", "p-budi"), ("limitation", "l-budi")]),
                row(&[("profile", "paket"), ("limitation", "l-paket")]),
            ],
            limits: vec![
                row(&[
                    ("name", "l-budi"),
                    ("transfer-limit", "10737418240"),
                    ("reset-counters-start-time", "2026-01-31 00:00:00"),
                    ("reset-counters-interval", "monthly"),
                ]),
                row(&[
                    ("name", "l-paket"),
                    ("download-limit", "5"),
                    ("upload-limit", "5"),
                ]),
            ],
            active: vec![row(&[
                ("name", "budi"),
                ("service", "pppoe"),
                ("address", "10.0.0.2"),
            ])],
        }
    }

    #[test]
    fn customers_and_usage() {
        let mut cs = customers_from(&data());
        assert_eq!(cs[0].name, "budi");
        assert!(cs[0].editable && cs[0].online);
        assert_eq!(cs[0].limit, Some(10 * GIB));
        // Profil dipakai bersama dan dua batas aktif: tidak bisa di-top-up.
        assert!(!cs[1].editable && cs[1].field.is_none() && cs[1].disabled);
        let now = t("2026-11-25 12:00:00");
        let mut usage = UsageAccumulator::new(&cs, now);
        let s = |started: &str, ended: Option<&str>, dl: u64| SessionRow {
            key: started.into(),
            user: "budi".into(),
            started: t(started),
            ended: ended.map(t),
            download: dl,
            upload: 1,
        };
        usage.add(&s("2026-11-01 00:00:00", Some("2026-11-02 00:00:00"), GIB));
        usage.add(&s("2026-10-20 00:00:00", Some("2026-10-21 00:00:00"), GIB)); // siklus lalu
        usage.apply(&mut cs);
        assert_eq!(cs[0].used, Some(GIB + 1));
        assert_eq!(cs[0].remaining, Some(9 * GIB - 1));
        assert_eq!(cs[0].cycle_start.as_deref(), Some("2026-10-31"));
        assert!(!cs[0].approximate);
    }

    fn sample(now: &str, sessions: Vec<SessionRow>) -> Sample {
        Sample {
            resources: Row::new(),
            customers: vec![],
            interfaces: vec![],
            sessions,
            now: t(now),
        }
    }

    fn sess(dl: u64, ended: Option<&str>) -> SessionRow {
        SessionRow {
            key: "budi|1".into(),
            user: "budi".into(),
            started: t("2026-11-24 20:00:00"),
            ended: ended.map(t),
            download: dl,
            upload: 0,
        }
    }

    fn daily(conn: &rusqlite::Connection) -> Vec<(String, i64)> {
        let mut s = conn
            .prepare("SELECT day, download FROM daily ORDER BY day")
            .unwrap();
        s.query_map([], |r| Ok((r.get(0).unwrap(), r.get(1).unwrap())))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[tokio::test]
    async fn records_deltas_without_losing_gaps() {
        let store = Store::open_in_memory().unwrap();
        store
            .run(|c| {
                // Pertama kali: hanya patokan.
                record(c, &sample("2026-11-24 23:00:00", vec![sess(500, None)]), 60)?;
                assert!(daily(c).is_empty());
                // Jeda 2 jam (panel mati) melewati tengah malam: tetap terhitung, dibagi per hari.
                record(
                    c,
                    &sample("2026-11-25 01:00:00", vec![sess(1700, None)]),
                    60,
                )?;
                assert_eq!(
                    daily(c),
                    vec![("2026-11-24".into(), 600), ("2026-11-25".into(), 600)]
                );
                // Sesi selesai di antara dua sampel: sisa trafiknya tetap masuk.
                record(
                    c,
                    &sample(
                        "2026-11-25 01:01:00",
                        vec![sess(1800, Some("2026-11-25 01:00:30"))],
                    ),
                    60,
                )?;
                assert_eq!(
                    daily(c),
                    vec![("2026-11-24".into(), 600), ("2026-11-25".into(), 700)]
                );
                let left: i64 =
                    c.query_row("SELECT count(*) FROM session_counters", [], |r| r.get(0))?;
                assert_eq!(left, 0);
                // Cakupan hanya untuk jeda normal (60 detik), bukan jeda 2 jam.
                let cov: i64 =
                    c.query_row("SELECT SUM(seconds) FROM coverage", [], |r| r.get(0))?;
                assert_eq!(cov, 60);
                Ok(())
            })
            .await
            .unwrap();
    }
}
