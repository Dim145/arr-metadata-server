/**
 * What a form is made of, and the fields around it.
 */

import { type ReactNode } from 'react'
import { cn } from '../../lib/cn'

import { Glyph } from './glyphs'

export function Input({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...props}
      className={cn(
        'min-h-11 w-full rounded-card border border-rule bg-ink px-3 text-sm text-bone',
        'transition-colors duration-200 placeholder:text-bone-faint',
        'hover:border-rule-bright focus:border-bone-dim focus:outline-none',
        className,
      )}
    />
  )
}

/**
 * Exists because the administration side edits prose — an overview, a note —
 * and a one-line box turns a paragraph into a keyhole. Same frame as `Input`,
 * so a form mixing the two reads as one form.
 */
export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      {...props}
      className={cn(
        'w-full rounded-card border border-rule bg-ink px-3 py-2.5 text-sm leading-relaxed text-bone',
        'transition-colors duration-200 placeholder:text-bone-faint',
        'hover:border-rule-bright focus:border-bone-dim focus:outline-none',
        className,
      )}
    />
  )
}

export function Select({ className, ...props }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      {...props}
      className={cn(
        'min-h-11 w-full cursor-pointer appearance-none rounded-card border border-rule bg-ink',
        'px-3 pr-9 text-sm text-bone transition-colors duration-200',
        'hover:border-rule-bright focus:border-bone-dim focus:outline-none',
        // The chevron, drawn rather than imported, so it inherits the palette.
        "bg-[url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='8' viewBox='0 0 12 8'%3E%3Cpath d='M1 1.5 6 6.5 11 1.5' stroke='%23a5a099' stroke-width='1.5' fill='none' stroke-linecap='round'/%3E%3C/svg%3E\")]",
        'bg-[length:12px_8px] bg-[position:right_0.75rem_center] bg-no-repeat',
        className,
      )}
    />
  )
}

/**
 * A flag, as a switch.
 *
 * Exists because a boolean rendered as a two-option `Select` makes the reader
 * parse "enabled/disabled" to find out what it is doing now, where a switch
 * says it at a glance. The name is compulsory: the caller writes it beside the
 * track, and it reaches a screen reader through `aria-label`, because a track
 * on its own means nothing to either kind of reader.
 */
export function Toggle({
  id,
  checked,
  label,
  onChange,
  disabled,
}: {
  id?: string
  checked: boolean
  label: string
  onChange: (next: boolean) => void
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      id={id}
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        // 44px of target around a 24px track: the track is what it looks like,
        // not what a thumb has to find.
        'inline-flex h-11 w-14 shrink-0 cursor-pointer items-center justify-center rounded-card',
        'transition-colors duration-150 hover:bg-ink-high',
        'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent',
      )}
    >
      <span
        aria-hidden
        className={cn(
          'relative block h-6 w-11 rounded-full border transition-colors duration-150',
          checked ? 'border-moss bg-moss/25' : 'border-rule-bright bg-ink',
        )}
      >
        {/* The knob travels by transform rather than by `left`: the second
            would lay the row out again on every frame of a 150ms slide. */}
        <span
          className={cn(
            'absolute top-1/2 left-0.5 size-4 -translate-y-1/2 rounded-full',
            'transition-[transform,background-color] duration-150',
            checked ? 'translate-x-5 bg-moss' : 'bg-bone-faint',
          )}
        />
      </span>
    </button>
  )
}

/** A labelled form control. The label is always visible, never a placeholder. */
export function FormField({
  label,
  hint,
  error,
  htmlFor,
  children,
}: {
  label: ReactNode
  hint?: ReactNode
  error?: ReactNode
  htmlFor: string
  children: ReactNode
}) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={htmlFor} className="label block">
        {label}
      </label>
      {children}
      {hint && !error ? <p className="text-xs text-bone-faint">{hint}</p> : null}
      {error ? (
        <p role="alert" className="flex items-center gap-1.5 text-xs text-vermillion">
          <Glyph name="alert" className="size-3.5 shrink-0" />
          {error}
        </p>
      ) : null}
    </div>
  )
}
