/**
 * The events a destination is told of, ticked rather than typed.
 *
 * The server names every one it can tell of — the audit trail's actions and
 * the facts nobody did — and a destination keeps the names ticked. Every
 * event, `*`, stands above the rest: those the server grows later come with
 * it. Then the facts, the few an operator came for, and the trail's actions
 * folded away, forty-odd of them. A name kept that this server does not know
 * is kept as it is, at the end, and said so.
 *
 * It was the settings page's own list until destinations had a page of
 * their own; it is controlled now, so a destination's form saves it with
 * the rest rather than at each tick.
 */

import { useI18n } from '../lib/i18n'
import type { WebhookEvents } from '../lib/types'
import { Glyph } from './ui'

/** `*`: every event, those the server grows later included. */
export const EVERY = '*'

export function EventPicker({
  value,
  known,
  onChange,
  disabled,
}: {
  /** The names ticked, as the destination keeps them. */
  value: string[]
  known: WebhookEvents
  onChange: (next: string[]) => void
  disabled?: boolean
}) {
  const { t } = useI18n()
  const e = t.admin.notifications.events
  const says: Record<string, string | undefined> = t.admin.trail.says

  const offered = [...known.facts, ...known.actions]
  const unknown = value.filter((name) => name !== EVERY && !offered.includes(name))
  const ticked = new Set(value)
  const every = ticked.has(EVERY)
  const actionsTicked = known.actions.filter((name) => every || ticked.has(name)).length

  // In the order they are offered; a name this server does not know is kept
  // where it was, at the end.
  const write = (next: Set<string>) =>
    onChange([...offered.filter((name) => next.has(name)), ...unknown])
  const toggle = (name: string, on: boolean) => {
    const next = new Set(ticked)
    if (on) next.add(name)
    else next.delete(name)
    write(next)
  }
  // Every event off is every one named: what was told goes on being told,
  // and each can then be unticked.
  const toggleEvery = (on: boolean) => (on ? onChange([EVERY]) : write(new Set(offered)))

  const box = (name: string, label: string, hint?: string) => (
    <li key={name}>
      <label className="flex min-h-11 cursor-pointer items-start gap-3 py-1.5 has-[:disabled]:cursor-not-allowed">
        <input
          type="checkbox"
          className="mt-1 size-4 shrink-0 cursor-pointer accent-vermillion disabled:cursor-not-allowed disabled:opacity-60"
          checked={every || ticked.has(name)}
          disabled={every || disabled}
          onChange={(event) => toggle(name, event.target.checked)}
        />
        <span className="min-w-0">
          <span className="block text-sm text-bone">{label}</span>
          <span className="block font-mono text-[0.6875rem] text-bone-faint">{name}</span>
          {hint ? <span className="mt-0.5 block text-xs leading-relaxed text-bone-faint">{hint}</span> : null}
        </span>
      </label>
    </li>
  )

  return (
    <fieldset className="min-w-0">
      <legend className="label">{e.title}</legend>

      <label className="mt-2 flex min-h-11 cursor-pointer items-center gap-3 has-[:disabled]:cursor-not-allowed">
        <input
          type="checkbox"
          className="size-4 shrink-0 cursor-pointer accent-vermillion disabled:cursor-not-allowed"
          checked={every}
          disabled={disabled}
          onChange={(event) => toggleEvery(event.target.checked)}
        />
        <span className="text-sm text-bone">{e.every}</span>
        <span className="font-mono text-[0.6875rem] text-bone-faint">{EVERY}</span>
      </label>

      <fieldset className="mt-3 min-w-0">
        <legend className="label">{e.facts}</legend>
        <ul className="mt-1">
          {known.facts.map((name) => box(name, e.fact[name] ?? name, e.factHint[name]))}
        </ul>
      </fieldset>

      <details className="group mt-3">
        <summary className="label flex min-h-11 cursor-pointer list-none items-center gap-2 transition-colors duration-150 hover:text-bone [&::-webkit-details-marker]:hidden">
          <Glyph name="chevronRight" className="size-3.5 transition-transform duration-150 group-open:rotate-90" />
          {e.actions}
          <span className="font-mono tabular-nums normal-case">
            {actionsTicked}/{known.actions.length}
          </span>
        </summary>
        <fieldset className="min-w-0">
          <legend className="sr-only">{e.actions}</legend>
          <ul className="mt-1 grid gap-x-6 sm:grid-cols-2">
            {known.actions.map((name) => box(name, says[name] ?? name))}
          </ul>
        </fieldset>
      </details>

      {unknown.length ? (
        <p className="mt-3 max-w-prose text-xs leading-relaxed text-bone-faint">
          {e.unknown(unknown.join(', '))}
        </p>
      ) : null}
    </fieldset>
  )
}
