# LocalCloud — Design

A cloud with no server. Your own devices, on your own network, forming a mesh that
behaves like shared storage — except you decide explicitly what goes where.

The reference point is Mega or AirDrop rather than Dropbox: **you select what to
share and to which device.** Nothing is copied, moved, or updated without a person
asking for it.

---

## 1. Devices and pairing

Any device can start pairing; there is no designated host.

1. The initiator lists every LocalCloud device it can see on the network, by name
   and platform.
2. You multi-select the ones you want. The initiator displays a single random
   **6-digit code**.
3. Each selected device prompts for that code. You enter it on each.
4. On a match, the two devices exchange and pin each other's TLS certificates.
   They are now paired, permanently, until explicitly unpaired.

The code exists to authenticate the certificate exchange. It must have a short
expiry (~3 minutes) and a hard attempt cap (5), or an attacker on the same network
can simply try all 10^6 combinations.

Devices are identified by a hex-encoded Ed25519 public key. Unpaired devices on the
network are visible for the purpose of pairing and nothing else — they get no
catalog access and no data access.

**Scope: same local network only.** Discovery is mDNS. There is no relay, no NAT
traversal, no rendezvous server. Leave the network and sync stops until you return.

---

## 2. The shared space

Paired devices share one flat namespace — the **catalog**. Every paired device
holds a complete copy of the catalog: every item's name, size, and the set of
devices that physically hold its bytes.

The catalog is metadata only. Being able to see an item does not mean having it.

An item enters the catalog when a person puts it there: dropped into the sync
folder on desktop, or imported via the share sheet on mobile. Files outside that
space are private and invisible to the mesh.

---

## 3. Copies and holders

Each item carries a **holder set** — the devices that currently have its bytes,
and *which version* of the content each one has:

```
notes.txt
  Android   current      edited 2m ago
  macOS     older        sent 3 days ago
  Windows   current
```

Two rules govern it:

**Each device is authoritative for its own membership.** Only macOS can add or
remove macOS from a holder set, because only macOS can actually write or erase
macOS's disk. Every cross-device operation is therefore a *request* that the
holder executes and then publishes. This sidesteps concurrent-update merging
entirely — there is never more than one writer per row.

**Copies are snapshots, not replicas.** Editing an item on one device does not
touch any other copy. The Mac's `notes.txt` stays byte-for-byte as it was until
someone explicitly sends it again. This is why the holder set records a content
hash per copy: without it the catalog would claim "macOS has notes.txt" while
quietly meaning a three-edits-old version.

---

## 4. Moving bytes

Two operations, both requiring a deliberate human act:

**Push** — the sender selects an item and one or more destination devices, and
sends. The bytes are transferred; the sender keeps its own copy. This is the
primary gesture.

**Pull** — a device sees an item in the catalog and takes a copy for itself.

Both end the same way: the receiving device adds itself to the holder set with the
content hash of what it received.

If a destination is offline, the transfer queues and completes when it returns.

---

## 5. Deleting

There is exactly one delete operation: **remove this copy from this device.**

It is *addressable* — any device can delete any copy, including copies of items it
does not itself hold. Standing on the iPhone, seeing an image on the Mac, you can
delete the Mac's copy. Since only the Mac can erase its own disk, this is sent as
a request; the Mac executes it and republishes its holder row. Until it does, the
UI says *"delete pending — waiting for macOS"* rather than showing a state that
isn't true yet.

Deleting a copy while other copies exist frees the space **immediately**. Nothing
can be lost, so there is nothing to protect against.

An item leaves the catalog only when its last copy is deleted.

---

## 6. Trash

Deleting the **last** copy is the only operation that can destroy data, so it is
the only one that goes through trash.

- The bytes stay on the deleting device for **30 days**.
- The item is marked trashed across the whole mesh and can be restored from any
  device.
- Restoring makes it live again where the bytes already are. It does not pull a
  copy to the device you restored from; do that separately if you want one.
- The deleting device does not get its space back during those 30 days. A
  **Delete permanently** option is offered prominently for when space is the point.
- After 30 days the holder purges the blocks and propagates a tombstone. Every
  device drops the catalog entry.

---

## 7. Name collisions

Two different files can want the same name in a shared namespace. When a send
would collide, the sender is prompted:

- **Keep both** — the incoming file is renamed with a numeric suffix
  (`example.txt` → `example 1.txt`). Two independent items, two holder sets.
- **Override** — the catalog entry now points at the new content. The previous
  content goes to trash under the same 30-day rule, so an accidental override is
  recoverable.

Re-sending an edited file to a device that already has it is the same operation:
it collides, and Override is how you refresh a stale copy. Updating a copy and
resolving a name clash are one mechanism, not two.

---

## 8. Platform surfaces

| | Desktop (macOS / Windows / Linux) | Mobile (iOS / Android) |
|---|---|---|
| Shared space | `~/LocalCloudSync/` in Finder / Explorer | In-app library |
| Add an item | Drop it in the folder | Share sheet or Add button |
| Browse the mesh | App window | App window |
| Delete a local copy | Drag out of the folder | Delete button |

**Invariant on desktop: the folder contains exactly what that device holds.**
Pulling an item materialises it in the folder; dragging it out deletes that copy.
There are no placeholder stubs, because a stub you can drag out reintroduces the
ambiguity this invariant removes.

Mobile has no folder watching. iOS cannot watch a user directory or run a
background daemon freely, and Android would need `MANAGE_EXTERNAL_STORAGE`. Mobile
uses an explicit `import_file()` entry point instead, and on iOS surfaces the
library through a Files.app provider extension. The filesystem watcher is compiled
for desktop only.

---

## 9. Deliberate non-goals

Each of these is a decision, not an omission.

- **No automatic propagation.** Copies never update themselves.
- **No conflict resolution.** It follows from the above: nothing merges, so nothing
  can conflict. No version vectors, no causality tracking, no convergence protocol.
- **No automatic replication or durability policy.** The engine never decides to
  place a copy somewhere. If the only holder is offline, the item shows as
  unavailable with a last-seen time rather than being fetched from elsewhere.
- **No internet reach.** Same network only.
- **No sync folder on mobile.**

---

## 10. Data model

```sql
devices(id, name, platform, cert_pem, paired_at, last_seen)
files(id, path, size, created_by, created_at, trashed_at, trashed_by)
blocks(id, size, is_present)
file_blocks(file_id, block_id, block_index)
file_holders(file_id, device_id, content_hash, received_at)
delete_requests(file_id, target_device, requested_by, requested_at)
tombstones(file_id, deleted_at, deleted_by)
```

`content_hash` is the hash of the ordered block-id list — a manifest hash. Blocks
are already content-addressed by SHA-256, so this is nearly free to compute and
makes staleness a comparison rather than a guess.

`file_holders` replaces the `files.pinned_devices` JSON array. A device may only
write rows where `device_id` is itself.

Paths are relative to the shared space. Desktop subfolders produce nested paths;
mobile treats the namespace as flat.

---

## 11. Protocol

All traffic is mutually-authenticated TLS between paired devices, with certificates
pinned at pairing time.

| Endpoint | Purpose |
|---|---|
| `GET /hello` | Identity and certificate, pairing only |
| `POST /pair_request` | Begin pairing, carries the 6-digit code |
| `POST /pair_confirm` | Complete the certificate exchange |
| `GET /catalog` | Full catalog including holder sets |
| `POST /push_metadata` | Announce an item being sent |
| `POST /push_block/{id}` | Transfer one block |
| `POST /finalize_file/{id}` | Assemble and claim a holder row |
| `GET /get_block/{id}` | Serve a block to a puller |
| `POST /request_delete` | Ask a device to drop its copy |

---

## 12. Worked example

```
0.  Pair Android, macOS, iPhone with a 6-digit code.

1.  Android shares image.jpg to macOS.
    holders: {Android, macOS}          ← Android keeps its copy
    Every paired device now sees the item and both holders.

2.  iPhone — which holds nothing — deletes the macOS copy.
    Request queued to macOS; macOS erases and republishes.
    holders: {Android}                 ← space freed immediately, no trash

3.  Android deletes its copy. This is the last one.
    holders: {} · TRASHED · 30 days · bytes retained by Android
    Restorable from any device.

4a. Day 12: restored from iPhone → holders: {Android}, live again.
4b. Day 30: Android purges, tombstone propagates, item gone everywhere.
```

---

## 13. Build order

1. ~~**Pairing** — 6-digit code protocol replacing trust-on-first-use.~~ **Done.**
2. ~~**Holder sets** — `file_holders` with per-copy content hashes.~~ **Done.**
3. **Push** — the collision prompt. `share_to(item, targets)` transfers, but a
   recipient that already has a different item at the same path rejects the
   metadata instead of offering Override or Keep both.
4. **Pull, delete, trash** — `/request_delete` with offline queueing, the
   30-day retention and its sweep, and restore.
5. **Platform surfaces** — folder invariant on desktop, `import_file()` on mobile.
6. **Performance and cleanup** — block size, batched transfer, catalog sync on
   peer discovery.

Steps 1 and 2 were load-bearing: everything after assumes trusted peers and a
truthful holder set.

Until step 4 lands, deleting the last copy of an item leaves the catalog entry
in place with an empty holder set rather than moving it to trash. Nothing is
silently destroyed, but nothing is recoverable either.

---

## 14. Known issues in the code as it stands

- **Pairing is not man-in-the-middle proof.** The proof binds the code to both
  certificates, which defeats eavesdropping and naive relaying, but an active
  attacker who captures a proof can brute-force six digits offline in
  milliseconds. The expiry and attempt cap bound *online* guessing only.
  Closing it properly needs a PAKE such as SPAKE2, where the code is never
  committed to. Acceptable on a home network; not on a hostile one.
- Block size is 4 KB (`storage.rs`) and each block moves in its own HTTP
  round-trip. A 1 GB file is 262,144 requests.
- Tombstones are dead code: the table and helpers exist, nothing writes or reads
  them, and no endpoint serves them. They come alive with trash in step 4.
- The desktop app never syncs the catalog on peer discovery; only the CLI does
  (`cli/src/main.rs`).
- File identity is path-based, so a rename is indistinguishable from
  delete-plus-create.
- Blocks are stored unencrypted at rest.
- A device's certificate is pinned for good; there is no rotation path, so a
  compromised key means unpairing and pairing again.

Fixed since the first draft: the committed `identity.json`, the two fail-open
TLS paths, trust-on-first-use in `sync_with_peer`, and the hardcoded
`/tmp/local_cloud_sync/` download path.
