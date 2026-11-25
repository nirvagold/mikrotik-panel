//! Klien AI streaming untuk endpoint Anthropic-compatible dan OpenAI-compatible
//! (dipilih admin di Pengaturan AI). Raw HTTP + SSE, bisa dibatalkan kapan saja.
//!
//! Keamanan endpoint: hanya HTTPS port 443 tanpa kredensial/query di URL, dan
//! semua alamat hasil DNS harus publik. Koneksi dipaku ke alamat yang sudah
//! dicek (mencegah DNS rebinding ke jaringan lokal/router).
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use tokio_util::sync::CancellationToken;

use crate::tools::ToolDef;

const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
pub const DEFAULT_MAX_TOKENS: u32 = 32_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[serde(rename = "openai")]
    OpenAi,
    Anthropic,
}

#[derive(Clone)]
pub struct Endpoint {
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    ToolUse { id: String, name: String, input: Json },
    ToolResult { tool_use_id: String, content: String, #[serde(default)] is_error: bool },
    /// Blok asli penyedia (mis. `thinking` Anthropic) yang wajib dikirim balik apa adanya.
    Opaque { block: Json },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refusal,
    PauseTurn,
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    TextDelta(String),
    ToolStart { id: String, name: String },
    /// Model sedang berpikir (untuk indikator di UI; isinya tidak ditampilkan).
    Thinking,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub parts: Vec<Part>,
    pub stop: StopReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    Cancelled,
    Endpoint(String),
    Http(String),
    Protocol(String),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Cancelled => f.write_str("Dihentikan oleh admin."),
            AiError::Endpoint(m) | AiError::Http(m) | AiError::Protocol(m) => f.write_str(m),
        }
    }
}
impl std::error::Error for AiError {}

/// Alamat IP yang boleh dijangkau (bukan loopback, privat, link-local, CGNAT, dll.).
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_broadcast()
                || v4.is_documentation() || v4.is_unspecified() || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))   // CGNAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)      // IETF
                || (o[0] == 198 && (18..20).contains(&o[1]))    // benchmarking
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00   // unique local
                || (s[0] & 0xffc0) == 0xfe80   // link local
                || s[0] == 0x2001 && s[1] == 0x0db8 // dokumentasi
                || s[0] == 0x64 && s[1] == 0xff9b)  // NAT64
        }
    }
}

/// Validasi URL endpoint dan resolusi DNS. Mengembalikan host + alamat terpaku.
pub async fn check_endpoint(base_url: &str) -> Result<(url::Url, String, Vec<SocketAddr>), AiError> {
    let bad = |m: &str| AiError::Endpoint(m.to_string());
    let url = url::Url::parse(base_url).map_err(|_| bad("Base URL tidak valid."))?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some()
        || !matches!(url.port(), None | Some(443))
    {
        return Err(bad("Gunakan base URL HTTPS tanpa parameter, kredensial, atau port khusus."));
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_string(),
        _ => return Err(bad("Base URL harus memakai nama domain.")),
    };
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 443))
        .await
        .map_err(|_| bad("Nama domain endpoint AI tidak dapat di-resolve."))?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
        return Err(bad("Endpoint AI harus memiliki alamat publik."));
    }
    Ok((url, host, addrs))
}

fn http_client(pin: Option<(&str, &[SocketAddr])>) -> Result<reqwest::Client, AiError> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .user_agent("mikrotik-panel-agent");
    if let Some((host, addrs)) = pin {
        builder = builder.resolve_to_addrs(host, addrs);
    }
    builder.build().map_err(|_| AiError::Endpoint("Gagal menyiapkan klien HTTP.".into()))
}

fn is_first_party(url: &url::Url) -> bool {
    url.host_str() == Some("api.anthropic.com")
}

pub fn build_body(endpoint: &Endpoint, first_party: bool, system: &str, messages: &[Message], tools: &[ToolDef], max_tokens: u32) -> Json {
    match endpoint.protocol {
        Protocol::Anthropic => {
            let mut tool_defs: Vec<Json> = tools.iter().map(|t| {
                let mut def = json!({"name": t.name, "description": t.description, "input_schema": t.input_schema});
                if first_party {
                    def["eager_input_streaming"] = json!(true);
                }
                def
            }).collect();
            let mut system_block = json!({"type": "text", "text": system});
            if first_party {
                // Prefix stabil (alat + system) di-cache; percakapan berubah di belakangnya.
                if let Some(last) = tool_defs.last_mut() {
                    last["cache_control"] = json!({"type": "ephemeral"});
                }
                system_block["cache_control"] = json!({"type": "ephemeral"});
            }
            let msgs: Vec<Json> = messages.iter().map(|m| json!({
                "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
                "content": m.parts.iter().map(|p| match p {
                    Part::Text { text } => json!({"type": "text", "text": text}),
                    Part::ToolUse { id, name, input } => json!({"type": "tool_use", "id": id, "name": name, "input": input}),
                    Part::ToolResult { tool_use_id, content, is_error } => json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error}),
                    Part::Opaque { block } => block.clone(),
                }).collect::<Vec<_>>(),
            })).collect();
            let mut body = json!({"model": endpoint.model, "max_tokens": max_tokens, "stream": true,
                "system": [system_block], "messages": msgs});
            if !tool_defs.is_empty() {
                body["tools"] = Json::Array(tool_defs);
            }
            body
        }
        Protocol::OpenAi => {
            let mut msgs = vec![json!({"role": "system", "content": system})];
            for m in messages {
                match m.role {
                    Role::User => {
                        let text: Vec<&str> = m.parts.iter().filter_map(|p| match p { Part::Text { text } => Some(text.as_str()), _ => None }).collect();
                        for p in &m.parts {
                            if let Part::ToolResult { tool_use_id, content, is_error } = p {
                                let content = if *is_error { format!("ERROR: {content}") } else { content.clone() };
                                msgs.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": content}));
                            }
                        }
                        if !text.is_empty() {
                            msgs.push(json!({"role": "user", "content": text.join("\n")}));
                        }
                    }
                    Role::Assistant => {
                        let text: String = m.parts.iter().filter_map(|p| match p { Part::Text { text } => Some(text.as_str()), _ => None }).collect::<Vec<_>>().join("");
                        let calls: Vec<Json> = m.parts.iter().filter_map(|p| match p {
                            Part::ToolUse { id, name, input } => Some(json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}})),
                            _ => None,
                        }).collect();
                        let mut msg = json!({"role": "assistant", "content": if text.is_empty() { Json::Null } else { json!(text) }});
                        if !calls.is_empty() {
                            msg["tool_calls"] = Json::Array(calls);
                        }
                        msgs.push(msg);
                    }
                }
            }
            let mut body = json!({"model": endpoint.model, "max_tokens": max_tokens, "stream": true, "messages": msgs});
            if !tools.is_empty() {
                body["tools"] = Json::Array(tools.iter().map(|t| json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.input_schema}})).collect());
            }
            body
        }
    }
}

/// Satu giliran model, di-stream. `on_event` dipanggil untuk tiap potongan teks.
pub async fn stream_turn(
    endpoint: &Endpoint,
    system: &str,
    messages: &[Message],
    tools: &[ToolDef],
    cancel: &CancellationToken,
    on_event: impl FnMut(Event),
) -> Result<Reply, AiError> {
    #[cfg(test)]
    if endpoint.base_url.starts_with("http://127.0.0.1:") {
        let url = url::Url::parse(&endpoint.base_url).expect("test url");
        let body = build_body(endpoint, false, system, messages, tools, DEFAULT_MAX_TOKENS);
        return send(&http_client(None)?, &url, endpoint, &body, cancel, on_event).await;
    }
    let (url, host, addrs) = check_endpoint(&endpoint.base_url).await?;
    let client = http_client(Some((&host, &addrs)))?;
    let body = build_body(endpoint, is_first_party(&url), system, messages, tools, DEFAULT_MAX_TOKENS);
    send(&client, &url, endpoint, &body, cancel, on_event).await
}

async fn send(
    client: &reqwest::Client,
    base: &url::Url,
    endpoint: &Endpoint,
    body: &Json,
    cancel: &CancellationToken,
    mut on_event: impl FnMut(Event),
) -> Result<Reply, AiError> {
    let redact = |text: String| if endpoint.api_key.len() >= 8 { text.replace(&endpoint.api_key, "[API key]") } else { text };
    let path = match endpoint.protocol { Protocol::Anthropic => "/messages", Protocol::OpenAi => "/chat/completions" };
    let target = format!("{}{}", base.as_str().trim_end_matches('/'), path);
    let mut request = client.post(&target).json(body).header("accept", "text/event-stream");
    request = match endpoint.protocol {
        Protocol::Anthropic => request.header("x-api-key", &endpoint.api_key).header("anthropic-version", "2023-06-01"),
        Protocol::OpenAi => request.bearer_auth(&endpoint.api_key),
    };
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(AiError::Cancelled),
        r = request.send() => r.map_err(|e| AiError::Http(redact(format!("Koneksi ke endpoint AI gagal: {}", e.without_url()))))?,
    };
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<Json>(&text).ok()
            .and_then(|j| j.pointer("/error/message").and_then(Json::as_str).map(str::to_owned))
            .unwrap_or_default();
        let detail: String = detail.chars().take(300).collect();
        return Err(AiError::Http(redact(format!("Endpoint AI merespons HTTP {}. {}", status.as_u16(), detail).trim().to_string())));
    }
    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut acc = Accumulator::new(endpoint.protocol);
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            c = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => c,
        };
        let chunk = match chunk {
            Err(_) => return Err(AiError::Http("Endpoint AI berhenti merespons.".into())),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(AiError::Http(redact(format!("Stream terputus: {}", e.without_url())))),
            Ok(Some(Ok(bytes))) => bytes,
        };
        total += chunk.len();
        if total > MAX_STREAM_BYTES {
            return Err(AiError::Protocol("Jawaban endpoint AI melebihi batas ukuran.".into()));
        }
        buffer.extend_from_slice(&chunk);
        while let Some(event) = take_event(&mut buffer) {
            if acc.feed(&event, &mut on_event)? {
                return acc.finish().map(|r| redact_reply(r, &endpoint.api_key));
            }
        }
    }
    if let Some(event) = take_event_eof(&mut buffer) {
        acc.feed(&event, &mut on_event)?;
    }
    acc.finish().map(|r| redact_reply(r, &endpoint.api_key))
}

fn redact_reply(mut reply: Reply, key: &str) -> Reply {
    if key.len() < 8 {
        return reply;
    }
    for part in &mut reply.parts {
        if let Part::Text { text } = part {
            *text = text.replace(key, "[API key]");
        }
    }
    reply
}

/// Ambil satu event SSE lengkap (dipisah baris kosong) dari buffer: isi `data:`.
fn take_event(buffer: &mut Vec<u8>) -> Option<String> {
    let pos = (0..buffer.len()).find(|&i| buffer[i..].starts_with(b"\n\n") || buffer[i..].starts_with(b"\r\n\r\n"))?;
    let sep = if buffer[pos..].starts_with(b"\r\n\r\n") { 4 } else { 2 };
    let raw: Vec<u8> = buffer.drain(..pos + sep).collect();
    Some(data_lines(&raw))
}

fn take_event_eof(buffer: &mut Vec<u8>) -> Option<String> {
    if buffer.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let raw = std::mem::take(buffer);
    Some(data_lines(&raw))
}

fn data_lines(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .lines()
        .filter_map(|l| l.strip_prefix("data:").map(|d| d.strip_prefix(' ').unwrap_or(d)))
        .collect::<Vec<_>>()
        .join("\n")
}

enum Block {
    Text(String),
    Tool { id: String, name: String, json: String },
    Thinking { thinking: String, signature: String },
    Raw(Json),
}

struct Accumulator {
    protocol: Protocol,
    blocks: Vec<(usize, Block)>,
    stop: Option<StopReason>,
    // OpenAI: tool call per index
    oa_text: String,
    oa_tools: Vec<(usize, String, String, String)>,
}

impl Accumulator {
    fn new(protocol: Protocol) -> Self {
        Self { protocol, blocks: Vec::new(), stop: None, oa_text: String::new(), oa_tools: Vec::new() }
    }

    fn block(&mut self, index: usize) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|(i, _)| *i == index).map(|(_, b)| b)
    }

    /// Mengembalikan true bila stream selesai.
    fn feed(&mut self, data: &str, on_event: &mut impl FnMut(Event)) -> Result<bool, AiError> {
        let data = data.trim();
        if data.is_empty() {
            return Ok(false);
        }
        if data == "[DONE]" {
            return Ok(true);
        }
        let ev: Json = serde_json::from_str(data).map_err(|_| AiError::Protocol("Format stream endpoint AI tidak dikenali.".into()))?;
        match self.protocol {
            Protocol::Anthropic => self.feed_anthropic(&ev, on_event),
            Protocol::OpenAi => self.feed_openai(&ev, on_event),
        }
    }

    fn feed_anthropic(&mut self, ev: &Json, on_event: &mut impl FnMut(Event)) -> Result<bool, AiError> {
        let index = ev["index"].as_u64().unwrap_or(0) as usize;
        match ev["type"].as_str().unwrap_or("") {
            "content_block_start" => {
                let cb = &ev["content_block"];
                let block = match cb["type"].as_str().unwrap_or("") {
                    "text" => Block::Text(cb["text"].as_str().unwrap_or("").to_string()),
                    "tool_use" => {
                        let id = cb["id"].as_str().unwrap_or("").to_string();
                        let name = cb["name"].as_str().unwrap_or("").to_string();
                        on_event(Event::ToolStart { id: id.clone(), name: name.clone() });
                        Block::Tool { id, name, json: String::new() }
                    }
                    "thinking" => {
                        on_event(Event::Thinking);
                        Block::Thinking { thinking: cb["thinking"].as_str().unwrap_or("").to_string(), signature: cb["signature"].as_str().unwrap_or("").to_string() }
                    }
                    _ => Block::Raw(cb.clone()),
                };
                self.blocks.push((index, block));
            }
            "content_block_delta" => {
                let delta = &ev["delta"];
                match (delta["type"].as_str().unwrap_or(""), self.block(index)) {
                    ("text_delta", Some(Block::Text(text))) => {
                        let piece = delta["text"].as_str().unwrap_or("");
                        text.push_str(piece);
                        on_event(Event::TextDelta(piece.to_string()));
                    }
                    ("input_json_delta", Some(Block::Tool { json, .. })) => json.push_str(delta["partial_json"].as_str().unwrap_or("")),
                    ("thinking_delta", Some(Block::Thinking { thinking, .. })) => thinking.push_str(delta["thinking"].as_str().unwrap_or("")),
                    ("signature_delta", Some(Block::Thinking { signature, .. })) => signature.push_str(delta["signature"].as_str().unwrap_or("")),
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = ev["delta"]["stop_reason"].as_str() {
                    self.stop = Some(match reason {
                        "end_turn" | "stop_sequence" => StopReason::EndTurn,
                        "tool_use" => StopReason::ToolUse,
                        "max_tokens" => StopReason::MaxTokens,
                        "refusal" => StopReason::Refusal,
                        "pause_turn" => StopReason::PauseTurn,
                        other => StopReason::Other(other.to_string()),
                    });
                }
            }
            "message_stop" => return Ok(true),
            "error" => {
                let msg = ev["error"]["message"].as_str().unwrap_or("kesalahan tidak dikenal");
                return Err(AiError::Http(format!("Endpoint AI mengirim error: {}", msg.chars().take(300).collect::<String>())));
            }
            _ => {}
        }
        Ok(false)
    }

    fn feed_openai(&mut self, ev: &Json, on_event: &mut impl FnMut(Event)) -> Result<bool, AiError> {
        if let Some(msg) = ev.pointer("/error/message").and_then(Json::as_str) {
            return Err(AiError::Http(format!("Endpoint AI mengirim error: {}", msg.chars().take(300).collect::<String>())));
        }
        let Some(choice) = ev["choices"].get(0) else { return Ok(false) };
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                self.oa_text.push_str(text);
                on_event(Event::TextDelta(text.to_string()));
            }
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let index = call["index"].as_u64().unwrap_or(0) as usize;
                if !self.oa_tools.iter().any(|t| t.0 == index) {
                    self.oa_tools.push((index, String::new(), String::new(), String::new()));
                }
                let entry = self.oa_tools.iter_mut().find(|t| t.0 == index).expect("inserted");
                if let Some(id) = call["id"].as_str() { entry.1.push_str(id); }
                if let Some(name) = call.pointer("/function/name").and_then(Json::as_str) {
                    entry.2.push_str(name);
                    on_event(Event::ToolStart { id: entry.1.clone(), name: entry.2.clone() });
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(Json::as_str) { entry.3.push_str(args); }
            }
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.stop = Some(match reason {
                "stop" => StopReason::EndTurn,
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "content_filter" => StopReason::Refusal,
                other => StopReason::Other(other.to_string()),
            });
        }
        Ok(false)
    }

    fn finish(self) -> Result<Reply, AiError> {
        let parse_input = |raw: &str| -> Json {
            if raw.trim().is_empty() {
                json!({})
            } else {
                // JSON tidak lengkap/invalid dikirim sebagai penanda; loop agen membalas error.
                serde_json::from_str(raw).unwrap_or_else(|_| json!({"_invalid_json": raw.chars().take(200).collect::<String>()}))
            }
        };
        let mut parts = Vec::new();
        match self.protocol {
            Protocol::Anthropic => {
                let mut blocks = self.blocks;
                blocks.sort_by_key(|(i, _)| *i);
                for (_, block) in blocks {
                    parts.push(match block {
                        Block::Text(text) if text.is_empty() => continue,
                        Block::Text(text) => Part::Text { text },
                        Block::Tool { id, name, json } => Part::ToolUse { id, name, input: parse_input(&json) },
                        Block::Thinking { thinking, signature } => Part::Opaque { block: json!({"type": "thinking", "thinking": thinking, "signature": signature}) },
                        Block::Raw(block) => Part::Opaque { block },
                    });
                }
            }
            Protocol::OpenAi => {
                if !self.oa_text.is_empty() {
                    parts.push(Part::Text { text: self.oa_text });
                }
                let mut tools = self.oa_tools;
                tools.sort_by_key(|t| t.0);
                for (i, id, name, args) in tools {
                    let id = if id.is_empty() { format!("call_{i}") } else { id };
                    parts.push(Part::ToolUse { id, name, input: parse_input(&args) });
                }
            }
        }
        let has_tools = parts.iter().any(|p| matches!(p, Part::ToolUse { .. }));
        let stop = match self.stop {
            Some(stop) => stop,
            None if has_tools => StopReason::ToolUse,
            None => return Err(AiError::Protocol("Stream endpoint AI berakhir sebelum selesai.".into())),
        };
        Ok(Reply { parts, stop })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Server HTTP palsu yang mengirim body SSE apa adanya.
    pub async fn mock_server(body: &'static str, status: u16) -> url::Url {
        mock_sequence(vec![body], status).await
    }

    /// Server palsu yang mengirim body berurutan (body terakhir diulang).
    pub async fn mock_sequence(bodies: Vec<&'static str>, status: u16) -> url::Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut n = 0usize;
            while let Ok((mut socket, _)) = listener.accept().await {
                let body = bodies[n.min(bodies.len() - 1)];
                n += 1;
                let mut buf = vec![0u8; 65536];
                let _ = socket.read(&mut buf).await;
                let head = format!("HTTP/1.1 {status} X\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
            }
        });
        url::Url::parse(&format!("http://{addr}/v1")).unwrap()
    }

    fn endpoint(protocol: Protocol) -> Endpoint {
        Endpoint { protocol, base_url: String::new(), model: "m".into(), api_key: "sk-test-secret-123".into() }
    }

    #[tokio::test]
    async fn anthropic_stream_with_tool_and_thinking() {
        let body = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Saya ubah \"}}\r\n\r\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"aksennya. sk-test-secret-123\"}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"set_tokens\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"changes\\\":\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"radius\\\":4}}\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let url = mock_server(body, 200).await;
        let ep = endpoint(Protocol::Anthropic);
        let mut deltas = String::new();
        let reply = send(&http_client(None).unwrap(), &url, &ep, &json!({}), &CancellationToken::new(), |e| {
            if let Event::TextDelta(t) = e { deltas.push_str(&t) }
        }).await.unwrap();
        assert_eq!(reply.stop, StopReason::ToolUse);
        assert_eq!(deltas, "Saya ubah aksennya. sk-test-secret-123");
        assert_eq!(reply.parts[0], Part::Opaque { block: json!({"type":"thinking","thinking":"","signature":"sig"}) });
        assert_eq!(reply.parts[1], Part::Text { text: "Saya ubah aksennya. [API key]".into() });
        assert_eq!(reply.parts[2], Part::ToolUse { id: "tu_1".into(), name: "set_tokens".into(), input: json!({"changes":{"radius":4}}) });
    }

    #[tokio::test]
    async fn openai_stream_with_tool() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Oke\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"remember\",\"arguments\":\"{\\\"kind\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"user\\\",\\\"text\\\":\\\"x\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let url = mock_server(body, 200).await;
        let reply = send(&http_client(None).unwrap(), &url, &endpoint(Protocol::OpenAi), &json!({}), &CancellationToken::new(), |_| {}).await.unwrap();
        assert_eq!(reply.stop, StopReason::ToolUse);
        assert_eq!(reply.parts, vec![
            Part::Text { text: "Oke".into() },
            Part::ToolUse { id: "c1".into(), name: "remember".into(), input: json!({"kind":"user","text":"x"}) },
        ]);
    }

    #[tokio::test]
    async fn http_error_is_redacted_and_cancel_works() {
        let url = mock_server("{\"error\":{\"message\":\"bad key sk-test-secret-123\"}}", 401).await;
        let err = send(&http_client(None).unwrap(), &url, &endpoint(Protocol::Anthropic), &json!({}), &CancellationToken::new(), |_| {}).await.unwrap_err();
        assert_eq!(err.to_string(), "Endpoint AI merespons HTTP 401. bad key [API key]");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = send(&http_client(None).unwrap(), &url, &endpoint(Protocol::Anthropic), &json!({}), &cancel, |_| {}).await.unwrap_err();
        assert_eq!(err, AiError::Cancelled);
    }

    #[tokio::test]
    async fn endpoint_checks() {
        for bad in ["http://api.example.com", "https://u:p@api.example.com", "https://api.example.com:8443",
                    "https://api.example.com/v1?x=1", "https://10.0.0.1/v1", "https://localhost/v1"] {
            assert!(check_endpoint(bad).await.is_err(), "{bad}");
        }
        assert!(!is_public("100.64.1.1".parse().unwrap()));
        assert!(!is_public("::ffff:192.168.1.1".parse().unwrap()));
        assert!(!is_public("fd00::1".parse().unwrap()));
        assert!(is_public("1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn bodies() {
        let tools: Vec<ToolDef> = crate::tools::catalog().into_iter().take(2).collect();
        let msgs = vec![
            Message { role: Role::User, parts: vec![Part::Text { text: "hai".into() }] },
            Message { role: Role::Assistant, parts: vec![Part::Opaque { block: json!({"type":"thinking","thinking":"","signature":"s"}) }, Part::ToolUse { id: "t".into(), name: "get_design".into(), input: json!({}) }] },
            Message { role: Role::User, parts: vec![Part::ToolResult { tool_use_id: "t".into(), content: "{}".into(), is_error: false }] },
        ];
        let a = build_body(&endpoint(Protocol::Anthropic), true, "sys", &msgs, &tools, 1000);
        assert_eq!(a["tools"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(a["tools"][0]["eager_input_streaming"], true);
        assert_eq!(a["messages"][1]["content"][0]["signature"], "s");
        let a3 = build_body(&endpoint(Protocol::Anthropic), false, "sys", &msgs, &tools, 1000);
        assert!(a3["tools"][0].get("eager_input_streaming").is_none());
        let o = build_body(&endpoint(Protocol::OpenAi), false, "sys", &msgs, &tools, 1000);
        assert_eq!(o["messages"][2]["tool_calls"][0]["function"]["name"], "get_design");
        assert_eq!(o["messages"][3]["role"], "tool");
    }
}
