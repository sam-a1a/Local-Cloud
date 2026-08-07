// engine/src/discovery.rs
use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde::Serialize;
use std::collections::HashMap;

use std::sync::{Arc, Mutex, mpsc};
use crate::db::Database;
use crate::ignore::IgnoreSet;
use crate::tls::TrustStore;
use crate::EngineEvent;

const SERVICE_TYPE: &str = "_local-cloud._tcp.local.";

/// A device seen on the local network. Being discovered says nothing about
/// trust: unpaired devices appear here so they can be picked for pairing, and
/// get no catalog or data access until that completes.
#[derive(Clone, Debug, Serialize)]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub url: String,
}

/// device_id -> most recently resolved advertisement for that device.
pub type PeerMap = Arc<Mutex<HashMap<String, DiscoveredDevice>>>;

/// Asks the engine to sync with a device now, by id.
///
/// A peer becomes worth syncing with for two reasons - it was just discovered
/// while already paired, or it was just paired while already visible - and both
/// have to prompt it, or which one happened last would decide whether anything
/// appears before the next scheduled pass.
pub type SyncNudge = tokio::sync::mpsc::UnboundedSender<String>;

/// Picks the address to reach a peer on, and formats it as a base URL.
///
/// A device advertises every address it has, and they arrive as an unordered
/// set, so simply taking the first gives a different answer each time and makes
/// one device look like several. Ranking them keeps it stable, and the order
/// reflects what actually works:
///
/// - routable IPv4 first, which is what a home network hands out
/// - then global IPv6
/// - loopback only as a last resort, which is really two instances on one
///   machine
/// - link-local IPv6 never: it is meaningless without a scope id, and a scope
///   id does not survive being put in a URL
pub(crate) fn peer_url(
    addresses: impl IntoIterator<Item = std::net::IpAddr>,
    port: u16,
) -> Option<String> {
    use std::net::IpAddr;

    let rank = |addr: &IpAddr| -> Option<u8> {
        match addr {
            IpAddr::V4(v4) if v4.is_unspecified() => None,
            IpAddr::V4(v4) if v4.is_loopback() => Some(2),
            IpAddr::V4(_) => Some(0),
            IpAddr::V6(v6) if v6.is_unspecified() => None,
            // fe80::/10
            IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80 => None,
            IpAddr::V6(v6) if v6.is_loopback() => Some(3),
            IpAddr::V6(_) => Some(1),
        }
    };

    let best = addresses
        .into_iter()
        .filter_map(|addr| rank(&addr).map(|r| (r, addr)))
        // Tie-break on the address itself so repeated resolutions agree.
        .min_by_key(|(r, addr)| (*r, addr.to_string()))
        .map(|(_, addr)| addr)?;

    Some(match best {
        // IPv6 in a URL has to be bracketed or the port is unparseable.
        IpAddr::V6(v6) => format!("https://[{}]:{}", v6, port),
        IpAddr::V4(v4) => format!("https://{}:{}", v4, port),
    })
}

#[derive(Debug)]
struct TrustedPeerServerVerifier {
    trust: TrustStore,
}

impl TrustedPeerServerVerifier {
    fn new(trusted_cert_pems: &[String]) -> Self {
        Self {
            trust: TrustStore::from_pems(trusted_cert_pems),
        }
    }
}

impl ServerCertVerifier for TrustedPeerServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if self.trust.is_trusted(end_entity) {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "Server cert not in trusted peers list".into(),
            ))
        }
    }

    crate::impl_tls_verifier_methods!();
}

pub fn build_mtls_client(
    cert_pem: &str,
    key_pem: &str,
    trusted_cert_pems: &[String],
) -> Result<reqwest::Client> {
    let (certs, key) = crate::tls::load_certs_and_key(cert_pem, key_pem)?;

    let verifier = Arc::new(TrustedPeerServerVerifier::new(trusted_cert_pems));

    let tls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(certs, key)?;

    let client = reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .build()?;

    Ok(client)
}

pub fn start_discovery(
    device_id: String,
    device_name: String,
    port: u16,
    event_tx: mpsc::Sender<EngineEvent>,
    known_peers: PeerMap,
    // Separate from `event_tx`, which belongs to whoever embedded the engine:
    // an event channel has one consumer, so the engine cannot both hand
    // discoveries to the caller and act on them itself through the same one.
    // Carries the device id rather than the whole advertisement, so a sync
    // always uses the address the peer is reachable at *now*.
    peer_found: SyncNudge,
) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;

    let short_id = &device_id[..8];
    let host_name = format!("{}.local.", short_id);
    let mut properties = HashMap::new();
    properties.insert("device_id".to_string(), device_id.clone());
    properties.insert("name".to_string(), device_name);
    properties.insert("platform".to_string(), crate::crypto::platform_name().to_string());

    // Addresses are left to the daemon rather than guessed.
    //
    // Picking one by opening a socket towards the internet returns whichever
    // address carries the default route, which on any machine with a VPN up is
    // the tunnel rather than the LAN. The daemon then holds a service whose
    // address belongs to no interface it broadcasts on and silently never
    // announces it - no error, just a mesh where nothing ever finds anything.
    // Letting it fill them in also covers multi-homed hosts and addresses that
    // change while running.
    let service_info = ServiceInfo::new(SERVICE_TYPE, short_id, &host_name, (), port, Some(properties))?
        .enable_addr_auto();

    daemon.register(service_info)?;

    let receiver = daemon.browse(SERVICE_TYPE)?;
    let my_short_id = short_id.to_string();

    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                let peer_name = info.get_fullname();
                if peer_name.starts_with(&my_short_id) {
                    continue;
                }

                let peer_id = match info.get_property("device_id") {
                    Some(p) => p.val_str().to_string(),
                    None => continue,
                };

                // Fall back to the short id so a device with a malformed
                // advertisement is still selectable rather than nameless.
                let name = info
                    .get_property("name")
                    .map(|p| p.val_str().to_string())
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| peer_id.chars().take(8).collect());
                let platform = info
                    .get_property("platform")
                    .map(|p| p.val_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                let Some(url) = peer_url(
                    info.get_addresses().iter().map(|scoped| scoped.to_ip_addr()),
                    info.get_port(),
                ) else {
                    continue;
                };

                let device = DiscoveredDevice {
                    device_id: peer_id.clone(),
                    name,
                    platform,
                    url,
                };

                // mDNS re-resolves periodically, so only announce a device that
                // is new or has actually moved.
                let changed = {
                    let mut peers = known_peers.lock().unwrap();
                    match peers.get(&peer_id) {
                        Some(known) if known.url == device.url && known.name == device.name => false,
                        _ => {
                            peers.insert(peer_id, device.clone());
                            true
                        }
                    }
                };

                if changed {
                    let _ = peer_found.send(device.device_id.clone());
                    let _ = event_tx.send(EngineEvent::PeerDiscovered { device });
                }
            }
        }
    });

    Ok(daemon)
}

/// How many blocks are in the air between two devices at once.
///
/// A block is one HTTP request, and a request on a LAN spends most of its life
/// waiting rather than moving bytes. Sending the next only once the last has
/// come back leaves the link almost entirely idle, so a larger block size alone
/// would not have bought much; overlapping a handful is the other half of it.
/// Past this the link rather than the round-trips is the limit, and each one in
/// flight costs a block held in memory at both ends.
const TRANSFER_CONCURRENCY: usize = 8;

/// Runs `start` over every block with a bounded number in flight, giving up as
/// soon as one fails.
///
/// A partial transfer is worth nothing: the receiver will not assemble a file
/// until every block is present, so continuing after a failure only spends time
/// and bandwidth on a copy that cannot complete. Whatever did arrive is kept -
/// blocks are content-addressed, so a retry picks up where this left off.
async fn transfer_all<F, Fut>(blocks: &[crate::db::FileBlock], start: F) -> bool
where
    F: Fn(&crate::db::FileBlock) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    // A manifest names positions, so a block that fills several of them appears
    // several times - a file with a long run of zeros is mostly one block
    // repeated. Moving it once is enough: storage is addressed by content, so
    // every position it occupies is satisfied by the same transfer.
    let mut seen = std::collections::HashSet::new();
    let mut queued = blocks.iter().filter(|b| seen.insert(b.block_id.clone()));
    let mut in_flight = tokio::task::JoinSet::new();
    let mut failed = false;

    loop {
        while in_flight.len() < TRANSFER_CONCURRENCY {
            match queued.next() {
                Some(block) => {
                    in_flight.spawn(start(block));
                }
                None => break,
            }
        }

        let Some(finished) = in_flight.join_next().await else {
            break;
        };

        match finished {
            Ok(Ok(())) => continue,
            Ok(Err(e)) => println!("[Transfer] {}", e),
            Err(e) => println!("[Transfer] A block transfer did not finish: {}", e),
        }

        failed = true;
        in_flight.abort_all();
        while in_flight.join_next().await.is_some() {}
        break;
    }

    !failed
}

async fn fetch_blocks_from_peer(
    client: &reqwest::Client,
    peer_url: &str,
    blocks: &[crate::db::FileBlock],
    db_clone: &Arc<Mutex<Database>>,
    storage_dir_clone: &str,
) -> bool {
    transfer_all(blocks, |b| {
        let client = client.clone();
        let peer_url = peer_url.to_string();
        let storage_dir = storage_dir_clone.to_string();
        let db = db_clone.clone();
        let block_id = b.block_id.clone();

        async move {
            let resp = client
                .get(format!("{}/get_block/{}", peer_url, block_id))
                .send()
                .await
                .map_err(|e| format!("Could not ask {} for a block: {}", peer_url, e))?;

            if resp.status() != 200 {
                return Err(format!("{} would not serve a block: {}", peer_url, resp.status()));
            }

            let data = resp
                .bytes()
                .await
                .map_err(|e| format!("A block from {} did not arrive: {}", peer_url, e))?;

            // Storing checks that the bytes hash to the id they were asked for,
            // so a peer serving anything else is refused rather than having it
            // assembled into the file unnoticed. Hashing and writing a megabyte
            // are both blocking work and do not belong on a runtime thread.
            let stored = {
                let block_id = block_id.clone();
                tokio::task::spawn_blocking(move || {
                    crate::storage::write_block(&storage_dir, &block_id, &data)
                })
                .await
                .map_err(|e| format!("Storing a block failed: {}", e))?
            };
            stored.map_err(|e| format!("Refusing a block from {}: {}", peer_url, e))?;

            let db = db.lock().unwrap();
            db.set_block_present(&block_id, true)
                .map_err(|e| format!("Could not record a block as present: {}", e))
        }
    })
    .await
}

/// Sends the blocks of an item to a peer that has already accepted its
/// metadata.
pub async fn push_blocks_to_peer(
    client: &reqwest::Client,
    peer_url: &str,
    blocks: &[crate::db::FileBlock],
    storage_dir: &str,
) -> bool {
    transfer_all(blocks, |b| {
        let client = client.clone();
        let peer_url = peer_url.to_string();
        let storage_dir = storage_dir.to_string();
        let block_id = b.block_id.clone();

        async move {
            let data = {
                let wanted = block_id.clone();
                tokio::task::spawn_blocking(move || crate::storage::read_block(&storage_dir, &wanted))
                    .await
                    .map_err(|e| format!("Reading a block failed: {}", e))?
                    .map_err(|e| format!("Missing block {}: {}", block_id, e))?
            };

            let sent = client
                .post(format!("{}/push_block/{}", peer_url, block_id))
                .body(data)
                .send()
                .await
                .map_err(|e| format!("Could not send a block to {}: {}", peer_url, e))?;

            if !sent.status().is_success() {
                return Err(format!("{} refused a block: {}", peer_url, sent.status()));
            }
            Ok(())
        }
    })
    .await
}

pub async fn sync_with_peer(
    peer_url: String,
    peer_id: String,
    my_device_id: String,
    db_clone: Arc<Mutex<Database>>,
    storage_dir_clone: String,
    sync_dir: String,
    cert_pem: String,
    key_pem: String,
    ignore_set: IgnoreSet,
    collisions: crate::collision::CollisionQueue,
    indexer: crate::watcher::Indexer,
    event_tx: mpsc::Sender<EngineEvent>,
) {
    // 1. Sync only with devices that have been through pairing.
    //
    // This used to fetch the peer's certificate over an unverified connection
    // and pin it on the spot, which meant anything on the network could join by
    // simply showing up. Trust now comes from pairing and nowhere else.
    let paired = {
        let db = db_clone.lock().unwrap();
        db.is_paired(&peer_id).unwrap_or(false)
    };

    if !paired {
        return;
    }

    println!("[Sync] Starting metadata sync with {}", peer_id);

    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        let db = db_clone.lock().unwrap();
        let _ = db.touch_device(&peer_id, now.as_secs() as i64);
    }

    let trusted_certs = match crate::storage::load_all_trusted_certs(&storage_dir_clone) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mtls_client = match build_mtls_client(&cert_pem, &key_pem, &trusted_certs) {
        Ok(c) => c,
        Err(_) => return,
    };

    // 2. Pull the peer's view of the shared namespace.
    //
    // Catalog replication is pull-only in both directions: each device asks its
    // peers what they know rather than broadcasting. Nothing here moves file
    // data - bytes only ever move because a person shared or pulled something.
    let catalog: crate::db::Catalog = match mtls_client
        .get(format!("{}/catalog", peer_url))
        .send()
        .await
    {
        Ok(res) => match res.json().await {
            Ok(c) => c,
            Err(e) => {
                println!("[Sync] Failed to parse catalog: {}", e);
                return;
            }
        },
        Err(e) => {
            println!("[Sync] Failed to fetch catalog: {}", e);
            return;
        }
    };

    // Destructions are applied first, so an item the peer already destroyed is
    // not merged back in only to be removed again.
    for tombstone in &catalog.tombstones {
        match indexer.apply_tombstone(tombstone) {
            Ok(true) => println!("[Sync] {} was destroyed elsewhere", tombstone.file_id),
            Ok(false) => {}
            Err(e) => println!("[Sync] Could not apply tombstone: {}", e),
        }
    }

    let db = db_clone.lock().unwrap();

    for file in &catalog.files {
        match db.merge_catalog_file(file) {
            Ok(crate::db::MergeOutcome::Applied) => {}
            Ok(crate::db::MergeOutcome::Renamed {
                file_id,
                from,
                to,
                conflicting_file_id,
            }) => {
                println!("[Sync] \"{}\" was taken; \"{}\" kept as \"{}\"", from, from, to);

                // If the item that moved is one we hold, the sync folder has to
                // follow, or it would disagree with the catalog about the name.
                if db.is_holder(&file_id, &my_device_id).unwrap_or(false) {
                    let from_abs = format!("{}/{}", sync_dir, from);
                    let to_abs = format!("{}/{}", sync_dir, to);
                    if std::path::Path::new(&from_abs).exists() {
                        crate::ignore::mark_ignored(&ignore_set, &from_abs);
                        crate::ignore::mark_ignored(&ignore_set, &to_abs);
                        if let Err(e) = std::fs::rename(&from_abs, &to_abs) {
                            println!("[Sync] Renamed in catalog but not on disk: {}", e);
                        }
                        crate::ignore::schedule_unmark_ignored(ignore_set.clone(), from_abs, 3);
                        crate::ignore::schedule_unmark_ignored(ignore_set.clone(), to_abs, 3);
                    }
                }

                let (incoming_file_id, existing_file_id) = (file_id, conflicting_file_id);
                collisions.record(crate::collision::PendingCollision {
                    id: uuid::Uuid::new_v4().to_string(),
                    incoming_file_id,
                    requested_path: from.clone(),
                    current_path: to.clone(),
                    existing_file_id,
                    existing_created_by: file.created_by.clone(),
                    detected_at: file.modified_time,
                });

                let _ = event_tx.send(EngineEvent::NameCollision {
                    requested_path: from,
                    kept_as: to,
                });
            }
            Ok(crate::db::MergeOutcome::AlreadyDestroyed) => {}
            Err(e) => println!("[Sync] Skipped {}: {}", file.path, e),
        }
    }

    // The peer is the sole authority on its own copies, so its rows replace
    // ours wholesale - that is how a copy it deleted stops being listed.
    // Rows for destroyed items are dropped rather than becoming orphans.
    let peer_holders: Vec<crate::db::FileHolder> = catalog
        .holders
        .iter()
        .filter(|h| h.device_id == peer_id)
        .filter(|h| !db.has_tombstone(&h.file_id).unwrap_or(false))
        .cloned()
        .collect();
    if let Err(e) = db.replace_holders_for_device(&peer_id, &peer_holders) {
        println!("[Sync] Failed to record {}'s copies: {}", peer_id, e);
    }

    // What it reports about third devices is useful for discovering copies on
    // devices we cannot currently reach, but it is never allowed to overwrite
    // what we know about ourselves.
    for holder in &catalog.holders {
        if holder.device_id == peer_id || holder.device_id == my_device_id {
            continue;
        }
        if db.has_tombstone(&holder.file_id).unwrap_or(false) {
            continue;
        }
        let _ = db.set_holder(holder);
    }

    // Deletes aimed at a device that was away travel through the catalog, so
    // they have to be carried forward even when they concern neither of us.
    for request in &catalog.delete_requests {
        let _ = db.record_delete_request(request);
    }

    drop(db);

    // Anything aimed at this device is carried out now.
    for outcome in indexer.apply_pending_delete_requests() {
        let event = if outcome.trashed {
            EngineEvent::FileTrashed { file_id: outcome.file_id }
        } else {
            EngineEvent::CopyDeleted {
                file_id: outcome.file_id,
                device_id: my_device_id.clone(),
            }
        };
        let _ = event_tx.send(event);
    }

    // Requests whose target is no longer a live holder are done with.
    {
        let db = db_clone.lock().unwrap();
        let _ = db.prune_satisfied_delete_requests();
    }

    println!("[Sync] Finished catalog sync with {}", peer_id);
}

/// Takes a copy of an item for this device.
///
/// The counterpart to sharing: instead of a sender choosing a destination, a
/// device helps itself to something it can see. Both are deliberate acts by a
/// person; nothing here runs on its own.
pub async fn pull_copy(
    file_id: String,
    my_device_id: String,
    db_clone: Arc<Mutex<Database>>,
    storage_dir: String,
    sync_dir: String,
    cert_pem: String,
    key_pem: String,
    known_peers: PeerMap,
    ignore_set: IgnoreSet,
    event_tx: mpsc::Sender<EngineEvent>,
) -> Result<(), String> {
    let (file, blocks, missing) = {
        let db = db_clone.lock().unwrap();
        let file = db
            .get_file_by_id(&file_id)
            .map_err(|e| e.to_string())?
            .ok_or("No such item in the catalog")?;
        if file.is_trashed() {
            return Err("That item is in the trash; restore it first".to_string());
        }
        let blocks = db.get_blocks_for_file(&file_id).map_err(|e| e.to_string())?;
        let missing: Vec<crate::db::FileBlock> =
            blocks.iter().filter(|b| b.is_present == 0).cloned().collect();
        (file, blocks, missing)
    };

    if blocks.is_empty() {
        return Err("No content is recorded for that item yet".to_string());
    }

    if !missing.is_empty() {
        // Only paired devices that actually hold this item are worth asking.
        // Anything else either has nothing to give or would refuse.
        let sources: Vec<DiscoveredDevice> = {
            let visible = known_peers.lock().unwrap().clone();
            let db = db_clone.lock().unwrap();
            visible
                .into_values()
                .filter(|d| db.is_paired(&d.device_id).unwrap_or(false))
                .filter(|d| db.is_holder(&file_id, &d.device_id).unwrap_or(false))
                .collect()
        };

        if sources.is_empty() {
            return Err("No device holding that item is reachable".to_string());
        }

        let trusted_certs =
            crate::storage::load_all_trusted_certs(&storage_dir).map_err(|e| e.to_string())?;
        let client =
            build_mtls_client(&cert_pem, &key_pem, &trusted_certs).map_err(|e| e.to_string())?;

        let mut fetched = false;
        for source in sources {
            if fetch_blocks_from_peer(&client, &source.url, &missing, &db_clone, &storage_dir).await
            {
                fetched = true;
                break;
            }
            println!("[Pull] {} could not supply the content", source.name);
        }

        if !fetched {
            return Err("Could not get the content from any holder".to_string());
        }
    }

    // On desktop the folder holds exactly what this device holds, so a pulled
    // item has to appear in it.
    let output_path = format!("{}/{}", sync_dir, file.path);
    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::ignore::mark_ignored(&ignore_set, &output_path);
    crate::storage::assemble_file_from_blocks(&storage_dir, &output_path, &blocks)
        .map_err(|e| e.to_string())?;
    crate::ignore::schedule_unmark_ignored(ignore_set.clone(), output_path, 3);

    let downloaded = file_id.clone();

    // Claiming the holder row is what makes the copy visible to everyone else.
    {
        let db = db_clone.lock().unwrap();
        let _ = db.set_holder(&crate::db::FileHolder {
            file_id,
            device_id: my_device_id,
            content_hash: crate::storage::content_hash(&blocks),
            received_at: crate::watcher::now_secs(),
        });
    }

    let _ = event_tx.send(EngineEvent::FileDownloaded {
        file_id: downloaded,
        path: file.path,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::peer_url;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("address")
    }

    #[test]
    fn a_routable_ipv4_is_preferred() {
        let url = peer_url([ip("::1"), ip("127.0.0.1"), ip("10.234.3.171")], 8080);
        assert_eq!(url.as_deref(), Some("https://10.234.3.171:8080"));
    }

    #[test]
    fn ipv6_is_bracketed_or_the_port_cannot_be_parsed() {
        let url = peer_url([ip("2001:db8::1")], 8080);
        assert_eq!(url.as_deref(), Some("https://[2001:db8::1]:8080"));
    }

    #[test]
    fn link_local_ipv6_is_never_used() {
        // Meaningless without a scope id, and a scope id does not survive a URL.
        assert_eq!(peer_url([ip("fe80::1")], 8080), None);
        assert_eq!(
            peer_url([ip("fe80::1"), ip("192.168.1.5")], 8080).as_deref(),
            Some("https://192.168.1.5:8080")
        );
    }

    #[test]
    fn loopback_is_a_last_resort_but_still_works() {
        // Two instances on one machine, which is how this gets tested.
        assert_eq!(
            peer_url([ip("127.0.0.1")], 8080).as_deref(),
            Some("https://127.0.0.1:8080")
        );
        assert_eq!(
            peer_url([ip("127.0.0.1"), ip("10.0.0.4")], 8080).as_deref(),
            Some("https://10.0.0.4:8080")
        );
    }

    #[test]
    fn the_same_addresses_always_give_the_same_url() {
        // Addresses arrive as an unordered set, so an arbitrary pick would make
        // one device look like it keeps moving between resolutions.
        let addresses = [ip("10.0.0.9"), ip("10.0.0.4"), ip("192.168.1.5")];
        let first = peer_url(addresses, 8080);
        for _ in 0..20 {
            assert_eq!(peer_url(addresses, 8080), first);
        }
    }

    #[test]
    fn a_device_advertising_nothing_usable_is_skipped() {
        assert_eq!(peer_url([], 8080), None);
        assert_eq!(peer_url([ip("0.0.0.0"), ip("fe80::abc")], 8080), None);
    }
}
