//! What the engine currently believes, shaped for a page rather than a database.
//!
//! The same join the Kotlin and Swift apps do - catalog plus holders, ids
//! resolved to the names of devices you have actually paired with - except this
//! one happens once per change on this side of the wire, not once per render in
//! every open tab. What the browser receives it can draw.

use crate::Shared;
use crate::events::Message;
use localcloud::Engine;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// How long to wait after a change before looking, so a burst of block-progress
/// events costs one read of the catalog rather than thirty.
const SETTLE: Duration = Duration::from_millis(120);

/// How often to look anyway.
///
/// Replication is pull-only: a catalog that changed on a peer produces no event
/// here, and neither does a device going quiet. Two seconds is what the phone
/// and the Mac app both settled on.
const ANYWAY: Duration = Duration::from_secs(2);

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub device: ThisDevice,
    pub items: Vec<Item>,
    pub trash: Vec<TrashItem>,
    pub visible: Vec<Peer>,
    pub paired: Vec<PairedPeer>,
    pub offers: Vec<Peer>,
    pub collisions: Vec<Collision>,
    /// Deletes aimed at devices that were not reachable. They travel with the
    /// catalog instead, and there is nothing to do about them but know.
    pub deferred_deletes: usize,
}

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThisDevice {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub platform: String,
    pub running: bool,
    pub sync_dir: String,
}

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub name: String,
    pub size: i64,
    /// Whether *this* device has the bytes. It decides what the row can offer -
    /// you cannot send what you do not hold - so it is answered once here
    /// rather than by searching the holder list at every call site.
    pub held_here: bool,
    pub holders: Vec<Holder>,
    pub modified: i64,
}

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub device_id: String,
    pub name: String,
    pub is_this_device: bool,
    pub reachable: bool,
}

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    pub id: String,
    pub name: String,
    pub size: i64,
    pub seconds_remaining: Option<i64>,
    pub trashed_by: String,
}

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub device_id: String,
    pub name: String,
    pub platform: String,
}

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairedPeer {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub reachable: bool,
}

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collision {
    pub id: String,
    pub requested: String,
    pub kept_as: String,
}

/// Reads everything in one pass.
///
/// One pass because these have to agree with each other: a holder list read a
/// second after the catalog can name an item the catalog no longer has, and the
/// row that results is a bug nobody can reproduce.
pub fn read(engine: &Engine) -> Snapshot {
    let this_id = engine.device_id();
    let catalog = engine.catalog();
    let paired = engine.paired_devices();
    let visible = engine.visible_devices();

    let mut names: HashMap<String, String> = HashMap::new();
    names.insert(this_id.clone(), engine.device_name());
    for device in &paired {
        names.insert(device.id.clone(), device.name.clone());
    }
    for device in &visible {
        names
            .entry(device.device_id.clone())
            .or_insert_with(|| device.name.clone());
    }

    let mut reachable: HashSet<String> = visible.iter().map(|d| d.device_id.clone()).collect();
    reachable.insert(this_id.clone());

    let mut holders_by_file: HashMap<&str, Vec<Holder>> = HashMap::new();
    for holder in &catalog.holders {
        holders_by_file
            .entry(holder.file_id.as_str())
            .or_default()
            .push(Holder {
                name: names
                    .get(&holder.device_id)
                    .cloned()
                    .unwrap_or_else(|| short(&holder.device_id)),
                is_this_device: holder.device_id == this_id,
                reachable: reachable.contains(&holder.device_id),
                device_id: holder.device_id.clone(),
            });
    }
    for holders in holders_by_file.values_mut() {
        // This device first: the answer to "do I have this?" should be in the
        // same place on every row.
        holders.sort_by(|a, b| {
            b.is_this_device
                .cmp(&a.is_this_device)
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    let mut items: Vec<Item> = catalog
        .items
        .iter()
        .filter(|meta| meta.trashed_at == 0)
        .map(|meta| {
            let holders = holders_by_file.remove(meta.id.as_str()).unwrap_or_default();
            Item {
                id: meta.id.clone(),
                name: file_name(&meta.path),
                size: meta.size,
                held_here: holders.iter().any(|h| h.is_this_device),
                holders,
                modified: meta.modified_time,
            }
        })
        .collect();
    items.sort_by(|a, b| b.modified.cmp(&a.modified));

    let trash = engine
        .trashed_files()
        .into_iter()
        .map(|meta| TrashItem {
            seconds_remaining: engine.trash_seconds_remaining(meta.id.clone()),
            trashed_by: names
                .get(&meta.trashed_by)
                .cloned()
                .unwrap_or_else(|| short(&meta.trashed_by)),
            name: file_name(&meta.path),
            size: meta.size,
            id: meta.id,
        })
        .collect();

    let known: HashSet<&str> = paired.iter().map(|d| d.id.as_str()).collect();

    Snapshot {
        device: ThisDevice {
            short_id: short(&this_id),
            name: engine.device_name(),
            platform: engine.device_platform(),
            running: engine.is_running(),
            sync_dir: engine.sync_dir(),
            id: this_id,
        },
        items,
        trash,
        // Only the ones that are nobody yet. A paired device already has a row
        // of its own, and listing it twice invites pairing with it again.
        visible: visible
            .iter()
            .filter(|d| !known.contains(d.device_id.as_str()))
            .map(|d| Peer {
                device_id: d.device_id.clone(),
                name: d.name.clone(),
                platform: d.platform.clone(),
            })
            .collect(),
        paired: paired
            .iter()
            .map(|d| PairedPeer {
                reachable: reachable.contains(&d.id),
                device_id: d.id.clone(),
                name: d.name.clone(),
                platform: d.platform.clone(),
            })
            .collect(),
        offers: engine
            .pairing_offers()
            .into_iter()
            .map(|o| Peer {
                device_id: o.device_id,
                name: o.name,
                platform: o.platform,
            })
            .collect(),
        collisions: engine
            .pending_collisions()
            .into_iter()
            .map(|c| Collision {
                id: c.id,
                requested: file_name(&c.requested_path),
                kept_as: file_name(&c.current_path),
            })
            .collect(),
        deferred_deletes: engine.pending_delete_requests().len(),
    }
}

/// Sends the page a new snapshot when, and only when, there is a different one.
///
/// Every engine call blocks, so the read happens on a blocking thread rather
/// than on the one serving requests. The comparison is against the serialised
/// bytes: cheaper than a deep equality that would have to be written and kept
/// in step with the structs, and it is exactly the question being asked - is
/// there anything new to send.
pub async fn broadcast_changes(app: Shared) {
    let mut last: Option<String> = None;

    loop {
        let engine = app.engine.clone();
        let Ok(json) = tokio::task::spawn_blocking(move || {
            serde_json::to_string(&read(&engine)).unwrap_or_default()
        })
        .await
        else {
            return;
        };

        if last.as_deref() != Some(json.as_str()) {
            let _ = app.updates.send(Arc::new(Message {
                kind: "state",
                json: json.clone(),
            }));
            last = Some(json);
        }

        tokio::select! {
            _ = app.changed.notified() => tokio::time::sleep(SETTLE).await,
            _ = tokio::time::sleep(ANYWAY) => {}
        }
    }
}

/// The engine stores a path within the sync folder; a person wants the name.
pub fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Enough of a device id to tell two apart, when there is no name for it.
pub fn short(device_id: &str) -> String {
    device_id.chars().take(8).collect()
}
