import { For, Show } from 'solid-js'
import { destroy, restore, state } from './mesh'
import { Button, Empty, Panel, fileSize, timeLeft } from './ui'

/**
 * The thirty days between the last copy of an item being deleted and its bytes
 * being released.
 *
 * An item reaches this only when nobody holds it any more — deleting one copy
 * of three just removes a holder — which is why the countdown is the most
 * important thing on the row.
 */
export function Trash() {
  return (
    <Panel title="Trash" hint="30 days, then the bytes go">
      <Show
        when={state.trash.length > 0}
        fallback={<Empty>Nothing here. An item arrives when its last copy is deleted.</Empty>}
      >
        <For each={state.trash}>
          {(item) => (
            <div class="flex flex-wrap items-center gap-3 px-4 py-2.5">
              <div class="min-w-0 flex-1">
                <p class="truncate text-sm">{item.name}</p>
                <p class="text-xs text-muted">
                  {fileSize(item.size)} · {timeLeft(item.secondsRemaining)} · deleted by{' '}
                  {item.trashedBy}
                </p>
              </div>
              <Button onClick={() => void restore(item)}>Restore</Button>
              <Button tone="danger" onClick={() => void destroy(item)}>
                Delete now
              </Button>
            </div>
          )}
        </For>
      </Show>
    </Panel>
  )
}
