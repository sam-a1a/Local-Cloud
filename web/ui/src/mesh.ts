import { createSignal } from 'solid-js'
import { createStore, produce, reconcile } from 'solid-js/store'
import type { Collision, Item, Notice, Progress, Snapshot, TrashItem } from './types'

/**
 * Everything the page knows, and every way it can change it.
 *
 * One module-level store rather than a context. There is one engine, one
 * connection to it, and one page; a provider would be ceremony around a
 * singleton.
 *
 * The whole state arrives as one object and is applied with `reconcile`, which
 * is the point of doing the join on the server. Solid then walks the two trees
 * and touches only the signals that actually differ, so a snapshot arriving
 * every two seconds with one changed byte updates one text node — not a list.
 */

const empty: Snapshot = {
  device: { id: '', shortId: '', name: '', platform: '', running: false, syncDir: '' },
  items: [],
  trash: [],
  visible: [],
  paired: [],
  offers: [],
  collisions: [],
  deferredDeletes: 0,
}

const [state, setState] = createStore<Snapshot>(empty)

/**
 * Kept apart from the snapshot on purpose.
 *
 * Block progress arrives up to a thousand times for one large file. Putting it
 * in the snapshot would mean re-reading the catalog for each one; keeping it
 * here means a transfer bar updates and nothing else on the page is touched.
 */
const [transfers, setTransfers] = createStore<Record<string, Progress>>({})

const [notice, setNotice] = createSignal<Notice | null>(null)
const [connected, setConnected] = createSignal(false)

let noticeCount = 0
let noticeTimer: ReturnType<typeof setTimeout> | undefined

export { state, transfers, notice, connected }

export const key = (fileId: string, deviceId: string | null) => `${fileId}@${deviceId ?? 'here'}`

export function say(text: string, failure = false) {
  noticeCount += 1
  setNotice({ text, failure, id: noticeCount })
  clearTimeout(noticeTimer)
  noticeTimer = setTimeout(() => setNotice(null), failure ? 9000 : 5000)
}

export const dismiss = () => setNotice(null)

// -- The connection ----------------------------------------------------------

/**
 * One EventSource for the life of the page.
 *
 * The browser reconnects it on its own, so there is no retry loop here — only
 * a flag, because a page that is drawing a two-minute-old catalog should say
 * so rather than look correct.
 */
export function connect() {
  const source = new EventSource('/api/events')

  source.addEventListener('open', () => setConnected(true))
  source.addEventListener('error', () => setConnected(false))

  source.addEventListener('state', (event) => {
    setConnected(true)
    setState(reconcile(JSON.parse((event as MessageEvent).data) as Snapshot, { key: 'id' }))
  })

  source.addEventListener('progress', (event) => {
    const progress = JSON.parse((event as MessageEvent).data) as Progress
    setTransfers(key(progress.fileId, progress.deviceId), progress)
  })

  source.addEventListener('done', (event) => {
    const { fileId, deviceId } = JSON.parse((event as MessageEvent).data)
    setTransfers(produce((all) => { delete all[key(fileId, deviceId ?? null)] }))
  })

  source.addEventListener('notice', (event) => {
    const { text, failure } = JSON.parse((event as MessageEvent).data)
    say(text, failure)
  })
}

// -- Asking it to do things --------------------------------------------------

/**
 * One place where a request is made, fails, and is reported.
 *
 * Returns null rather than throwing, because every caller is a click: there is
 * nothing to recover, only something to say. The engine writes a sentence for
 * every one of its failures and this side does not improve on it.
 */
async function post<T = unknown>(path: string, body?: unknown): Promise<T | null> {
  try {
    const response = await fetch(path, {
      method: 'POST',
      headers: body ? { 'content-type': 'application/json' } : undefined,
      body: body ? JSON.stringify(body) : undefined,
    })
    const payload = await response.json().catch(() => null)
    if (!response.ok) {
      say(payload?.error ?? `That did not work (${response.status}).`, true)
      return null
    }
    return payload as T
  } catch {
    say('The engine is not answering. Is it still running?', true)
    return null
  }
}

/**
 * Sends the file as the request body rather than as a form.
 *
 * A `File` is a stream, so nothing here holds the bytes: the browser reads it
 * from disk as the socket drains and the server writes it out the other side.
 * A multipart form would have both ends buffering a copy for no reason.
 */
export async function importFiles(files: FileList | File[]) {
  for (const file of Array.from(files)) {
    try {
      const response = await fetch(`/api/import?name=${encodeURIComponent(file.name)}`, {
        method: 'POST',
        body: file,
      })
      if (!response.ok) {
        const payload = await response.json().catch(() => null)
        say(payload?.error ?? `${file.name} could not be added.`, true)
      }
    } catch {
      say(`${file.name} could not be added.`, true)
    }
  }
}

export const share = (item: Item, deviceIds: string[]) =>
  post('/api/share', { fileId: item.id, deviceIds })

export const pull = (item: Item) => post('/api/pull', { fileId: item.id })

export async function deleteHere(item: Item) {
  const outcome = await post<{ remainingCopies: number; trashed: boolean }>(
    '/api/delete-here',
    { fileId: item.id },
  )
  if (!outcome) return
  say(
    outcome.trashed
      ? 'That was the last copy. It is in the trash for 30 days.'
      : `Deleted here. ${outcome.remainingCopies === 1 ? '1 copy' : `${outcome.remainingCopies} copies`} left elsewhere.`,
  )
}

export const deleteFrom = (item: Item, deviceId: string) =>
  post('/api/delete-copy', { fileId: item.id, deviceId })

export const startPairing = (deviceId: string) =>
  post<{ code: string }>('/api/pair/start', { deviceId })

export const confirmPairing = (deviceId: string, code: string) =>
  post('/api/pair/confirm', { deviceId, code })

export const cancelPairing = () => post('/api/pair/cancel')

export const unpair = (deviceId: string) => post('/api/unpair', { deviceId })

export const rename = (name: string) => post('/api/rename', { name })

export const restore = (item: TrashItem) => post('/api/trash/restore', { fileId: item.id })

export const destroy = (item: TrashItem) => post('/api/trash/destroy', { fileId: item.id })

export const resolveCollision = (collision: Collision, keepBoth: boolean) =>
  post('/api/collision', { collisionId: collision.id, keepBoth })
