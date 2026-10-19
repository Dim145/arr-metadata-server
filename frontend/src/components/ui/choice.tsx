/**
 * Choosing one of a few, and saying what a thing is.
 *
 * A segmented choice is radio buttons in a row — the browser's own, hidden
 * and labelled, so the arrow keys move between them and a screen reader
 * says "2 of 4". A lamp is a status in a word with a dot beside it: the word
 * carries the meaning, the colour only helps.
 */

import { useId, type ReactNode } from 'react'

import { cn } from '../../lib/cn'

/** Which way an arrow key walks along a row of options, and none for any other key. */
function stepOf(key: string): number {
  return key === 'ArrowRight' || key === 'ArrowDown' ? 1 : key === 'ArrowLeft' || key === 'ArrowUp' ? -1 : 0
}

export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  className,
  deliberate,
}: {
  /** What is being chosen, for a screen reader. */
  label: string
  value: T
  options: { value: T; label: ReactNode }[]
  onChange: (next: T) => void
  disabled?: boolean
  className?: string
  /**
   * For a choice that is acted on the moment it is made. A radio group's own
   * arrow keys choose as they move, so one stroke on a keyboard — made to read
   * the options — opened a private site or made a member an administrator.
   * Here they only move the focus along; Space, Enter or a press chooses.
   */
  deliberate?: boolean
}) {
  const name = useId()

  return (
    // eslint-disable-next-line jsx-a11y/interactive-supports-focus -- the radios take the focus, one at a time; the group hears their keys
    <div
      role="radiogroup"
      aria-label={label}
      onKeyDown={
        deliberate
          ? (event) => {
              const radio = event.target
              if (!(radio instanceof HTMLInputElement) || radio.type !== 'radio') return
              if (event.key === 'Enter') {
                event.preventDefault()
                radio.click()
                return
              }
              const step = stepOf(event.key)
              if (!step) return
              // Without this the browser would check the next one as it focuses it.
              event.preventDefault()
              const radios = [
                ...event.currentTarget.querySelectorAll<HTMLInputElement>('input[type="radio"]:not(:disabled)'),
              ]
              radios[(radios.indexOf(radio) + step + radios.length) % radios.length]?.focus()
            }
          : undefined
      }
      className={cn(
        'inline-flex flex-wrap gap-1 rounded-full border border-rule bg-ink p-1',
        'forced-colors:border-[color:ButtonText]',
        disabled && 'opacity-50',
        className,
      )}
    >
      {options.map((option) => (
        <label
          key={option.value}
          className={cn(
            'relative grid min-h-9 cursor-pointer place-items-center rounded-full px-3.5 text-body-sm whitespace-nowrap',
            'transition-colors duration-150 has-focus-visible:outline-2 has-focus-visible:outline-vermillion',
            // The chosen one is a lighter surface and an inset rule, both of
            // which forced colours remove: there it wears the system's
            // selection colours instead, as a chosen item does everywhere else.
            value === option.value
              ? 'bg-ink-top text-bone shadow-[inset_0_0_0_1px_var(--color-rule-bright)] forced-colors:bg-[color:Highlight] forced-colors:text-[color:HighlightText]'
              : 'text-bone-dim hover:text-bone',
            disabled && 'cursor-not-allowed',
          )}
        >
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={value === option.value}
            disabled={disabled}
            onChange={() => onChange(option.value)}
            className="sr-only"
          />
          {option.label}
        </label>
      ))}
    </div>
  )
}

type LampTone = 'moss' | 'brass' | 'faint' | 'vermillion'

// A lamp is a state, so it reads the states' colours (styles.css): well,
// wanting a look, failing — and the faint one, for a state not known.
const LAMP: Record<LampTone, string> = {
  moss: 'bg-status-ok shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-status-ok)_18%,transparent)]',
  brass: 'bg-status-warn shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-status-warn)_18%,transparent)]',
  faint: 'bg-bone-faint shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-bone-faint)_18%,transparent)]',
  vermillion:
    'bg-status-error shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-status-error)_18%,transparent)]',
}

export function Lamp({ tone, children }: { tone: LampTone; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-2 text-body-sm text-bone">
      {/* Forced colours paint the dot in the page's own colour, which would
          leave a hole: a ring of the system's button colour keeps it there.
          Its tone goes, which is why the word beside it says the state. */}
      <span
        aria-hidden
        className={cn('size-2 shrink-0 rounded-full', LAMP[tone], 'forced-colors:border forced-colors:border-[color:ButtonText]')}
      />
      {children}
    </span>
  )
}
