/**
 * Buttons of every weight, the link that must look like one, and the spinner
 * they show while waiting.
 *
 * Every component in `ui/` says what it looks like in each state a control
 * can be in — at rest, under the pointer (hover), reached by the keyboard
 * (focus), pressed (active), disabled, waiting (loading), failed (error) and
 * with nothing to show (empty) — and the styleguide draws each one
 * (`/__styleguide`, in a development build). A state a component does not
 * have says so, and says whose it is instead.
 */

import { cn } from '../../lib/cn'

import { Glyph, type GlyphName } from './glyphs'
import { useTooltip } from './overlay'

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  /** The element itself, for whoever has to give it focus back. */
  ref?: React.Ref<HTMLButtonElement>
  variant?: 'primary' | 'ghost' | 'quiet' | 'danger'
  size?: 'sm' | 'md'
}

// Under forced colours a button keeps an edge of the system's button colour:
// the palette that drew the primary one is a gradient, which forced colours
// take away, and the quiet one never had an edge — either would be a word
// floating on the page. A disabled one says so in the system's grey.
const BUTTON_BASE =
  'inline-flex items-center justify-center gap-2 rounded-full font-sans font-medium ' +
  'transition-colors duration-200 cursor-pointer select-none ' +
  'disabled:cursor-not-allowed disabled:opacity-45 ' +
  'forced-colors:border forced-colors:border-[color:ButtonText] ' +
  'forced-colors:disabled:border-[color:GrayText] forced-colors:disabled:text-[color:GrayText]'

const BUTTON_VARIANT: Record<NonNullable<ButtonProps['variant']>, string> = {
  primary:
    'bg-[linear-gradient(135deg,var(--color-vermillion),color-mix(in_srgb,var(--color-vermillion)_70%,var(--color-brass)))] ' +
    'text-ink shadow-[var(--shadow-lift)] hover:brightness-110',
  ghost: 'border border-rule-bright text-bone hover:border-bone-faint hover:bg-ink-high',
  quiet: 'text-bone-dim hover:text-bone hover:bg-ink-high',
  danger: 'border border-vermillion-deep text-vermillion hover:bg-vermillion hover:text-ink',
}

// 44px is the smallest target a finger finds reliably; `sm` keeps that height
// and only loses horizontal padding, so a dense toolbar is still tappable.
const BUTTON_SIZE: Record<NonNullable<ButtonProps['size']>, string> = {
  sm: 'min-h-11 px-3 text-body-sm',
  md: 'min-h-11 px-5 text-sm',
}

/**
 * An action, in one of four weights: `primary`, the one thing a screen is
 * for; `ghost`, the rest; `quiet`, what sits in a row of others; `danger`,
 * what cannot be taken back. Danger is still an action, so it wears the
 * accent, not the colour of a failure: an error is something that went
 * wrong, a danger something about to be done.
 *
 * States:
 * - rest: primary, the accent shading into brass, ink words and the lifted
 *   shadow; ghost, a bright rule's edge and bone words; quiet, no edge and
 *   dim words; danger, the deep accent's edge and the accent's words.
 * - hover: primary 10% brighter; ghost and quiet on the high ink, the
 *   ghost's edge lighter, the quiet words bone; danger filled with the
 *   accent, its words ink.
 * - focus: the ring every control has (styles.css, `:focus-visible`), the
 *   accent two pixels out, for the keyboard alone.
 * - active: as hover. Nothing moves or shrinks under a press.
 * - disabled: at 45%, with the not-allowed cursor and no hover; under forced
 *   colours, the system's grey.
 * - loading: the caller's — `disabled`, a `Spinner` before words that say
 *   what is under way ("Saving…").
 * - error, empty: none of its own; the form's `FormField`, or an alert line
 *   beside the button, says what failed.
 */
export function Button({ variant = 'ghost', size = 'md', className, ...props }: ButtonProps) {
  return (
    <button
      type="button"
      {...props}
      className={cn(BUTTON_BASE, BUTTON_VARIANT[variant], BUTTON_SIZE[size], className)}
    />
  )
}

/**
 * A link that has to look like a button.
 *
 * Exists because a `<Button>` wrapped in an `<a>` is invalid markup and a
 * `<button>` that navigates lies to everyone who reads the page with anything
 * other than their eyes. Downloads and the documentation are navigations, so
 * they are anchors, dressed from the same three tables above.
 *
 * States: as `Button`'s, but for two. A link cannot be disabled, so it has
 * no such state: a way that is not open is not drawn. And it does not wait —
 * the browser shows a navigation under way.
 */
export function ButtonLink({
  variant = 'ghost',
  size = 'md',
  className,
  children,
  ...props
}: React.AnchorHTMLAttributes<HTMLAnchorElement> & {
  variant?: ButtonProps['variant']
  size?: ButtonProps['size']
}) {
  return (
    <a
      {...props}
      className={cn(
        BUTTON_BASE,
        BUTTON_VARIANT[variant ?? 'ghost'],
        BUTTON_SIZE[size ?? 'md'],
        'no-underline',
        className,
      )}
    >
      {children}
    </a>
  )
}

/**
 * A row action, reduced to its glyph.
 *
 * Exists because a register's action column has no room for four verbs, and a
 * `Button` with only an icon in it loses the thing that makes it usable — its
 * name. Here the name is compulsory: it reaches the screen reader through
 * `aria-label`, and the mouse and the keyboard through the tooltip (see
 * `useTooltip`), and the target stays 44px whatever the glyph measures.
 *
 * States:
 * - rest: the glyph in faint bone on nothing; `pressed`, in brass on a brass
 *   wash — a toggle a person turned on.
 * - hover: the high ink behind it, the glyph bone — or the accent, for a
 *   `danger` one. Its name shows above it after a rest (`useTooltip`).
 * - focus: the ring, and its name at once.
 * - active: as hover.
 * - disabled: at 45%, with the not-allowed cursor and no hover.
 * - loading (`busy`): a `Spinner` in the glyph's place, and disabled.
 * - error, empty: none of its own; whatever it acted on says it.
 */
export function IconButton({
  glyph,
  label,
  onClick,
  busy,
  disabled,
  tone = 'quiet',
  pressed,
  className,
}: {
  glyph: GlyphName
  label: string
  onClick: () => void
  busy?: boolean
  disabled?: boolean
  tone?: 'quiet' | 'danger'
  /** A toggle's state: said to assistive technology, and shown in brass. */
  pressed?: boolean
  className?: string
}) {
  const { trigger, tip } = useTooltip(label)

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={busy || disabled}
      aria-label={label}
      aria-pressed={pressed}
      {...trigger}
      className={cn(
        'grid size-11 shrink-0 cursor-pointer place-items-center rounded-card text-bone-faint',
        pressed && 'bg-brass/[0.08] text-brass',
        'transition-colors duration-150 hover:bg-ink-high',
        'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent',
        tone === 'danger' ? 'hover:text-vermillion' : 'hover:text-bone',
        // Forced colours take the brass tint away, and with it the only sign
        // a toggle is on: the system's own selection colours say it there.
        'forced-colors:border forced-colors:border-[color:ButtonText]',
        pressed && 'forced-colors:bg-[color:Highlight] forced-colors:text-[color:HighlightText]',
        className,
      )}
    >
      {busy ? <Spinner className="size-4" /> : <Glyph name={glyph} className="size-4" />}
      {tip}
    </button>
  )
}

/**
 * Work in flight, at the size of the text beside it.
 *
 * Exists because the administration side waits on the network in places too
 * small for a skeleton — inside a button, beside a panel head. Always
 * decorative: whatever it sits next to says in words what is happening.
 *
 * States: it is itself the loading state of whatever holds it — a ring at
 * half the text's colour, turning. For a reader who asked for less motion it
 * stands still, and the words beside it carry the meaning.
 */
export function Spinner({ className }: { className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        'inline-block size-4 shrink-0 animate-spin rounded-full',
        'border-2 border-current border-t-transparent opacity-50',
        className,
      )}
    />
  )
}
