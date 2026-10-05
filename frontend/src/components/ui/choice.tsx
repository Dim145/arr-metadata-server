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
        disabled && 'opacity-50',
        className,
      )}
    >
      {options.map((option) => (
        <label
          key={option.value}
          className={cn(
            'relative grid min-h-9 cursor-pointer place-items-center rounded-full px-3.5 text-[0.8125rem] whitespace-nowrap',
            'transition-colors duration-150 has-focus-visible:outline-2 has-focus-visible:outline-vermillion',
            value === option.value
              ? 'bg-ink-top text-bone shadow-[inset_0_0_0_1px_var(--color-rule-bright)]'
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

const LAMP: Record<LampTone, string> = {
  moss: 'bg-moss shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-moss)_18%,transparent)]',
  brass: 'bg-brass shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-brass)_18%,transparent)]',
  faint: 'bg-bone-faint shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-bone-faint)_18%,transparent)]',
  vermillion:
    'bg-vermillion shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-vermillion)_18%,transparent)]',
}

export function Lamp({ tone, children }: { tone: LampTone; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-2 text-[0.8125rem] text-bone">
      <span aria-hidden className={cn('size-2 shrink-0 rounded-full', LAMP[tone])} />
      {children}
    </span>
  )
}
