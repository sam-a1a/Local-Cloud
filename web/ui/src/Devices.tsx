import { For, Show, createSignal } from 'solid-js'
import { cancelPairing, confirmPairing, startPairing, state, unpair } from './mesh'
import { Button, Dot, Empty, Panel } from './ui'

/**
 * The mesh, and how devices join it.
 *
 * Three lists, in the order the question comes up: who has asked to pair with
 * this device, who is on the network but is nobody yet, and who this device
 * already trusts.
 */
export function Devices() {
  const [showing, setShowing] = createSignal<{ code: string; name: string } | null>(null)

  const begin = async (deviceId: string, name: string) => {
    const started = await startPairing(deviceId)
    if (started) setShowing({ code: started.code, name })
  }

  const stop = () => {
    setShowing(null)
    void cancelPairing()
  }

  return (
    <div class="flex flex-col gap-4">
      <Show when={state.offers.length > 0}>
        <Panel title="Asked to pair with this device">
          <For each={state.offers}>{(offer) => <Offer offer={offer} />}</For>
        </Panel>
      </Show>

      <Panel title="On the network" hint="not paired yet">
        <Show
          when={state.visible.length > 0}
          fallback={<Empty>Nothing else found. Both devices have to be on the same Wi-Fi.</Empty>}
        >
          <For each={state.visible}>
            {(peer) => (
              <div class="flex items-center gap-3 px-4 py-2.5">
                <Dot live={true} />
                <div class="min-w-0 flex-1">
                  <p class="truncate text-sm">{peer.name}</p>
                  <p class="font-mono text-xs text-muted">
                    {peer.platform} · {peer.deviceId.slice(0, 8)}
                  </p>
                </div>
                <Button tone="accent" onClick={() => void begin(peer.deviceId, peer.name)}>
                  Pair
                </Button>
              </div>
            )}
          </For>
        </Show>
      </Panel>

      <Panel title="Paired">
        <Show when={state.paired.length > 0} fallback={<Empty>No devices yet.</Empty>}>
          <For each={state.paired}>
            {(peer) => (
              <div class="flex items-center gap-3 px-4 py-2.5">
                <Dot live={peer.reachable} />
                <div class="min-w-0 flex-1">
                  <p class="truncate text-sm">{peer.name}</p>
                  <p class="font-mono text-xs text-muted">
                    {peer.platform} ·{' '}
                    {peer.reachable ? peer.deviceId.slice(0, 8) : 'not on the network'}
                  </p>
                </div>
                <Button tone="danger" onClick={() => void unpair(peer.deviceId)}>
                  Unpair
                </Button>
              </div>
            )}
          </For>
        </Show>
      </Panel>

      <Show when={showing()}>
        {(pairing) => (
          <div
            class="fixed inset-0 z-20 flex items-center justify-center bg-black/40 p-6"
            onClick={stop}
          >
            <div
              class="rounded-2xl border border-line bg-panel px-10 py-8 text-center shadow-2xl"
              onClick={(event) => event.stopPropagation()}
            >
              <p class="text-sm text-muted">Type this on</p>
              <p class="mb-4 text-lg font-semibold">{pairing().name}</p>
              <p class="font-mono text-5xl font-semibold tracking-[0.1em] tabular-nums select-all">
                {spaced(pairing().code)}
              </p>
              <p class="mt-4 text-xs text-muted">The code expires in a couple of minutes.</p>
              <Button class="mt-5" onClick={stop}>
                Cancel
              </Button>
            </div>
          </div>
        )}
      </Show>
    </div>
  )
}

/** 123 456 rather than 123456: read off one screen and typed into another. */
function spaced(code: string): string {
  return code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code
}

function Offer(props: { offer: { deviceId: string; name: string; platform: string } }) {
  const [code, setCode] = createSignal('')
  const [busy, setBusy] = createSignal(false)

  const confirm = async () => {
    if (code().length !== 6 || busy()) return
    setBusy(true)
    const ok = await confirmPairing(props.offer.deviceId, code())
    if (ok) setCode('')
    setBusy(false)
  }

  return (
    <div class="flex flex-wrap items-center gap-3 px-4 py-2.5">
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm">{props.offer.name}</p>
        <p class="font-mono text-xs text-muted">
          {props.offer.platform} · {props.offer.deviceId.slice(0, 8)}
        </p>
      </div>
      <input
        value={code()}
        onInput={(event) => setCode(event.currentTarget.value.replace(/\D/g, '').slice(0, 6))}
        onKeyDown={(event) => event.key === 'Enter' && void confirm()}
        inputmode="numeric"
        placeholder="000000"
        class="w-24 rounded-md border border-line bg-surface px-2 py-1 text-center font-mono text-sm tabular-nums outline-none focus:border-accent"
      />
      <Button tone="accent" disabled={code().length !== 6 || busy()} onClick={() => void confirm()}>
        Confirm
      </Button>
    </div>
  )
}
