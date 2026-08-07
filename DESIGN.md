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

- The bytes stay on the deleting device for **30 days**, and so does its holder
  row. This is not an oversight: an item nobody holds could not be restored.
- The item is marked trashed across the whole mesh and can be restored from any
  device.
- Restoring makes it live again where the bytes already are. It does not pull a
  copy to the device you restored from; do that separately if you want one.
- The deleting device does not get its space back during those 30 days. A
  **Delete permanently** option is offered prominently for when space is the point.
- After 30 days the holder releases the blocks and propagates a tombstone. Every
  device drops the catalog entry.

An hourly sweep applies the retention, and runs once at startup so trash that
expired while a device was switched off is still cleared. Only blocks nothing
else references are released — blocks are shared by content, so two identical
files hold the same ones.

---

## 7. Name collisions

A name is owned by exactly one live item across the whole mesh, so two different
files can want the same one:

- **Keep both** — the incoming file gets a numeric suffix (`example.txt` →
  `example 1.txt`). Two independent items, two holder sets.
- **Override** — the incoming item takes the name and the previous one goes to
  trash, so an accidental override is recoverable.

**Asking cannot block indexing.** The watcher runs in the background, and on
mobile the app may not even be foregrounded, so a prompt that had to be answered
before the file could be recorded would either stall or drop it. Keep both is
therefore applied immediately — it cannot lose anything — and the conflict is
surfaced with Override offered as a follow-up. The user still sees a prompt with
two buttons; the difference is that ignoring it destroys nothing.

Override is atomic: the previous owner goes to trash and the incoming item takes
the name, or neither happens. Half of it would leave an item trashed for a name
it never received.

**Editing versus colliding** is decided by whether this device already holds the
item at that path. If it does, the file was edited. If another device's item owns
the name, this is a different file that happens to share it.

**Two devices that were apart** can each create an item with the same name, and a
sync cannot stop to ask. There the tie-break is the item id: the smaller one
keeps the name. Both devices hold both ids, so each reaches the same answer alone
and they converge with no extra round trip. It is still recorded as a conflict,
so Override remains available afterwards.

Whenever an item this device holds is moved aside, the sync folder is renamed
with it — the folder and the catalog must never disagree about what a file is
called.

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
file_blocks(file_id, block_id, block_index)   -- keyed on (file_id, block_index)
file_holders(file_id, device_id, content_hash, received_at)
delete_requests(file_id, target_device, requested_by, requested_at)
tombstones(file_id, deleted_at, deleted_by)
```

`content_hash` is the hash of the ordered block-id list — a manifest hash. Blocks
are already content-addressed by SHA-256, so this is nearly free to compute and
makes staleness a comparison rather than a guess.

A manifest is an ordered list of *positions*, which is why `file_blocks` is keyed
on `(file_id, block_index)` rather than on the block. The same block legitimately
appears many times in one file — any run of zeros or repeated padding produces
one — and keying on content collapsed those repeats, so the file reassembled short
on every device it was sent to while the sender's own copy looked untouched.

Blocks are 1 MiB (`storage::BLOCK_SIZE`). The size is a transfer decision more
than a storage one: each block costs its own request, so the original 4 KiB made
a 1 GB file 262,144 round-trips. Chunking is fixed-size, so blocks only ever
dedup against byte-identical, identically-aligned content; the smaller block
bought finer sharing only for a file edited without any bytes shifting, and cost
two database rows and a request for every 4 KiB of every file.

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

The catalog carries four things: items, holder rows, outstanding delete requests
and tombstones. The last two are what make deletion survive a device being away —
a request reaches a target that was offline, and a tombstone stops a device that
missed a destruction from handing the item back.

Blocks are verified on arrival, pulled or pushed: a block id *is* the SHA-256 of
its contents, so one that does not hash to its own id is refused rather than
assembled into a file. An id that is not a hash at all is refused too — ids
arrive in a URL path and are joined onto the storage directory, so without that
check a paired device could name a block `../../identity.json` and read or
overwrite anything the process can reach.

Blocks move several at a time (`discovery::TRANSFER_CONCURRENCY`). A request
spends most of its life waiting rather than moving bytes, so sending the next
only once the last has returned leaves the link idle; overlapping a handful is
what turns the larger block size into throughput.

Keeping the catalog true is the engine's own job, not the caller's. It reads a
peer's catalog when that peer is discovered, when pairing with it completes, and
then every `CATALOG_SYNC_INTERVAL_SECS`. Both immediate triggers matter: which
of discovery and pairing happened last would otherwise decide whether anything
appeared before the next scheduled pass.

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
3. ~~**Push** — the collision prompt, and the trash storage Override needs.~~
   **Done.**
4. ~~**Pull, delete, trash** — `/request_delete` with offline queueing, the
   30-day retention and its sweep, permanent delete and restore.~~ **Done.**
5. **Platform surfaces** — folder invariant on desktop, `import_file()` on mobile,
   the watcher compiled desktop-only, an iOS Files provider extension.

   There is no UI at present. The Dioxus app was removed: it was one renderer's
   idea of the model, and it was shaping the engine's API before the engine had
   settled. `cli/` drives the engine directly and is the only consumer. A UI
   comes back once the platform surfaces below exist to build it on, and the
   engine's public API is what it should be answerable to.
6. ~~**Block size and transfer**~~ — 1 MiB blocks, several in flight at once,
   and superseded blocks released when a file is re-chunked. **Done.**
7. **Performance and cleanup** — sending only the blocks a recipient lacks.

Steps 1 and 2 were load-bearing: everything after assumes trusted peers and a
truthful holder set.

The model in sections 1–7 is complete. What remains is reach — making it usable
from each platform's own idioms — and making large files fast.

---

## 14. Known issues in the code as it stands

- **Pairing is not man-in-the-middle proof.** The proof binds the code to both
  certificates, which defeats eavesdropping and naive relaying, but an active
  attacker who captures a proof can brute-force six digits offline in
  milliseconds. The expiry and attempt cap bound *online* guessing only.
  Closing it properly needs a PAKE such as SPAKE2, where the code is never
  committed to. Acceptable on a home network; not on a hostile one.
- Sharing sends every block of an item, including ones the recipient already
  has. Blocks are content-addressed, so the recipient could be asked which of a
  manifest it is missing and only those sent; re-sending a large file that has
  barely changed currently costs as much as sending it the first time.
- Transfer throughput has only been measured over loopback. The block size and
  overlapping requests are both aimed at network round-trips, which loopback
  does not have, so the real-network figures are unproven.
- Discovery has only been exercised with two instances on one machine. It has
  never run across two physical devices, or on Android or iOS, where multicast
  needs an entitlement (iOS) and a multicast lock (Android).
- A change takes up to `CATALOG_SYNC_INTERVAL_SECS` (30s) to reach another
  device. Catalog replication is pull-only, so nothing tells a peer that
  something has changed and the periodic pass is the only thing that carries it.
  Discovery and pairing both prompt an immediate read, so the delay only applies
  in steady state — but that is the common case. Closing it means either polling
  harder, which fetches the whole catalog each time, or a notification carrying
  no content that says "there is something new to read". The latter is the real
  fix and is a protocol addition, not a tuning change.
- File identity is path-based at creation, so renaming a file in the folder reads
  as delete-plus-create rather than a rename.
- Blocks are stored unencrypted at rest.
- A device's certificate is pinned for good; there is no rotation path, so a
  compromised key means unpairing and pairing again.
- Tombstones are never garbage-collected. They are one small row per destroyed
  item and are kept deliberately, since dropping one lets a device that was away
  long enough reintroduce what it names. Worth revisiting only if the count ever
  becomes a problem.
- A delete request aimed at a device that never returns waits forever. It costs
  one row and is visible in the app, which seems better than expiring it and
  silently leaving the copy in place.
- Concurrent edits of the same item on two devices both set the item's content
  hash, and the last catalog sync wins for that field. Each holder row still
  records truthfully what that device has, so nothing is lost and the divergence
  is visible — but the catalog's idea of "current" is last-writer-wins.

Fixed along the way: the committed `identity.json`, the two fail-open TLS paths,
trust-on-first-use in `sync_with_peer`, the hardcoded `/tmp/local_cloud_sync/`
download path, deleting blocks that another item still needed, pushes being
refused by a stale catalog, and unverified block contents.
