//! Engine events, on their way to whatever tabs are open.
//!
//! Two kinds reach the page directly. Progress is one of them, because a
//! thousand block events during a large transfer must not become a thousand
//! reads of the catalog - they carry everything a progress bar needs and
//! nothing a catalog would add. Notices are the other: an outcome that happened
//! after the request that caused it had already returned.
//!
//! Everything else only rings the bell. It changed something the next snapshot
//! will show, and there is nothing to say about it that the page will not draw
//! on its own.

use crate::Shared;
use crate::snapshot::{file_name, short};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::StreamExt;
use futures_util::stream::Stream;
use localcloud::{EngineEvent, EventListener};
use serde::Serialize;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{Notify, broadcast};

/// One thing to tell the page, serialised once however many tabs are open.
pub struct Message {
    pub kind: &'static str,
    pub json: String,
}

impl Message {
    pub fn new(kind: &'static str, payload: &impl Serialize) -> Option<Arc<Self>> {
        serde_json::to_string(payload)
            .ok()
            .map(|json| Arc::new(Self { kind, json }))
    }
}

pub struct Bridge {
    updates: broadcast::Sender<Arc<Message>>,
    changed: Arc<Notify>,
}

impl Bridge {
    pub fn new(updates: broadcast::Sender<Arc<Message>>, changed: Arc<Notify>) -> Self {
        Self { updates, changed }
    }

    fn send(&self, kind: &'static str, payload: &impl Serialize) {
        if let Some(message) = Message::new(kind, payload) {
            let _ = self.updates.send(message);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    file_id: &'a str,
    /// Absent while receiving: a pull has one source and the page has no reason
    /// to care which. Sending has one bar per destination.
    device_id: Option<&'a str>,
    sending: bool,
    done: u64,
    total: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Done<'a> {
    file_id: &'a str,
    device_id: Option<&'a str>,
}

#[derive(Serialize)]
struct Notice<'a> {
    text: &'a str,
    failure: bool,
}

impl EventListener for Bridge {
    /// Called by the engine, on the engine's own dispatch thread.
    ///
    /// Nothing here blocks and nothing here calls back into the engine. The
    /// most it does is serialise a small struct and hand it to a broadcast
    /// channel, which never waits for a receiver.
    fn on_event(&self, event: EngineEvent) {
        match &event {
            EngineEvent::SendProgress {
                file_id,
                device_id,
                blocks_done,
                blocks_total,
            } => self.send(
                "progress",
                &Progress {
                    file_id,
                    device_id: Some(device_id),
                    sending: true,
                    done: *blocks_done,
                    total: *blocks_total,
                },
            ),

            EngineEvent::ReceiveProgress {
                file_id,
                blocks_done,
                blocks_total,
            } => self.send(
                "progress",
                &Progress {
                    file_id,
                    device_id: None,
                    sending: false,
                    done: *blocks_done,
                    total: *blocks_total,
                },
            ),

            EngineEvent::FileSent {
                file_id,
                path,
                device_id,
            } => {
                self.send(
                    "done",
                    &Done {
                        file_id,
                        device_id: Some(device_id),
                    },
                );
                self.notice(&format!(
                    "{} reached {}.",
                    file_name(path),
                    short(device_id)
                ), false);
            }

            EngineEvent::ShareFailed {
                file_id,
                path,
                device_id,
                reason,
            } => {
                self.send(
                    "done",
                    &Done {
                        file_id,
                        device_id: Some(device_id),
                    },
                );
                self.notice(
                    &format!("{} did not arrive: {reason}", file_name(path)),
                    true,
                );
            }

            EngineEvent::FileDownloaded { file_id, path } => {
                self.send(
                    "done",
                    &Done {
                        file_id,
                        device_id: None,
                    },
                );
                self.notice(&format!("{} is now on this device.", file_name(path)), false);
            }

            EngineEvent::PullFailed { file_id, reason } => {
                self.send(
                    "done",
                    &Done {
                        file_id,
                        device_id: None,
                    },
                );
                self.notice(&format!("Could not take a copy: {reason}"), true);
            }

            EngineEvent::DevicePaired { name, .. } => {
                self.notice(&format!("Paired with {name}."), false)
            }
            EngineEvent::PairingFailed { reason, .. } => {
                self.notice(&format!("Pairing failed: {reason}"), true)
            }
            EngineEvent::NameCollision {
                requested_path,
                kept_as,
            } => self.notice(
                &format!(
                    "{} was already taken, so it arrived as {}.",
                    file_name(requested_path),
                    file_name(kept_as)
                ),
                false,
            ),
            EngineEvent::DeleteRequestDeferred { device_id, .. } => self.notice(
                &format!(
                    "{} could not be reached, so the delete travels with the catalog.",
                    short(device_id)
                ),
                false,
            ),
            EngineEvent::EngineFailed { reason } => self.notice(reason, true),

            _ => {}
        }

        self.changed.notify_one();
    }
}

impl Bridge {
    fn notice(&self, text: &str, failure: bool) {
        self.send("notice", &Notice { text, failure });
    }
}

/// One long-lived response per tab, and the same `Arc<str>` frame handed to
/// each of them.
///
/// Lagging is skipped rather than closed. A tab that was asleep missed some
/// progress bars; the next snapshot is complete and puts it right, which is
/// exactly why the snapshot is whole rather than a diff.
pub async fn stream(
    State(app): State<Shared>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribed before the first snapshot is read, so a change landing between
    // the two is queued rather than missed.
    let receiver = app.updates.subscribe();

    // Whatever is true right now, before anything has changed.
    //
    // The shared loop only sends a snapshot that differs from the last one it
    // sent, which is what keeps a quiet mesh quiet - but it means a tab opening
    // into that quiet would draw nothing at all until it stopped being quiet.
    // Quiet is the normal state of a mesh.
    let engine = app.engine();
    let current = tokio::task::spawn_blocking(move || {
        serde_json::to_string(&crate::snapshot::read(&engine)).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let opening =
        futures_util::stream::once(
            async move { Ok(Event::default().event("state").data(current)) },
        );

    let changes = futures_util::stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(message) => {
                    let event = Event::default().event(message.kind).data(&message.json);
                    return Some((Ok(event), receiver));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(opening.chain(changes)).keep_alive(KeepAlive::default())
}
