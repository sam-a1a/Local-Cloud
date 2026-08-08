# Where we are

The engine is done, and it now has a consumer. An Android app drives it through
the Kotlin bindings, builds the engine and those bindings as part of an ordinary
Gradle build, and runs — the engine starts on a phone, mints an identity, and
reports itself discoverable. What is left is the network: two physical devices
on one Wi-Fi, finding each other.

`DESIGN.md` is the design and the reasoning. This is the status.

---

## What exists

| | |
|---|---|
| `engine/` | 6,760 lines. The whole model, the server, discovery, storage, FFI. |
| `cli/` | 372 lines. A prompt for driving one device — the test harness. |
| `android/` | The app. Compose, three screens, every one of them the engine's own idea. |
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
  every 30s. It used to be the caller's job, and only `cli/` remembered.
- **Transfer progress**, per block, counting what actually has to move.
- **Typed errors and pushed events.** Failures name the item and the device.
- **Mobile.** `import_file`, the watcher gated to desktop, `cargo check --target
  aarch64-apple-ios` clean.
- **Swift and Kotlin bindings** via uniffi, generated from the compiled library.
- **A CLI that can pair**, which is what makes the device test possible at all.
- **An Android app**, in `android/`, which is the first thing to consume those
  bindings — and the first proof the engine runs anywhere but a desktop.

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

One of the two can now be the phone. The Android multicast lock is written and
held while the app is in the foreground, so the Android half of "mobile
discovery is a separate question" has been answered as far as it can be without
a second machine. iOS still needs its entitlement, and still has no app.

---

## The app

`android/` exists and runs. Three screens — the catalog and who holds what,
the mesh and how devices join it, the thirty days before a deleted item is
gone — and no fourth screen for anything the engine does not have an opinion
about.

`./gradlew assembleDebug` cross-compiles the engine for arm64-v8a and x86_64,
generates the Kotlin from the library it just built, and packages both. Neither
is checked in. Verified on an emulator: the app launches, the engine constructs,
mints an identity, opens its database and reports itself running.

What the first consumer found, which is what a first consumer is for:

- **The engine had never been built for Android.** It compiles clean, first
  try. §8 was a real boundary rather than a stated one.
- **Typed errors keep their variants across the FFI but lose their sentences.**
  uniffi generates `message` from a variant's fields, so `NotVisible` arrives in
  Kotlin as `deviceId=7f3a…` rather than as "That device is not visible on the
  network". The variants were the point and they survive; the app supplies the
  English. Worth knowing before writing the iOS one.
- **An Android device is called "Unknown".** `whoami::devicename()` does not
  fail there, it succeeds with that literal string, so the "Unnamed Android"
  fallback never fires. `DeviceIdentity::set_device_name` exists and is not
  exposed, and exposing it is not a one-liner: `identity` is a plain field on
  `Engine`, read by `start`, pairing and discovery, so making it settable means
  putting it behind a lock. The alternative is for the engine to read
  `ro.product.model` itself. **Undecided, and it blocks nothing but reads badly
  on the one screen where devices are named.**

Still app-side rather than engine-side: an iOS Files provider extension, the iOS
multicast entitlement, and a foreground service if Android is ever to sync while
the app is closed. The app runs the engine only in the foreground today.

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
