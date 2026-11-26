//! Jembatan antara loop agen (`panel-agent`) dan aplikasi: database, router, dan SSE.

use std::collections::HashMap;
use std::sync::Mutex;

use panel_agent::ai::{Endpoint, Message, Protocol, Thinking};
use panel_agent::design::{DesignSpec, default_spec};
use panel_agent::router_tools::{self, RouterApi};
use panel_agent::runner::{Host, Proposal};
use panel_core::crypto::{Cipher, random_token};
use panel_core::policy::Permission;
use panel_core::policy::command::Command as PolicyCommand;
use rusqlite::OptionalExtension;
use serde_json::{Value as Json, json};
use tokio_util::sync::CancellationToken;

use crate::http::Shared;
use crate::routeros::Command;
use crate::store::now;
use crate::telemetry;

/// Giliran agen yang sedang berjalan, untuk tombol Stop.
#[derive(Default)]
pub struct Runs {
    inner: Mutex<HashMap<String, (String, CancellationToken)>>,
}

impl Runs {
    /// Mulai giliran baru; None bila agen masih mengerjakan giliran lain.
    pub fn start(&self, conversation: &str) -> Option<(String, CancellationToken)> {
        let mut map = self.inner.lock().unwrap();
        if !map.is_empty() {
            return None;
        }
        let id = random_token();
        let token = CancellationToken::new();
        map.insert(id.clone(), (conversation.to_string(), token.clone()));
        Some((id, token))
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .get(id)
            .map(|(_, t)| t.cancel())
            .is_some()
    }

    pub fn for_conversation(&self, conversation: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|(_, (c, _))| c == conversation)
            .map(|(id, _)| id.clone())
    }

    /// Percakapan yang sedang dikerjakan agen (paling banyak satu giliran berjalan).
    pub fn current_conversation(&self) -> Option<String> {
        self.inner.lock().unwrap().values().next().map(|(c, _)| c.clone())
    }

    pub fn finish(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
    }
}

pub struct AppHost {
    pub state: Shared,
    pub cipher: Cipher,
}

fn e(err: impl std::fmt::Display) -> String {
    err.to_string()
}

/// Pastikan ada versi desain aktif (desain bawaan) dan kembalikan (id, spec) draft terbaru atau versi aktif.
pub fn current_draft(c: &mut rusqlite::Connection) -> rusqlite::Result<(String, String)> {
    let latest: Option<(String, String)> = c
        .query_row(
            "SELECT id, spec FROM design_versions WHERE status IN ('draft', 'active')
             ORDER BY CASE status WHEN 'draft' THEN 0 ELSE 1 END, created_at DESC, rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some(v) = latest {
        return Ok(v);
    }
    let spec = serde_json::to_string(&default_spec()).expect("spec bawaan");
    c.execute(
        "INSERT INTO design_versions (id, parent_id, spec, custom_css, source, status, created_at)
         VALUES ('default', NULL, ?1, '', 'default', 'active', ?2)",
        (&spec, now()),
    )?;
    Ok(("default".into(), spec))
}

impl Host for AppHost {
    fn publish(&self, kind: &'static str, data: Json) {
        self.state.hub.publish(kind, data);
    }

    async fn endpoint(&self) -> Result<Endpoint, String> {
        let raw: Option<String> = self
            .state
            .store
            .run(|c| {
                c.query_row("SELECT value FROM settings WHERE key = 'ai'", [], |r| {
                    r.get(0)
                })
                .optional()
            })
            .await
            .map_err(e)?;
        let v: Json = serde_json::from_str(
            &raw.ok_or("Simpan endpoint, model, dan API key di Pengaturan AI.")?,
        )
        .map_err(e)?;
        let api_key = self
            .cipher
            .decrypt(v["encrypted_key"].as_str().unwrap_or_default())
            .map_err(|_| "Simpan ulang API key di Pengaturan AI.".to_string())?;
        let protocol = if v["protocol"] == "anthropic" {
            Protocol::Anthropic
        } else {
            Protocol::OpenAi
        };
        Ok(Endpoint {
            protocol,
            base_url: v["url"].as_str().unwrap_or_default().to_string(),
            model: v["model"].as_str().unwrap_or_default().to_string(),
            api_key,
            thinking: Thinking::parse(v["thinking"].as_str().unwrap_or_default()),
        })
    }

    async fn load_messages(&self, conversation: &str) -> Result<Vec<Message>, String> {
        let conv = conversation.to_string();
        let rows: Vec<String> = self
            .state
            .store
            .run(move |c| {
                let mut s = c.prepare("SELECT content FROM chat_messages WHERE conversation_id = ?1 ORDER BY created_at, rowid")?;
                s.query_map([conv], |r| r.get(0))?.collect()
            })
            .await
            .map_err(e)?;
        Ok(rows
            .iter()
            .filter_map(|t| serde_json::from_str(t).ok())
            .collect())
    }

    async fn append_message(
        &self,
        conversation: &str,
        message: &Message,
        design_version: Option<&str>,
    ) -> Result<(), String> {
        let (conv, dv) = (conversation.to_string(), design_version.map(str::to_owned));
        let role = serde_json::to_value(message.role)
            .map_err(e)?
            .as_str()
            .unwrap_or("user")
            .to_string();
        let content = serde_json::to_string(message).map_err(e)?;
        self.state
            .store
            .run(move |c| {
                let at = now();
                c.execute(
                    "INSERT INTO chat_messages (id, conversation_id, role, content, design_version_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    (random_token(), &conv, role, content, dv, at),
                )?;
                c.execute("UPDATE conversations SET updated_at = ?1 WHERE id = ?2", (at, &conv))
            })
            .await
            .map(|_| ())
            .map_err(e)
    }

    async fn draft_design(&self) -> Result<(String, DesignSpec), String> {
        let (id, spec) = self.state.store.run(current_draft).await.map_err(e)?;
        Ok((id, serde_json::from_str(&spec).map_err(e)?))
    }

    async fn design_version(&self, id: &str) -> Result<DesignSpec, String> {
        let id = id.to_string();
        let spec: Option<String> = self
            .state
            .store
            .run(move |c| {
                c.query_row(
                    "SELECT spec FROM design_versions WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
            })
            .await
            .map_err(e)?;
        serde_json::from_str(&spec.ok_or("Versi desain tidak ditemukan.")?).map_err(e)
    }

    async fn save_draft(&self, parent: &str, spec: &DesignSpec) -> Result<String, String> {
        let id = random_token();
        let (parent, text, css, vid) = (
            parent.to_string(),
            serde_json::to_string(spec).map_err(e)?,
            spec.custom_css.clone(),
            id.clone(),
        );
        self.state
            .store
            .run(move |c| {
                let tx = c.transaction()?;
                tx.execute("UPDATE design_versions SET status = 'archived' WHERE status = 'draft'", [])?;
                tx.execute(
                    "INSERT INTO design_versions (id, parent_id, spec, custom_css, source, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, 'agent', 'draft', ?5)",
                    (&vid, &parent, &text, &css, now()),
                )?;
                tx.commit()
            })
            .await
            .map_err(e)?;
        Ok(id)
    }

    async fn recall(&self, query: &str) -> Vec<String> {
        search_memories(&self.state, query, 5)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| m["text"].as_str().unwrap_or("").to_string())
            .collect()
    }

    async fn run_tool(&self, name: &str, input: &Json) -> Result<Json, String> {
        run_tool(&self.state, name, input).await
    }

    async fn propose(&self, p: Proposal) -> Result<String, String> {
        let id = random_token();
        let mut perm = p.permission;
        let mut label = proposal_label(&p.tool, &p.input);
        let mut preview = p.preview.clone();
        if p.tool == "run_action_button" {
            // Izin dan teks konfirmasi mengikuti tombol tersimpan (kritis = ketik label tombol).
            let a = crate::actions::load(&self.state, p.input["id"].as_str().unwrap_or_default()).await?;
            perm = perm.max(crate::actions::permission_of(&a.risk));
            label = a.label.clone();
            preview = a.to_json()["preview"].as_str().map(str::to_owned);
        }
        let permission = serde_json::to_value(perm)
            .map_err(e)?
            .as_str()
            .unwrap_or("approve")
            .to_string();
        let tool_use_id = serde_json::to_value(&p)
            .ok()
            .and_then(|v| v["tool_use_id"].as_str().map(str::to_owned));
        let (pid, input) = (id.clone(), p.input.to_string());
        let (conv, tool) = (p.conversation_id.clone(), p.tool.clone());
        self.state
            .store
            .run(move |c| {
                c.execute(
                    "INSERT INTO agent_proposals (id, conversation_id, tool_use_id, tool, input, permission, preview, label, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9)",
                    (&pid, conv, tool_use_id, tool, input, permission, preview, label, now()),
                )
            })
            .await
            .map_err(e)?;
        if let Ok(card) = crate::agent_api::load_card(&self.state, &id).await {
            self.state
                .hub
                .publish("proposal.updated", json!({ "card": card }));
        }
        Ok(id)
    }
}

/// Cari memori agen dengan FTS5; kata kunci dipisah dan dikutip supaya aman sebagai query FTS.
pub async fn search_memories(state: &Shared, query: &str, limit: i64) -> anyhow::Result<Vec<Json>> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 3)
        .take(12)
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        return Ok(vec![]);
    }
    let fts = terms.join(" OR ");
    state
        .store
        .run(move |c| {
            let mut s = c.prepare(
                "SELECT m.id, m.kind, m.text, m.updated_at FROM agent_memories_fts f
                 JOIN agent_memories m ON m.rowid = f.rowid WHERE agent_memories_fts MATCH ?1 ORDER BY rank LIMIT ?2",
            )?;
            s.query_map((fts, limit), |r| {
                Ok(json!({ "id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?, "text": r.get::<_, String>(2)?, "updated_at": r.get::<_, i64>(3)? }))
            })?
            .collect()
        })
        .await
}

fn str_arg<'a>(input: &'a Json, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())
}

const READ_ONLY_VERBS: &[&str] = &["print", "get"];

/// Alat berizin Free selain alat desain. Alat yang belum diimplementasikan mengembalikan pesan jelas.
pub async fn run_tool(state: &Shared, name: &str, input: &Json) -> Result<Json, String> {
    match name {
        "router_snapshot" => {
            let v = state
                .store
                .run(|c| telemetry::dashboard(c, 1, None))
                .await
                .map_err(e)?;
            Ok(
                json!({ "sampled_at": v["sampled_at"], "resources": v["resources"], "interfaces": v["interfaces"],
                       "customers_total": v["customers"].as_array().map_or(0, Vec::len),
                       "customers_online": v["customers"].as_array().map_or(0, |a| a.iter().filter(|c| c["online"] == true).count()) }),
            )
        }
        "router_query" => {
            let cmd: PolicyCommand = serde_json::from_value(input["command"].clone())
                .map_err(|err| format!("Perintah tidak valid: {err}"))?;
            cmd.validate().map_err(e)?;
            if cmd.permission() != Permission::Free || !READ_ONLY_VERBS.contains(&cmd.verb.as_str())
            {
                return Err(
                    "router_query hanya untuk print/get. Pakai propose_command untuk perubahan."
                        .into(),
                );
            }
            let mut api = Command::new(&cmd.api_path()).map_err(e)?;
            for (k, v) in PolicyCommand::api_pairs(&cmd.find).map_err(e)? {
                api = api.filter(&k, v).map_err(e)?;
            }
            for (k, v) in PolicyCommand::api_pairs(&cmd.args).map_err(e)? {
                api = api.arg(&k, v).map_err(e)?;
            }
            let mut rows = Vec::new();
            let mut total = 0usize;
            state
                .router
                .each(&api, &mut |r| {
                    total += 1;
                    if rows.len() < 200 {
                        rows.push(mask_secrets(r));
                    }
                })
                .await
                .map_err(e)?;
            Ok(json!({ "rows": rows, "total": total, "truncated": total > 200 }))
        }
        "get_traffic" => {
            let days = input["days"].as_i64().unwrap_or(7).clamp(1, 30);
            let account = str_arg(input, "account").map(str::to_owned);
            let v = state
                .store
                .run(move |c| telemetry::dashboard(c, days, account.as_deref()))
                .await
                .map_err(e)?;
            Ok(json!({ "history": v["history"] }))
        }
        "get_logs" => {
            let limit = input["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
            let contains = str_arg(input, "contains").map(str::to_owned);
            let topics: Vec<String> = input["topics"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let cmd = Command::new("/log/print")
                .and_then(|c| c.proplist(&["time", "topics", "message"]))
                .map_err(e)?;
            let mut out = std::collections::VecDeque::new();
            state
                .router
                .each(&cmd, &mut |r| {
                    let msg = r.get("message").map(String::as_str).unwrap_or("");
                    let tps = r.get("topics").map(String::as_str).unwrap_or("");
                    if contains.as_deref().is_none_or(|c| msg.contains(c))
                        && (topics.is_empty()
                            || tps.split(',').any(|t| topics.iter().any(|w| w == t)))
                    {
                        if out.len() == limit {
                            out.pop_front();
                        }
                        out.push_back(r);
                    }
                })
                .await
                .map_err(e)?;
            Ok(json!({ "logs": out }))
        }
        "list_customers" | "customer_detail" => {
            let v = state
                .store
                .run(|c| telemetry::dashboard(c, 1, None))
                .await
                .map_err(e)?;
            let all = v["customers"].as_array().cloned().unwrap_or_default();
            if name == "customer_detail" {
                let account = str_arg(input, "account")
                    .ok_or("account wajib diisi")?
                    .to_string();
                let row = all
                    .into_iter()
                    .find(|c| c["name"] == account.as_str())
                    .ok_or("Pelanggan tidak ditemukan.")?;
                let hist = state
                    .store
                    .run(move |c| telemetry::dashboard(c, 7, Some(&account)))
                    .await
                    .map_err(e)?;
                return Ok(json!({ "customer": row, "history": hist["history"] }));
            }
            let filter = str_arg(input, "filter").unwrap_or("all");
            let limit = input["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize;
            let rows: Vec<Json> = all
                .into_iter()
                .filter(|c| match filter {
                    "online" => c["online"] == true,
                    "offline" => c["online"] == false,
                    "low_quota" => match (c["remaining"].as_u64(), c["limit"].as_u64()) {
                        (Some(r), Some(l)) if l > 0 => r * 5 <= l,
                        _ => false,
                    },
                    _ => true,
                })
                .take(limit)
                .collect();
            Ok(json!({ "customers": rows }))
        }
        "remember" => {
            let kind = str_arg(input, "kind")
                .filter(|k| ["user", "router", "outcome"].contains(k))
                .ok_or("kind tidak valid")?
                .to_string();
            let text: String = str_arg(input, "text")
                .ok_or("text wajib diisi")?
                .chars()
                .take(500)
                .collect();
            let id = random_token();
            let mid = id.clone();
            state
                .store
                .run(move |c| {
                    let at = now();
                    c.execute("INSERT INTO agent_memories (id, kind, text, source, created_at, updated_at) VALUES (?1, ?2, ?3, 'agent', ?4, ?4)", (&mid, kind, text, at))
                })
                .await
                .map_err(e)?;
            Ok(json!({ "id": id, "saved": true }))
        }
        "recall" => Ok(
            json!({ "memories": search_memories(state, str_arg(input, "query").unwrap_or(""), 10).await.map_err(e)? }),
        ),
        "update_memory" | "forget" => {
            let id = str_arg(input, "id").ok_or("id wajib diisi")?.to_string();
            let text: Option<String> =
                str_arg(input, "text").map(|t| t.chars().take(500).collect());
            let forget = name == "forget";
            let n = state
                .store
                .run(move |c| {
                    if forget {
                        c.execute("DELETE FROM agent_memories WHERE id = ?1", [&id])
                    } else {
                        c.execute(
                            "UPDATE agent_memories SET text = ?1, updated_at = ?2 WHERE id = ?3",
                            (text.unwrap_or_default(), now(), &id),
                        )
                    }
                })
                .await
                .map_err(e)?;
            if n == 0 {
                Err("Memori tidak ditemukan.".into())
            } else {
                Ok(json!({ "ok": true }))
            }
        }
        "search_routeros_docs" => {
            let q = str_arg(input, "query").unwrap_or("").to_string();
            let terms: Vec<String> = q
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| t.len() >= 2)
                .map(|t| format!("\"{t}\""))
                .collect();
            if terms.is_empty() {
                return Ok(json!({ "results": [] }));
            }
            let fts = terms.join(" ");
            let rows: Vec<Json> = state
                .store
                .run(move |c| {
                    let mut s = c.prepare("SELECT path, title, snippet(routeros_reference, 2, '', '', '…', 40) FROM routeros_reference WHERE routeros_reference MATCH ?1 ORDER BY rank LIMIT 8")?;
                    s.query_map([fts], |r| Ok(json!({ "path": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?, "excerpt": r.get::<_, String>(2)? })))?.collect()
                })
                .await
                .map_err(e)?;
            Ok(
                json!({ "results": rows, "note": if rows.is_empty() { "Referensi lokal belum diisi atau tidak ada hasil." } else { "" } }),
            )
        }
        "get_command_tree" => {
            let path: Vec<String> = serde_json::from_value(input["path"].clone()).map_err(|_| "path harus daftar teks")?;
            router_tools::command_tree(&ApiBridge(state), &path).await
        }
        "check_command" => {
            let cmd: PolicyCommand = serde_json::from_value(input["command"].clone()).map_err(|err| format!("Perintah tidak valid: {err}"))?;
            router_tools::check_command(&ApiBridge(state), &cmd).await
        }
        "net_diagnose" => {
            let kind = str_arg(input, "kind").ok_or("kind wajib diisi")?;
            let address = str_arg(input, "address").ok_or("address wajib diisi")?;
            router_tools::net_diagnose(&ApiBridge(state), kind, address, input["count"].as_u64()).await
        }
        "get_topology" => router_tools::topology(&ApiBridge(state)).await,
        "audit_router" => {
            let focus: Vec<String> = serde_json::from_value(input["focus"].clone()).unwrap_or_default();
            router_tools::audit(&ApiBridge(state), &focus).await
        }
        "traffic_report" => {
            let days = input["days"].as_i64().unwrap_or(7).clamp(1, 30);
            let since = (telemetry::now_wib().date_naive() - chrono::Duration::days(days - 1)).to_string();
            let top: Vec<Json> = state
                .store
                .run(move |c| {
                    let mut s = c.prepare(
                        "SELECT account, SUM(download), SUM(upload) FROM daily WHERE day >= ?1 GROUP BY account
                         ORDER BY SUM(download) + SUM(upload) DESC LIMIT 10",
                    )?;
                    s.query_map([since], |r| Ok(json!({ "account": r.get::<_, String>(0)?, "download": r.get::<_, i64>(1)?, "upload": r.get::<_, i64>(2)? })))?
                        .collect()
                })
                .await
                .map_err(e)?;
            let hist = state.store.run(move |c| telemetry::dashboard(c, days, None)).await.map_err(e)?;
            Ok(json!({ "days": days, "daily": hist["history"], "top_accounts": top,
                       "note": "Data per jam belum dicatat; jam sibuk belum tersedia." }))
        }
        "check_update" => {
            let before = crate::http::read_update_status().await.map(|u| u["checked_at"].clone());
            crate::http::request_update("check").await.map_err(|(_, m)| m)?;
            let mut status = None;
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let now = crate::http::read_update_status().await;
                if now.as_ref().map(|u| u["checked_at"].clone()) != before {
                    status = now;
                    break;
                }
            }
            let u = status.ok_or("Pembaru belum menjawab dalam 40 detik; coba lagi nanti.")?;
            let latest = u["latest"].as_str().unwrap_or("").trim_start_matches('v').to_string();
            Ok(json!({ "installed": crate::VERSION, "latest": u["latest"], "check": u["check"],
                       "update_available": !latest.is_empty() && latest != crate::VERSION,
                       "auto_update": u["auto"], "last_update": { "tag": u["last_tag"], "result": u["last_result"], "at": u["last_at"] } }))
        }
        "snapshot_diff" => {
            let hours = input["since_hours"].as_i64().unwrap_or(24);
            crate::snapshots::since(state, hours).await.map_err(e)
        }
        "create_recommendation" => {
            let kind = str_arg(input, "kind").filter(|k| ["performance", "security", "topology", "layout"].contains(k)).ok_or("kind tidak valid")?.to_string();
            let title = str_arg(input, "title").ok_or("title wajib diisi")?;
            let finding = str_arg(input, "finding").ok_or("finding wajib diisi")?;
            if let Some(cmd) = input["fix"].get("command") {
                let cmd: PolicyCommand = serde_json::from_value(cmd.clone()).map_err(|err| format!("fix.command tidak valid: {err}"))?;
                cmd.validate().map_err(e)?;
                if !cmd.placeholders().is_empty() {
                    return Err("fix.command tidak boleh berisi placeholder.".into());
                }
            }
            let text = format!("{} — {}", title.chars().take(80).collect::<String>(), finding.chars().take(600).collect::<String>());
            let evidence = json!({ "title": title, "evidence": input["evidence"], "impact": input["impact"], "fix": input["fix"] }).to_string();
            let id = random_token();
            let id2 = id.clone();
            state
                .store
                .run(move |c| c.execute(
                    "INSERT INTO recommendations (id, kind, finding, evidence, status, action_id, created_at) VALUES (?1, ?2, ?3, ?4, 'open', NULL, ?5)",
                    (id2, kind, text, evidence, now()),
                ))
                .await
                .map_err(e)?;
            state.hub.publish("recommendations.updated", json!({ "id": id }));
            Ok(json!({ "ok": true, "id": id, "note": "Kartu saran tampil di blok Saran agen; admin bisa menerapkan atau mengabaikannya." }))
        }
        "show_card" => {
            let text = input["data"].to_string();
            if text.len() > 20_000 {
                return Err("Data kartu terlalu besar (maks 20 KB).".into());
            }
            Ok(json!({ "shown": true }))
        }
        "set_conversation_title" => {
            let title: String = str_arg(input, "title").ok_or("title wajib diisi")?.chars().take(60).collect();
            let conv = state.runs.current_conversation().ok_or("Tidak ada percakapan aktif.")?;
            let (c2, t2) = (conv.clone(), title.clone());
            state
                .store
                .run(move |c| c.execute("UPDATE conversations SET title = ?1 WHERE id = ?2", (t2, c2)))
                .await
                .map_err(e)?;
            state.hub.publish("conversation.updated", json!({ "id": conv, "title": title }));
            Ok(json!({ "ok": true, "title": title }))
        }
        _ => Err(format!("Alat `{name}` belum tersedia di versi panel ini.")),
    }
}

/// Adaptor klien RouterOS ke trait `RouterApi` milik crate agen.
pub struct ApiBridge<'a>(pub &'a Shared);

impl RouterApi for ApiBridge<'_> {
    async fn call(&self, path: &str, args: &[(&str, String)], filters: &[(&str, String)]) -> Result<Vec<crate::routeros::Row>, String> {
        let mut cmd = Command::new(path).map_err(e)?;
        for (k, v) in args {
            cmd = cmd.arg(k, v).map_err(e)?;
        }
        for (k, v) in filters {
            cmd = cmd.filter(k, v).map_err(e)?;
        }
        self.0.router.call(&cmd).await.map(|rows| rows.into_iter().map(mask_secrets).collect()).map_err(e)
    }
}

/// Teks kartu; untuk tindakan kritis inilah yang harus diketik admin.
pub fn proposal_label(tool: &str, input: &Json) -> String {
    let target = ["account", "name", "label", "user"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .unwrap_or("");
    let base = tool.replace('_', " ");
    if target.is_empty() {
        base
    } else {
        format!("{base} {target}")
    }
}

fn mask_secrets(mut row: crate::routeros::Row) -> crate::routeros::Row {
    for (k, v) in row.iter_mut() {
        if [
            "password",
            "secret",
            "passphrase",
            "key",
            "private-key",
            "authentication-key",
            "wpa-pre-shared-key",
            "wpa2-pre-shared-key",
        ]
        .iter()
        .any(|s| k.contains(s))
        {
            *v = "***".into();
        }
    }
    row
}
