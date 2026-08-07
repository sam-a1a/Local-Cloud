# Where we are

The engine is done. Nothing in DESIGN.md §13 is outstanding, it builds for iOS,
and it emits Swift and Kotlin bindings. The next thing to do is run it on two
physical machines; the thing after that is an app.

`DESIGN.md` is the design and the reasoning. This is the status.

---

## What exists

| | |
|---|---|
| `Engine/` | 6,760 lines. The whole model, the server, discovery, storage, FFI. |
| `Cli/` | 372 lines. A prompt for driving one device — the test harness. |
| tests | 115 across 6 suites, ~3s. One `#[ignore]`d because it waits out a 30s interval. |
| dependencies | 304, all on current stable releases. Toolchain pinned to 1.97.1. |

Test suites, and what each is for:

- **`localcloud` (52)** — units: chunking, hashing, the database and its
  migrations, pairing proofs, collision tie-breaks, address ranking.
- **`indexing` (27)** — the `Indexer` driven directly: collisions, renames on
  disk, deletion, trash and its retention.
- **`sync_e2e` (10)** — two live devices over real mutually authenticated TLS,
  exchanging catalogs.
- **`api_errors` (10)** — which misuse produces which typed error. The contract
  an app binds against.
- **`pairing_e2e` (9)** — the 6-digit exchange over a real listener, plus what a
  paired device is still not allowed to do.
- **`events` (5)** — how events reach an application.
- **`mesh_e2e` (2 + 1 ignored)** — two whole engines: mDNS, pairing, a catalog
  converging with nobody calling sync.

---

## Done

Everything in §13. Pairing, holder sets, push, pull, delete, trash — and since:

- **1 MiB blocks, eight in flight.** 64 MB went from 5.5s to 0.35s over
  loopback; a gigabyte from roughly 90s to under 6.
- **Only missing blocks are sent.** `/push_metadata` replies with what the
  recipient lacks. Verified on two running instances: a re-share moved 0 blocks.
- **The engine keeps its own catalog true** — on discovery, on pairing, and
  every 30s. It used to be the caller's job, and only `Cli/` remembered.
- **Transfer progress**, per block, counting what actually has to move.
- **Typed errors and pushed events.** Failures name the item and the device.
- **Mobile.** `import_file`, the watcher gated to desktop, `cargo check --target
  aarch64-apple-ios` clean.
- **Swift and Kotlin bindings** via uniffi, generated from the compiled library.
- **A CLI that can pair**, which is what makes the device test possible at all.

Bugs found and fixed along the way, each by a test written for something else:

- A file whose blocks repeated **arrived corrupt** — `file_blocks` was keyed on
  the block rather than its position, so a 3 MB file with a run of zeros became
  1 MB on every device it was sent to, while the sender's copy looked perfect.
- **A paired device could read or overwrite any file** the process could reach —
  block ids went into a path unvalidated. Confirmed by watching a peer fetch the
  device's private key, 200 OK.
- **`Engine::start` panicked outside a tokio runtime**, which is precisely how an
  FFI binding calls it.
- **A restarted engine stopped syncing** — the normal path on mobile.

---

## Next: two physical devices

Nothing blocks this. Build the CLI on two machines on the same network, point
each at its own directory, and:

```
> devices                     # each should see the other
> pair <id>                   # shows a 6-digit code
> accept <id> <code>          # on the other machine
> import ~/some/video.mp4
> share video.mp4 <id>        # watch the progress lines
> ls                          # both devices listed as holders
```

Then compare checksums. This is exactly the flow already driven between two
instances on one machine, so what is being tested is the network, not the logic.

**What it is there to find**, all of it currently unproven:

1. **Discovery across machines.** It has only ever run as two processes on one
   host. This is the single highest-risk assumption in the project.
2. **Real-network throughput.** Every figure so far is loopback, which has no
   round-trip latency — the exact thing the block size and concurrency address.
3. **Whether a 30s catalog delay is tolerable** in practice, or wants the
   notification described in §14.

Mobile discovery is a separate question and cannot be answered here: iOS
multicast needs an entitlement and Android a multicast lock, and both live in an
app project.

---

## Then: the app

The engine's API is settled and deliberately small — 32 methods, and `cargo doc`
shows only them. Errors are typed and carry their ids, events are pushed to a
listener, and no internal type crosses the boundary. §10a of DESIGN.md records
the rules it is held to.

Two things are still app-side rather than engine-side: an iOS Files provider
extension, and the multicast entitlement and lock.

I would build one platform end to end before starting a second. The bindings are
generated but nothing has consumed them yet, and the first consumer always finds
something.

---

## Deliberately not done

From §14. None block an app; each is a decision rather than an oversight.

- **Pairing is not MITM-proof.** An attacker who captures a proof can brute-force
  six digits offline. Fine on a home network, not on a hostile one. SPAKE2 is the
  fix and is a project of its own.
- **A change takes up to 30s to reach another device** in steady state.
  Replication is pull-only, so nothing announces a change. The real fix is a
  contentless "there is something to read" notification — a protocol addition.
- **Renames read as delete-plus-create**, because identity is path-based at
  creation.
- **Blocks are unencrypted at rest.**
- **No certificate rotation.** A compromised key means unpairing and pairing
  again — which has already been done once, see below.
- **Concurrent edits on two devices**: last writer wins for the item's content
  hash. Each holder row still records truthfully what that device has, so nothing
  is lost and the divergence is visible.

---

## One thing to know

The device key committed in `identity.json` at `ef9e0e2` is on a **public**
GitHub repository. It has been retired — the file is untracked and gitignored,
and this device minted a fresh identity — but the old key is still readable in
the history and must be treated as known to anyone. Nothing uses it.

There is no rotation path yet, so if a key is ever exposed again the remedy is
the same: delete `identity.json`, restart, and pair the devices again.
