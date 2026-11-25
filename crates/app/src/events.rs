//! Kanal realtime: satu `broadcast` untuk semua tab browser, dikirim sebagai SSE.

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::Stream;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

/// Jenis event yang disepakati dengan modul agen:
/// telemetry.update, customer.update, router.status, chat.delta, chat.done, design.patch, design.version.
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub kind: &'static str,
    pub data: serde_json::Value,
}

#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<Event>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new(256)
    }
}

impl Hub {
    pub fn new(capacity: usize) -> Self {
        Self {
            tx: broadcast::channel(capacity).0,
        }
    }

    #[allow(dead_code)] // dipakai poller (tahap 2) dan agen
    pub fn publish(&self, kind: &'static str, data: serde_json::Value) {
        // Tidak ada pendengar bukan kesalahan.
        let _ = self.tx.send(Event { kind, data });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn sse(&self) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>> + use<>> {
        let stream = BroadcastStream::new(self.subscribe()).map(|item| {
            Ok(match item {
                Ok(ev) => SseEvent::default().event(ev.kind).data(ev.data.to_string()),
                // Klien tertinggal: minta muat ulang data, bukan memutus koneksi.
                Err(_) => SseEvent::default().event("resync").data("{}"),
            })
        });
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
    }
}
