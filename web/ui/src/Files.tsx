import { For, Show, createMemo, createSignal } from 'solid-js'
import type { Item } from './types'
import {
  deleteFrom,
  deleteHere,
  importFiles,
  pull,
  resolveCollision,
  share,
  state,
  transfers,
} from './mesh'
import { Button, Empty, HolderChip, Panel, fileSize } from './ui'
import { FolderPicker } from './FolderPicker'

/**
 * The shared catalog: every item in the mesh, and which devices hold it.
 *
 * An item is in the catalog whether or not this device has its contents, which
 * is the whole idea — so every row says which of the two it is before it says
 * anything else.
 */
export function Files() {
  const [dragging, setDragging] = createSignal(false)
  const [choosingFolder, setChoosingFolder] = createSignal(false)
  let picker!: HTMLInputElement

  const onDrop = (event: DragEvent) => {
    event.preventDefault()
    setDragging(false)
    if (event.dataTransfer?.files.length) void importFiles(event.dataTransfer.files)
  }

  return (
    <div
      class="flex flex-col gap-4"
      onDragOver={(event) => {
        event.preventDefault()
        setDragging(true)
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
    >
      <Show when={state.collisions.length > 0}>
        <div class="rounded-xl border border-away/40 bg-away-soft">
          <For each={state.collisions}>
            {(collision) => (
              <div class="flex flex-wrap items-center gap-3 px-4 py-3">
                <div class="min-w-0 flex-1">
                  <p class="text-sm">
                    <b>{collision.requested}</b> was already taken.
                  </p>
                  <p class="text-xs text-muted">It was kept as {collision.keptAs}.</p>
                </div>
                <Button onClick={() => void resolveCollision(collision, true)}>Keep both</Button>
                <Button tone="danger" onClick={() => void resolveCollision(collision, false)}>
                  Replace the old one
                </Button>
              </div>
            )}
          </For>
        </div>
      </Show>

      <Panel
        title="Catalog"
        hint={state.items.length === 1 ? '1 item' : `${state.items.length} items`}
      >
        <div class="flex items-center gap-2 px-4 py-2.5">
          <Button tone="accent" onClick={() => picker.click()}>
            Add files
          </Button>
          <input
            ref={picker}
            type="file"
            multiple
            class="hidden"
            onChange={(event) => {
              if (event.currentTarget.files) void importFiles(event.currentTarget.files)
              event.currentTarget.value = ''
            }}
          />
          <span class="min-w-0 text-xs text-muted">
            or drop them anywhere — anything put in{' '}
            <button
              class="font-mono underline decoration-dotted underline-offset-2 hover:text-accent"
              title="Choose another folder"
              onClick={() => setChoosingFolder(true)}
            >
              {state.device.syncDir}
            </button>{' '}
            is picked up on its own
          </span>
        </div>

        <Show when={state.items.length > 0} fallback={<Empty>Nothing in the catalog yet.</Empty>}>
          <For each={state.items}>{(item) => <Row item={item} />}</For>
        </Show>

        <Show when={state.deferredDeletes > 0}>
          <p class="px-4 py-2 text-xs text-muted">
            {state.deferredDeletes} delete{state.deferredDeletes === 1 ? '' : 's'} waiting for
            devices that are not on the network. They travel with the catalog.
          </p>
        </Show>
      </Panel>

      <Show when={dragging()}>
        <div class="pointer-events-none fixed inset-3 rounded-2xl border-2 border-dashed border-accent" />
      </Show>

      <Show when={choosingFolder()}>
        <FolderPicker close={() => setChoosingFolder(false)} />
      </Show>
    </div>
  )
}

function Row(props: { item: Item }) {
  /** Paired, on the network, and not already holding this. */
  const sendable = createMemo(() => {
    const holders = new Set(props.item.holders.map((holder) => holder.deviceId))
    return state.paired.filter((peer) => peer.reachable && !holders.has(peer.deviceId))
  })

  const bars = createMemo(() =>
    Object.values(transfers).filter((progress) => progress.fileId === props.item.id),
  )

  const elsewhere = createMemo(() => props.item.holders.filter((holder) => !holder.isThisDevice))

  return (
    <div class="px-4 py-3">
      <div class="flex items-baseline gap-3">
        <span class="min-w-0 flex-1 truncate text-sm font-medium" title={props.item.name}>
          {props.item.name}
        </span>
        <span class="font-mono text-xs text-muted tabular-nums">{fileSize(props.item.size)}</span>
      </div>

      <div class="mt-2 flex flex-wrap items-center gap-1.5">
        <Show
          when={props.item.holders.length > 0}
          fallback={<span class="text-xs text-danger">No device holds this</span>}
        >
          <For each={props.item.holders}>{(holder) => <HolderChip holder={holder} />}</For>
        </Show>

        <span class="flex-1" />

        <Show when={props.item.heldHere}>
          <a
            href={`/api/file/${props.item.id}`}
            class="inline-flex items-center rounded-md border border-line px-2.5 py-1 text-[13px] font-medium hover:bg-ink/5"
          >
            Download
          </a>
        </Show>

        <Show
          when={props.item.heldHere}
          fallback={<Button onClick={() => void pull(props.item)}>Take a copy</Button>}
        >
          <Show
            when={sendable().length > 0}
            fallback={
              <Button disabled title="No paired device is on the network">
                Send
              </Button>
            }
          >
            <SendMenu item={props.item} targets={sendable()} />
          </Show>
        </Show>

        <DeleteMenu item={props.item} elsewhere={elsewhere()} />
      </div>

      <For each={bars()}>
        {(progress) => (
          <div class="mt-2">
            <div class="flex justify-between text-[11px] text-muted">
              <span>
                {progress.sending
                  ? `Sending to ${nameOf(progress.deviceId)}`
                  : 'Receiving'}
              </span>
              <span class="tabular-nums">
                {progress.done} / {progress.total} blocks
              </span>
            </div>
            <div class="mt-1 h-1 overflow-hidden rounded-full bg-line">
              <div
                class="h-full rounded-full bg-accent transition-[width] duration-150"
                style={{
                  width: `${progress.total > 0 ? (progress.done / progress.total) * 100 : 0}%`,
                }}
              />
            </div>
          </div>
        )}
      </For>
    </div>
  )
}

function nameOf(deviceId: string | null): string {
  if (!deviceId) return 'another device'
  return state.paired.find((peer) => peer.deviceId === deviceId)?.name ?? deviceId.slice(0, 8)
}

/**
 * A <details> rather than a popover.
 *
 * It closes on click-outside for free, needs no positioning code, and costs
 * nothing in DOM until it is opened.
 */
function SendMenu(props: { item: Item; targets: { deviceId: string; name: string }[] }) {
  let box!: HTMLDetailsElement
  const send = (ids: string[]) => {
    box.open = false
    void share(props.item, ids)
  }
  return (
    <details ref={box} class="relative">
      <summary class="inline-flex cursor-pointer list-none items-center rounded-md border border-line px-2.5 py-1 text-[13px] font-medium hover:bg-ink/5">
        Send
      </summary>
      <div class="absolute right-0 z-10 mt-1 min-w-44 overflow-hidden rounded-lg border border-line bg-panel shadow-lg">
        <For each={props.targets}>
          {(target) => (
            <button
              class="block w-full px-3 py-1.5 text-left text-[13px] hover:bg-ink/5"
              onClick={() => send([target.deviceId])}
            >
              {target.name}
            </button>
          )}
        </For>
        <Show when={props.targets.length > 1}>
          <button
            class="block w-full border-t border-line px-3 py-1.5 text-left text-[13px] hover:bg-ink/5"
            onClick={() => send(props.targets.map((target) => target.deviceId))}
          >
            Every device here
          </button>
        </Show>
      </div>
    </details>
  )
}

function DeleteMenu(props: { item: Item; elsewhere: { deviceId: string; name: string }[] }) {
  let box!: HTMLDetailsElement
  const run = (work: () => void) => {
    box.open = false
    work()
  }
  return (
    <details ref={box} class="relative">
      <summary class="inline-flex cursor-pointer list-none items-center rounded-md border border-line px-2.5 py-1 text-[13px] font-medium text-danger hover:bg-danger/10">
        Delete
      </summary>
      <div class="absolute right-0 z-10 mt-1 min-w-52 overflow-hidden rounded-lg border border-line bg-panel shadow-lg">
        <Show when={props.item.heldHere}>
          <button
            class="block w-full px-3 py-1.5 text-left text-[13px] text-danger hover:bg-danger/10"
            onClick={() => run(() => void deleteHere(props.item))}
          >
            Delete from here
          </button>
        </Show>
        <For each={props.elsewhere}>
          {(holder) => (
            <button
              class="block w-full border-t border-line px-3 py-1.5 text-left text-[13px] text-danger hover:bg-danger/10"
              onClick={() => run(() => void deleteFrom(props.item, holder.deviceId))}
            >
              Delete from {holder.name}
            </button>
          )}
        </For>
      </div>
    </details>
  )
}
