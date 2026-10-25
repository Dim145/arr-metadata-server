/**
 * What a form is made of, and the fields around it.
 */

import { type ReactNode } from 'react'
import { cn } from '../../lib/cn'

import { Glyph } from './glyphs'

/**
 * What every field shares — an edge, a fill, and how each state changes them.
 *
 * States, for `Input`, `Textarea` and `Select` alike:
 * - rest: the page's ink inside a rule, bone words, a placeholder in the
 *   faintest bone (a `Select` has its chevron).
 * - hover: the edge brightens.
 * - focus: the edge turns bone. The ring the field hides is `hidden` rather
 *   than `none`: invisible in the palette, drawn where forced colours paint
 *   every edge alike.
 * - active: as focus.
 * - disabled: at 45%, with the not-allowed cursor; the edge stays a rule
 *   under the pointer. It had been drawn as an open field.
 * - loading: none of its own — the form's button waits.
 * - error: the edge in the error's colour while the field says it is wrong
 *   (`aria-invalid`), and the words under it from `FormField`. When to say
 *   so is the page's: once the field has been typed in and left, or the form
 *   sent (see routes/fields.ts).
 * - empty: the placeholder, which is never the field's only name.
 */
const FIELD =
  'rounded-card border border-rule bg-ink text-sm text-bone transition-colors duration-200 ' +
  'placeholder:text-bone-faint hover:border-rule-bright focus:border-bone-dim focus:outline-hidden ' +
  'aria-invalid:border-status-error ' +
  'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:border-rule'

export function Input({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={cn('min-h-11 w-full px-3', FIELD, className)} />
}

/**
 * Exists because the administration side edits prose — an overview, a note —
 * and a one-line box turns a paragraph into a keyhole. Same frame as `Input`,
 * so a form mixing the two reads as one form, and the same states (`FIELD`).
 */
export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea {...props} className={cn('w-full px-3 py-2.5 leading-relaxed', FIELD, className)} />
}

/**
 * A choice from a list: the browser's own select, its list drawn in the
 * room's colours where the engine allows it (`.select-base`, styles.css) and
 * the system's elsewhere. Full width unless told otherwise — a drawn list
 * does not size itself to its longest option, so a narrower field wants a
 * width or a `min-w-` of its own. The same states as `Input` (`FIELD`); its
 * list, where the room's colours draw it, marks the option under the
 * pointer with the high ink and the chosen one with the accent's tick.
 */
export function Select({ className, ...props }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      {...props}
      // The chevron is the stylesheet's (`.select-base`), drawn in each theme's
      // colours. Centred: where the engine draws the list, the field is laid
      // out as a row, which left its words at the top.
      className={cn('select-base min-h-11 w-full cursor-pointer items-center px-3 pr-9', FIELD, className)}
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
 *
 * States:
 * - rest, off: the track on the page's ink inside a bright rule, the knob
 *   faint at the start.
 * - rest, on: the track washed in the colour of what is well, edged with it,
 *   the knob in it at the end — on is a state, so it reads `status-ok`.
 * - hover: the high ink behind the 44px target.
 * - focus: the ring round the target.
 * - active: as hover; the knob slides by `transform`, in 150ms.
 * - disabled: at 45%, with the not-allowed cursor and no hover.
 * - loading, error, empty: none of its own. A switch that saves at once is
 *   disabled while it does, and says what came of it beside it.
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
      {/* Under forced colours the track and the knob would both take the
          page's colour and vanish, leaving the switch's state to nothing but
          the knob's place: there the knob is drawn in the button colour, and
          a switch that is on fills with the system's selection colour. */}
      <span
        aria-hidden
        className={cn(
          'relative block h-6 w-11 rounded-full border transition-colors duration-150',
          checked
            ? 'border-status-ok bg-status-ok/25 forced-colors:border-[color:Highlight] forced-colors:bg-[color:Highlight]'
            : 'border-rule-bright bg-ink forced-colors:border-[color:ButtonText]',
        )}
      >
        {/* The knob travels by transform rather than by `left`: the second
            would lay the row out again on every frame of a 150ms slide. */}
        <span
          className={cn(
            'absolute top-1/2 left-0.5 size-4 -translate-y-1/2 rounded-full',
            'transition-[transform,background-color] duration-150',
            checked
              ? 'translate-x-5 bg-status-ok forced-colors:bg-[color:HighlightText]'
              : 'bg-bone-faint forced-colors:bg-[color:ButtonText]',
          )}
        />
      </span>
    </button>
  )
}

/**
 * A labelled form control. The label is always visible, never a placeholder.
 *
 * States: the control's own, and two of its words.
 * - rest: the name above in the label's mono capitals, the control, a hint
 *   under it in the faintest bone.
 * - error: the hint gives way to what is wrong, after the alert glyph, in
 *   the error's colour, said once to a screen reader (`role="alert"`). The
 *   control wears its error edge when the page marks it `aria-invalid`.
 * - disabled, loading, empty: the control's.
 */
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
        <p role="alert" className="flex items-center gap-1.5 text-xs text-status-error">
          <Glyph name="alert" className="size-3.5 shrink-0" />
          {error}
        </p>
      ) : null}
    </div>
  )
}
