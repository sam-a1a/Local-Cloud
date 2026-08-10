import { For, Show, createResource, createSignal } from 'solid-js'
import { browseFolders, chooseFolder, createFolder, state } from './mesh'
import { Button } from './ui'

/**
 * Choosing where this device keeps what it holds.
 *
 * The browser has no picker that would answer this: `webkitdirectory` gives up
 * file names and no path, and the folder being chosen belongs to the machine
 * the engine is on rather than to the page. So the page browses that machine a
 * directory at a time and sends back the path it settled on.
 *
 * Both consequences are on screen before the button is, because neither is
 * reversible by clicking again: files already in the chosen folder join the
 * mesh, and files in the current one come across to it.
 */
export function FolderPicker(props: { close: () => void }) {
  const [at, setAt] = createSignal<string | undefined>(undefined)
  const [listing] = createResource(at, browseFolders)
  const [naming, setNaming] = createSignal(false)
  const [busy, setBusy] = createSignal(false)

  const here = () => listing()?.path ?? state.device.syncDir
  const isCurrent = () => here() === state.device.syncDir

  const use = async () => {
    setBusy(true)
    const outcome = await chooseFolder(here())
    setBusy(false)
    if (outcome) props.close()
  }

  const make = async (name: string) => {
    setNaming(false)
    if (!name.trim()) return
    const made = await createFolder(here(), name)
    if (made) setAt(made.path)
  }

  return (
    <div
      class="fixed inset-0 z-30 flex items-center justify-center bg-black/40 p-6"
      onClick={props.close}
    >
      <div
        class="flex max-h-[80vh] w-full max-w-xl flex-col overflow-hidden rounded-2xl border border-line bg-panel shadow-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        <header class="border-b border-line px-5 py-4">
          <h2 class="text-sm font-semibold">Where to keep files</h2>
          <p class="mt-1 truncate font-mono text-xs text-muted" title={here()}>
            {here()}
          </p>
        </header>

        <div class="flex items-center gap-2 border-b border-line px-5 py-2">
          <Button
            disabled={!listing()?.parent}
            onClick={() => setAt(listing()?.parent ?? undefined)}
          >
            ↑ Up
          </Button>
          <Show
            when={!naming()}
            fallback={
              <input
                autofocus
                placeholder="Folder name"
                class="flex-1 rounded-md border border-line bg-surface px-2 py-1 text-sm outline-none focus:border-accent"
                onKeyDown={(event) => {
                  if (event.key === 'Enter') void make(event.currentTarget.value)
                  if (event.key === 'Escape') setNaming(false)
                }}
                onBlur={() => setNaming(false)}
              />
            }
          >
            <Button onClick={() => setNaming(true)}>New folder</Button>
            <span class="flex-1" />
            <Show when={listing()}>
              {(current) => (
                <span class="text-xs text-muted">
                  {current().folders.length} folders · {current().files} files
                </span>
              )}
            </Show>
          </Show>
        </div>

        <div class="min-h-32 flex-1 overflow-y-auto">
          <Show when={listing()} fallback={<p class="p-5 text-sm text-muted">Reading…</p>}>
            {(current) => (
              <Show
                when={current().folders.length > 0}
                fallback={<p class="p-5 text-sm text-muted">No folders inside this one.</p>}
              >
                <For each={current().folders}>
                  {(folder) => (
                    <button
                      class="flex w-full items-center gap-2 border-b border-line px-5 py-2 text-left text-sm last:border-0 hover:bg-ink/5"
                      onDblClick={() => setAt(folder.path)}
                      onClick={() => setAt(folder.path)}
                    >
                      <span class="text-muted">/</span>
                      <span class="truncate">{folder.name}</span>
                    </button>
                  )}
                </For>
              </Show>
            )}
          </Show>
        </div>

        <footer class="border-t border-line px-5 py-4">
          <p class="text-xs text-muted">
            <Show when={(listing()?.files ?? 0) > 0}>
              The {listing()?.files} files already here will join the mesh.{' '}
            </Show>
            <Show when={!isCurrent()}>
              Anything in the current folder moves across; nothing is overwritten.
            </Show>
          </p>
          <div class="mt-3 flex justify-end gap-2">
            <Button onClick={props.close}>Cancel</Button>
            <Button
              tone="accent"
              disabled={busy() || isCurrent() || !listing()}
              onClick={() => void use()}
            >
              {busy() ? 'Moving…' : isCurrent() ? 'Already in use' : 'Use this folder'}
            </Button>
            <Show when={listing.error}>
              <span class="text-xs text-danger">could not read that folder</span>
            </Show>
          </div>
        </footer>
      </div>
    </div>
  )
}
