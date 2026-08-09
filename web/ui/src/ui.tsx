import type { JSX } from 'solid-js'
import { splitProps } from 'solid-js'
import type { Holder } from './types'

/** "4.2 MB", the way the rest of the machine writes it. */
export function fileSize(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`
  const units = ['kB', 'MB', 'GB', 'TB']
  let value = bytes / 1000
  let unit = 0
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000
    unit += 1
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`
}

/**
 * How long a trashed item has left, at the resolution a person cares about.
 * Nobody restoring a file wants to be told it has 2,591,999 seconds.
 */
export function timeLeft(seconds: number | null): string {
  if (seconds === null) return 'no countdown'
  if (seconds <= 0) return 'gone shortly'
  if (seconds >= 86_400) {
    const days = Math.floor(seconds / 86_400)
    return days === 1 ? '1 day left' : `${days} days left`
  }
  if (seconds >= 3_600) {
    const hours = Math.floor(seconds / 3_600)
    return hours === 1 ? '1 hour left' : `${hours} hours left`
  }
  const minutes = Math.max(1, Math.floor(seconds / 60))
  return minutes === 1 ? '1 minute left' : `${minutes} minutes left`
}

type ButtonProps = JSX.ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: 'plain' | 'accent' | 'danger'
}

export function Button(props: ButtonProps) {
  const [own, rest] = splitProps(props, ['tone', 'class', 'children'])
  const tone = () => own.tone ?? 'plain'
  return (
    <button
      {...rest}
      class={`inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-[13px] font-medium
        transition-colors disabled:cursor-not-allowed disabled:opacity-40
        ${
          tone() === 'accent'
            ? 'border-transparent bg-accent text-white hover:opacity-90'
            : tone() === 'danger'
              ? 'border-line text-danger hover:bg-danger/10'
              : 'border-line text-ink hover:bg-ink/5'
        } ${own.class ?? ''}`}
    >
      {own.children}
    </button>
  )
}

/**
 * A device that holds a copy.
 *
 * Cool for here, warm for elsewhere, faded when the device is not on the
 * network — so "where is this?" is answered by colour before it is answered by
 * reading. The same three states the phone and the Mac app use.
 */
export function HolderChip(props: { holder: Holder }) {
  return (
    <span
      title={props.holder.reachable ? props.holder.name : `${props.holder.name} — not on the network`}
      class={`rounded-full px-2 py-0.5 text-[11px] font-medium whitespace-nowrap ${
        props.holder.isThisDevice
          ? 'bg-here-soft text-here'
          : 'bg-away-soft text-away'
      } ${props.holder.reachable ? '' : 'opacity-45'}`}
    >
      {props.holder.isThisDevice ? 'Here' : props.holder.name}
    </span>
  )
}

/** Present, or not, at a glance. */
export function Dot(props: { live: boolean }) {
  return (
    <span
      class={`inline-block size-2 shrink-0 rounded-full ${
        props.live ? 'bg-live' : 'bg-muted/40'
      }`}
    />
  )
}

export function Panel(props: { title: string; hint?: string; children: JSX.Element }) {
  return (
    <section class="rounded-xl border border-line bg-panel">
      <header class="flex items-baseline gap-2 border-b border-line px-4 py-2.5">
        <h2 class="text-[13px] font-semibold tracking-wide uppercase">{props.title}</h2>
        {props.hint && <span class="text-xs text-muted">{props.hint}</span>}
      </header>
      <div class="divide-y divide-line">{props.children}</div>
    </section>
  )
}

export function Empty(props: { children: JSX.Element }) {
  return <p class="px-4 py-6 text-center text-sm text-muted">{props.children}</p>
}
