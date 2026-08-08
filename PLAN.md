# Where we are

The engine is done and it has a consumer.

An Android app drives it through the Kotlin bindings. It pairs with a six-digit
code, names itself what the phone is called, brings files in from the picker or
the share sheet, opens them again or hands them to another app, shows which
devices hold what, and keeps syncing with the screen off behind a foreground
service. The engine and the bindings are built by an ordinary `./gradlew
assembleDebug`, and every part of that has been run on a phone.

**None of it has been run on two phones.** Every throughput figure and every
claim about discovery in this file still comes from processes talking to each
other on one machine over loopback. That is the next thing, it needs no code,
and it has been the next thing for a while.

`DESIGN.md` is the design and the reasoning. This is the status.

---

## What exists

| | |
|---|---|
| `engine/` | 6,999 lines. The whole model, the server, discovery, storage, FFI. |
| `cli/` | 372 lines. A prompt for driving one device — the test harness. |
| `android/` | 3,868 lines of Kotlin across 22 files. Compose, three screens, a foreground service. |
| tests | 121 Rust across 7 suites, ~3s. One `#[ignore]`d because it waits out a 30s interval. 14 Kotlin: the background notification's line, and names arriving from other apps. |
| dependencies | 304, all on current stable releases. Toolchain pinned to 1.97.1. |

Test suites, and what each is for:

- **`localcloud` (57)** — units: chunking, hashing, the database and its
  migrations, pairing proofs, collision tie-breaks, address ranking, and what a
  device is allowed to call itself.
- **`indexing` (27)** — the `Indexer` driven directly: collisions, renames on
  disk, deletion, trash and its retention.
- **`sync_e2e` (10)** — two live devices over real mutually authenticated TLS,
  exchanging catalogs.
- **`api_errors` (11)** — which misuse produces which typed error. The contract
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
- **Files go both ways.** Open a received file, pass it to another app, or
  accept one from the share sheet — §8's "share sheet or Add button", which
  until now was only the button.
- **Syncing with the app closed**, behind a switch that is off until asked for.
  A foreground service holds the engine up with no screen on, and the engine
  runs while anything needs it rather than while a screen is open.
- **A device can be named.** `set_device_name` crosses the FFI, so a platform
  that knows better than Rust does can say so. The name is the one mutable part
  of an identity, so it is the one part behind a lock: the running server shares
  it rather than holding a copy, and renaming re-announces over mDNS instead of
  waiting for a restart. Renaming does not mint a new identity, which is what
  the test asserts.

Bugs in the engine, every one found by a test written for something else:

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

## What to do next

In order. The first one is not optional and everything below it is worth less
until it is done.

### 1. Two devices on one Wi-Fi

The Mac running `cli`, the phone running the app. Nothing blocks it and no code
is needed.

```
# on the Mac
cargo run -p cli
> devices                     # the phone should appear
> pair <id>                   # shows a 6-digit code
# on the phone: Devices → Pair → type the code
> import ~/some/video.mp4
> share video.mp4 <id>        # watch the progress lines
> ls                          # both devices listed as holders
```

Then compare checksums, and repeat it with the phone locked and background
syncing on — that path is the one nothing has ever exercised across a network.

**What it is there to find**, all of it still unproven:

1. **Discovery across machines.** mDNS has only ever run as two processes on one
   host. Still the single highest-risk assumption in the project, and the one
   that has already produced one bug — advertising on the wrong interface.
2. **Real-network throughput.** Every figure in this file is loopback, which has
   no round-trip latency. Latency is the exact thing 1 MiB blocks and eight in
   flight exist to hide, so these numbers are the ones that have never been
   tested against the problem they solve.
3. **Whether 30 seconds is tolerable** in practice, or whether the change
   notification in §14 stops being optional.

### 2. iOS

The Swift bindings have existed since `1152c24` and nothing has ever compiled
against them. Android was the first consumer and found three things in an
afternoon; the second will find more.

Start the multicast entitlement early — `com.apple.developer.networking.multicast`
is granted by Apple on request, not by ticking a box, and without it iOS
discovery finds nothing in exactly the way Android did without its lock.

### 3. Before anyone else runs this

Not needed to keep building, needed before it leaves your own network:

- **SPAKE2 pairing.** Six digits are brute-forceable offline from a captured
  proof. Fine at home, not on a shared network.
- **Blocks encrypted at rest**, and a certificate rotation path — there is still
  none, and the remedy for an exposed key is still to unpair everything.
- **Release build.** `optimization { enable = false }`, no signing config, and a
  48 MB debug APK carrying two ABIs. An app bundle and per-ABI splits are the
  fix, and none of it matters until 1 is done.

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
- **An Android device called itself "Unknown".** `whoami::devicename()` does not
  fail there — it succeeds, with that literal string, so the "Unnamed Android"
  fallback never fired. Fixed on both sides: placeholder names are now treated
  as the absence they are, and `set_device_name` is exposed, so the app hands
  the engine the name Android actually knows. See below.

### Syncing with the app closed

A `dataSync` foreground service, off by default and switched on from the This
device card. While it runs, this phone stays on the mesh, keeps its multicast
lock, and accepts files with no screen on — verified on an emulator by pressing
Home and watching the engine stay up.

The engine no longer starts and stops with the process lifecycle. It runs while
*anything* needs it to — an open screen, the service, or both — because those
overlap constantly, and a service starting a moment before the app is
backgrounded must not lose the engine to the departing screen.

Three limits worth knowing before relying on it:

- **Android 15 caps a `dataSync` service at roughly six hours a day.** When the
  six are up the system calls `onTimeout`, and a service that does not stop is
  treated as misbehaving. This one stops, turns the switch off, and leaves a
  notification saying why — the failure to avoid is a device that quietly
  stopped syncing hours ago and never said.
- **The notification is not optional**, and the switch will not turn on without
  permission to post it. Android would run the service anyway and simply not
  show it, which is the one outcome worth refusing: syncing with the screen off
  and nothing anywhere admitting it.
- **Nothing restarts it after a reboot.** Android 15 does not allow a `dataSync`
  service to be started from `BOOT_COMPLETED`, so the switch stays on and the
  service comes back the next time the app is opened. Deliberate, not missing.

### Files go both ways now

A file can be opened, passed to another app, and accepted from the share sheet —
so the app is usable by someone who is not testing it. Verified on an emulator
by importing a PNG, opening it in Google Photos, sharing it back into LocalCloud
from the system share sheet, and watching the engine number the second copy
`mesh-test 1.png` under the §7 collision rule.

Received files moved from `noBackupFilesDir` to `filesDir/sync` to make that
possible: a `FileProvider` has no vocabulary for `no_backup`, and the
alternatives were naming the private data path literally or copying every file
to the cache before handing it out. Backup exclusion is now a rule rather than a
side effect of location, which is better because the reasoning is written down.

Still not possible: surviving a reboot with background syncing on. That is
Android's rule, not an omission.

### Bugs in the app, every one found by running it

The engine's bugs were all found by tests. None of the app's were, and none of
them could have been:

- **Every catalog row was as wide as its filename.** A Card sizes to its
  content. It had been wrong since the screen was written and never showed,
  because every screenshot until then was of an empty catalog.
- **No row had ever expanded.** `Modifier.clickable` on a Card is swallowed by
  the Material Surface's own pointer handler, which sits inside the modifier
  passed in and consumes the tap first. No error, no crash — the row simply did
  nothing, which is how it survived being written and reviewed.

The lesson is cheap to state and was expensive to learn twice: a compiling
Compose screen is not a working one, and a screenshot of the state you already
had proves nothing about the state you have not reached.

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
