/**
 * The wire format, which is `web/src/snapshot.rs` and nothing else.
 *
 * Written out by hand rather than generated. It is one file, it changes when
 * the Rust changes, and a generator would be a build step to explain for the
 * sake of eighty lines.
 */

export type Holder = {
  deviceId: string
  name: string
  isThisDevice: boolean
  reachable: boolean
}

export type Item = {
  id: string
  name: string
  size: number
  /** Whether this device has the bytes, which decides what the row can offer. */
  heldHere: boolean
  holders: Holder[]
  modified: number
}

export type TrashItem = {
  id: string
  name: string
  size: number
  secondsRemaining: number | null
  trashedBy: string
}

export type Peer = {
  deviceId: string
  name: string
  platform: string
}

export type PairedPeer = Peer & { reachable: boolean }

export type Collision = {
  id: string
  requested: string
  keptAs: string
}

export type ThisDevice = {
  id: string
  shortId: string
  name: string
  platform: string
  running: boolean
  syncDir: string
}

export type Snapshot = {
  device: ThisDevice
  items: Item[]
  trash: TrashItem[]
  /** Devices on the network that are nobody yet. Paired ones are filtered out. */
  visible: Peer[]
  paired: PairedPeer[]
  offers: Peer[]
  collisions: Collision[]
  deferredDeletes: number
}

/** A copy on the move. Keyed by item *and* peer: one bar per destination. */
export type Progress = {
  fileId: string
  deviceId: string | null
  sending: boolean
  done: number
  total: number
}

export type Notice = {
  text: string
  failure: boolean
  /** Ours, not the server's — it exists so a later notice can clear an earlier
   *  one's timer without clearing itself. */
  id: number
}

/** A directory on the machine the engine is on, for choosing a sync folder. */
export type Folder = { name: string; path: string }

export type Listing = {
  path: string
  /** Absent at the root, which is the only place with nowhere above it. */
  parent: string | null
  folders: Folder[]
  /** Files already here. Choosing this folder puts them in the mesh. */
  files: number
}

export type ChooseOutcome = {
  syncDir: string
  moved: number
  leftBehind: number
  adopted: number
}
