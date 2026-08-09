import { Show, createSignal, onMount } from 'solid-js'
import { Dynamic } from 'solid-js/web'
import { connect, connected, dismiss, notice, rename, state } from './mesh'
import { Devices } from './Devices'
import { Files } from './Files'
import { Trash } from './Trash'
import { Dot } from './ui'

const screens = {
  Files,
  Devices,
  Trash,
} as const

type Screen = keyof typeof screens

export function App() {
  const [screen, setScreen] = createSignal<Screen>('Files')
  onMount(connect)

  const count = (name: Screen) =>
    name === 'Files' ? state.items.length : name === 'Trash' ? state.trash.length : state.offers.length

  return (
    <div class="mx-auto flex min-h-full max-w-4xl flex-col px-5 pb-24">
      <header class="flex flex-wrap items-center gap-3 py-5">
        <h1 class="text-base font-semibold">LocalCloud</h1>
        <span class="flex-1" />
        <ThisDevice />
      </header>

      <nav class="mb-4 flex gap-1 rounded-lg border border-line bg-panel p-1">
        {(Object.keys(screens) as Screen[]).map((name) => (
          <button
            onClick={() => setScreen(name)}
            class={`flex-1 rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
              screen() === name ? 'bg-accent text-white' : 'text-muted hover:text-ink'
            }`}
          >
            {name}
            <Show when={count(name) > 0}>
              <span class="ml-1.5 text-xs opacity-70 tabular-nums">{count(name)}</span>
            </Show>
          </button>
        ))}
      </nav>

      <main class="flex-1">
        <Dynamic component={screens[screen()]} />
      </main>

      <Show when={notice()}>
        {(current) => (
          <div class="fixed inset-x-0 bottom-0 z-30 flex justify-center p-4">
            <div
              class={`flex max-w-2xl items-center gap-3 rounded-xl border px-4 py-2.5 text-sm shadow-lg backdrop-blur ${
                current().failure
                  ? 'border-danger/40 bg-panel/95 text-danger'
                  : 'border-line bg-panel/95'
              }`}
            >
              <span>{current().text}</span>
              <button class="text-muted hover:text-ink" onClick={dismiss} aria-label="Dismiss">
                ✕
              </button>
            </div>
          </div>
        )}
      </Show>
    </div>
  )
}

/**
 * Who this device is on the mesh, and whether it is on it at all.
 *
 * In the header, on every screen, because every question on every screen is
 * asked relative to this device.
 */
function ThisDevice() {
  const [editing, setEditing] = createSignal(false)
  let field!: HTMLInputElement

  const commit = () => {
    setEditing(false)
    const name = field.value.trim()
    if (name && name !== state.device.name) void rename(name)
  }

  return (
    <div class="flex items-center gap-2.5 rounded-lg border border-line bg-panel px-3 py-1.5">
      <Dot live={state.device.running && connected()} />
      <Show
        when={!editing()}
        fallback={
          <input
            ref={field}
            value={state.device.name}
            autofocus
            onBlur={commit}
            onKeyDown={(event) => {
              if (event.key === 'Enter') commit()
              if (event.key === 'Escape') setEditing(false)
            }}
            class="w-40 rounded border border-line bg-surface px-1.5 py-0.5 text-sm outline-none focus:border-accent"
          />
        }
      >
        <button
          class="text-sm font-medium hover:text-accent"
          title="Rename this device"
          onClick={() => setEditing(true)}
        >
          {state.device.name || 'This device'}
        </button>
      </Show>
      <span class="font-mono text-xs text-muted">
        {state.device.platform} · {state.device.shortId}
      </span>
      <Show when={!connected()}>
        <span class="text-xs text-danger">reconnecting…</span>
      </Show>
    </div>
  )
}
