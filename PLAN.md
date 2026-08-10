# Where we are

The engine is done and it has three consumers.

An Android app drives it through the Kotlin bindings. It pairs with a six-digit
code, names itself what the phone is called, brings files in from the picker or
the share sheet, opens them again or hands them to another app, shows which
devices hold what, and keeps syncing with the screen off behind a foreground
service. The engine and the bindings are built by an ordinary `./gradlew
assembleDebug`, and every part of that has been run on a phone.

A Mac app drives it through the Swift ones. Same three screens, built by an
ordinary Cmd-B, and it found in an afternoon what the first consumer could not:
**the engine's TLS server had never worked outside a test.** Every suite and the
CLI installed a rustls crypto provider by hand, an app binding through the FFI
has no way to, and without one the server panicked on a background thread while
the engine went on reporting itself running. Fixed, and the workaround is gone
from everywhere, so the tests now exercise the path an app takes.

A web app drives it from a browser, which cannot join a mesh — so `web/` runs an
engine of its own and puts a loopback-only API in front of it. The page is a
view of that device rather than a peer, and nothing about the protocol had to
move to allow it.

**It works between two real devices.** A Galaxy S24 on Wi-Fi and this Mac
found each other over mDNS, paired with a six-digit code, and a photo taken on
the phone is now held by both — the phone that created it and the Mac that
asked for a copy, over the network rather than over loopback. The oldest open
question in this file is answered, and answered yes.

That leaves the things that only show up at size. Every throughput figure here
is still loopback, the largest thing to cross the network so far is 124 kB, and
the locked-phone path has never been exercised across one.

`DESIGN.md` is the design and the reasoning. This is the status.

---

## What exists

| | |
|---|---|
| `engine/` | 7,037 lines. The whole model, the server, discovery, storage, FFI. |
| `cli/` | 370 lines. A prompt for driving one device — the test harness. |
| `android/` | 3,868 lines of Kotlin across 22 files. Compose, three screens, a foreground service. |
| `macos/` | 1,590 lines of Swift across 11 files. SwiftUI, the same three screens. |
| `web/` | 1,217 lines of Rust — an engine host and a loopback API — and 968 of TypeScript. Solid, Tailwind, the same three screens. |
| tests | 125 Rust across 7 suites, ~4s. One `#[ignore]`d because it waits out a 30s interval. 14 Kotlin: the background notification's line, and names arriving from other apps. |
| dependencies | one fewer provider than it had. Toolchain pinned to 1.97.1. |

Test suites, and what each is for:

- **`localcloud` (57)** — units: chunking, hashing, the database and its
  migrations, pairing proofs, collision tie-breaks, address ranking, and what a
  device is allowed to call itself.
- **`indexing` (30)** — the `Indexer` driven directly: collisions, renames on
  disk, deletion, trash and its retention, and what a scan of an existing
  folder finds.
- **`sync_e2e` (10)** — two live devices over real mutually authenticated TLS,
  exchanging catalogs.
- **`api_errors` (11)** — which misuse produces which typed error. The contract
  an app binds against.
- **`pairing_e2e` (9)** — the 6-digit exchange over a real listener, plus what a
  paired device is still not allowed to do.
- **`events` (5)** — how events reach an application.
- **`mesh_e2e` (3 + 1 ignored)** — two whole engines: mDNS, pairing, a catalog
  converging with nobody calling sync — and that an engine arranges its own TLS
  before its server needs it, which for a long time it did not.

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
- **A Mac app**, in `macos/`, the second consumer and the first on Apple's side
  of the FFI. It found the TLS bug above on its first run.
- **A web consumer**, in `web/`, which is a process running an engine with a
  loopback API in front of it, because a browser cannot join a mesh. It is the
  consumer the phone was finally tested against.
- **A folder you choose.** The sync folder used to be wherever the process
  decided. The page now browses this machine a directory at a time and sends
  back the one it settled on — and because the catalog records paths relative to
  that folder, changing it is one operation rather than a setting: the engine
  stops, the files come across, a new engine opens on the new folder, and the
  choice is written down for the next run. `Engine::scan_sync_folder` goes with
  it, because a folder that already has things in it must not look empty.
- **A device can be named.** `set_device_name` crosses the FFI, so a platform
  that knows better than Rust does can say so. The name is the one mutable part
  of an identity, so it is the one part behind a lock: the running server shares
  it rather than holding a copy, and renaming re-announces over mDNS instead of
  waiting for a restart. Renaming does not mint a new identity, which is what
  the test asserts.

Bugs in the engine. Every one until the last two was found by a test written for
something else; those two were found by an app, which is what apps are for:

- A file whose blocks repeated **arrived corrupt** — `file_blocks` was keyed on
  the block rather than its position, so a 3 MB file with a run of zeros became
  1 MB on every device it was sent to, while the sender's copy looked perfect.
- **A paired device could read or overwrite any file** the process could reach —
  block ids went into a path unvalidated. Confirmed by watching a peer fetch the
  device's private key, 200 OK.
- **`Engine::start` panicked outside a tokio runtime**, which is precisely how an
  FFI binding calls it.
- **A restarted engine stopped syncing** — the normal path on mobile.
- **The TLS server never started, anywhere but a test.** rustls will not choose
  between two crypto providers and panics rather than guess; asking for `ring`
  while leaving default features on quietly enabled aws-lc-rs as well. The panic
  landed on a tokio worker inside `start`, where nothing was waiting on the
  result, so the engine reported itself running with no server behind it — and a
  device advertising itself perfectly well would have failed every transfer.
  Every test suite and the CLI had installed a provider by hand, and an app
  binding through the FFI cannot: `rustls::crypto::ring` does not cross into
  Swift or Kotlin. The engine does it itself now, the manual calls are gone, and
  only one provider is compiled in at all.
- **Opening the sync folder put `.DS_Store` in the catalog.** `import_file` has
  refused a name beginning with a dot since it was written; the folder had no
  such rule, and on a desktop the folder is the way in. It would have replicated
  to the phones, which have no idea what it is.

---

## What to do next

The one that gated everything else is done. What is left is what it did not
reach: size, sleep, and everything in §3 before this leaves your own network.

### ~~1. Two devices on one Wi-Fi~~ — done

A Galaxy S24 running the Android app and this Mac running the browser. They
found each other over mDNS on the first try, paired with the six digits, and a
photo the phone had made was afterwards held by both devices. Three things that
had never happened before happened at once:

- **Discovery across machines.** The phone advertised
  `https://192.168.1.8:41151`, the Mac saw it as "Sam S25 Plus · Android" within
  seconds, and the phone's TLS server answered `/ping` from the Mac. That was
  the single highest-risk assumption in the project and it is no longer an
  assumption.
- **Pairing across machines**, over a real network rather than a loopback
  listener.
- **A file crossing between two devices** rather than two processes.

It is also the proof that the crypto fix was exactly what stood between here and
a working mesh: the same phone, before the rebuild, advertised itself perfectly
and answered nothing.

What the run did *not* settle, because a 124 kB photo does not ask the question:

1. **Real-network throughput.** Every figure in this file is loopback, which has
   no round-trip latency. Latency is the exact thing 1 MiB blocks and eight in
   flight exist to hide, so these numbers are still the ones that have never been
   tested against the problem they solve. Send a video next.
2. **Whether 30 seconds is tolerable** in practice, or whether the change
   notification in §14 stops being optional.
3. **The locked phone.** Background syncing has been watched on an emulator with
   the screen off, never across a network with a real device asleep.

### 2. iOS

The Swift bindings now have a consumer, so the language is proven and the shape
of an app around it is written down in `macos/`. What iOS adds is a second
platform for the same Swift: a different sandbox, no folder to watch, no window
that can be left open, and a discovery story that is Apple's rather than the
engine's.

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

## The Mac app

`macos/` exists and runs. The same three screens the phone has, in SwiftUI, and
no fourth one — the engine did not grow an opinion by being called from a
different language.

`build-engine.sh` runs as a build phase: it cross-compiles the engine for
whatever architectures Xcode asked for, `lipo`s them into one static library,
and generates the Swift from the library it just built. Neither is checked in,
for the same reason neither is on Android. The integration is one rename —
uniffi writes `localcloudFFI.modulemap` and Clang only looks for
`module.modulemap` — so there is no bridging header and the C stays in its own
module.

Verified by running it, sandboxed: the engine constructs, mints an identity,
opens its database, binds its TLS server and answers `/ping`, and a `cli`
instance beside it discovers "Sam's MacBook Air" over mDNS at the Wi-Fi address
rather than only at loopback.

What the second consumer found:

- **The TLS server had never started outside a test**, which is the entry above
  and the whole argument for writing a second consumer.
- **`.DS_Store` in the catalog**, likewise.
- **Xcode 26's `MainActor` default isolation applies to the generated
  bindings.** So the compiler decides `Engine.start()` belongs on the main actor
  and then warns about the one place that correctly refuses to put it there — an
  app that believed it would block the thread drawing its own window on every
  import. The target sets `SWIFT_DEFAULT_ACTOR_ISOLATION` to `nonisolated` and
  the app's own types say `@MainActor` where they mean it. Worth knowing before
  writing the iOS one.
- **The typed errors lose their sentences here too, and more bluntly.** uniffi
  generates `errorDescription` as `String(reflecting: self)`, so `NotVisible`
  arrives as `localcloud.EngineError.NotVisible(deviceId: "7f3a…")`. Same
  remedy: match the variant, supply the English.

Two things the Mac gets that the phone did not:

- **It knows its own name.** `whoami` reads the computer name through
  SystemConfiguration, so nothing has to tell the engine what this device is
  called — the opposite of Android, which insisted it was "Unknown". It also
  means the app links SystemConfiguration, which is the one thing the Mac needed
  at the link line that Android did not.
- **The sync folder is a real folder.** `#[cfg(desktop)]` means the engine
  watches it, so a file put there in Finder is indexed and offered to the mesh
  with the app not involved at all. Dropping a file on the window is the Mac's
  share sheet and reaches the same `import_file` the button does.

Still not possible: syncing with the window closed. Android earned that with a
foreground service and a notification; the Mac's version is a menu bar item, and
until there is one, closing the window quits — because a window that closed into
nothing would be a device that quietly stopped syncing and never said.

---

## The web app

`web/` exists and runs, and it is the one consumer that had to answer a question
the other two did not: a browser cannot join the mesh. Mutual TLS against
certificates pinned by pairing, on a port found over mDNS, is three things a
browser will not do — and each of them is the point of the protocol rather than
an obstacle in it.

So the process is the mesh member and the page is a view of it. `web/src` runs a
whole engine and serves a loopback-only API beside it; `web/ui` is Solid,
TypeScript and Tailwind, built by Vite and read into memory once at startup.
Closing the tab does not stop the device; closing the terminal does. That is
the first consumer where those are different things, and it is also, by
accident, the first one that syncs with nothing on screen.

Loopback is the security boundary. The API unpairs devices, deletes copies off
other machines and hands back the contents of any file in the catalog, with no
authentication at all — it is safe because the operating system will not route
it off this machine, and for no other reason. Exposing it would mean designing
authentication, which is a project rather than a flag.

What it costs, since a page is the easiest place to be wasteful:

- **One `EventSource`, no polling.** The server compares each snapshot to the
  last one it sent and says nothing when nothing changed, so a quiet mesh is a
  silent socket.
- **Block progress never touches the catalog.** A gigabyte produces a thousand
  progress events and not one of them changes an item, so they go straight to
  the bar.
- **`reconcile` rather than replace.** A whole snapshot arriving updates the
  signals that differ and leaves the rest of the DOM alone.
- **Nothing buffers a file.** Uploads stream from the browser to a temporary
  file to `import_file`; downloads stream off disk.
- 16 kB of JavaScript and 5 kB of CSS, gzipped.

It is also where the sync folder stopped being a decision the process made. The
page browses this machine and chooses one; the engine is stopped, the files are
carried across, and a new engine opens on the other side of the move. Two things
are said on screen before the button is, because clicking again does not undo
either: files already in the chosen folder join the mesh, and files in the
current one come with you. Nothing at the destination is overwritten — a name
already taken leaves that file where it was and says how many.

Verified against a second engine on the same machine: paired over the real mDNS
and mTLS path, sent a 3 MB file, and the checksum on the far side matches — as
does the byte-for-byte round trip back out through the download route. The
folder change is verified the same way: a file carried across, one already at
the destination adopted into the catalog, one name clash left alone, every guard
refused, and the choice still in force after a restart.

Not verified: how the page looks in a browser. Every request it makes has been
exercised with `curl`, and the bundle typechecks and builds, but nothing here
has rendered it — Chrome headless would not cooperate on this machine. You have
since used it, so this is now the weakest sentence in the file rather than a
real gap; the Compose lesson above is the reason it is still here at all.

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
