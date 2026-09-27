/**
 * Buttons of every weight, the link that must look like one, and the spinner
 * they show while waiting.
 */

import { cn } from '../../lib/cn'

import { Glyph, type GlyphName } from './glyphs'

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  /** The element itself, for whoever has to give it focus back. */
  ref?: React.Ref<HTMLButtonElement>
  variant?: 'primary' | 'ghost' | 'quiet' | 'danger'
  size?: 'sm' | 'md'
}

const BUTTON_BASE =
  'inline-flex items-center justify-center gap-2 rounded-full font-sans font-medium ' +
  'transition-colors duration-200 cursor-pointer select-none ' +
  'disabled:cursor-not-allowed disabled:opacity-45'

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
  sm: 'min-h-11 px-3 text-[0.8125rem]',
  md: 'min-h-11 px-5 text-sm',
}

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
 */
export function ButtonLink({
  variant = 'ghost',
  size = 'md',
  className,
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
    />
  )
}

/**
 * A row action, reduced to its glyph.
 *
 * Exists because a register's action column has no room for four verbs, and a
 * `Button` with only an icon in it loses the thing that makes it usable — its
 * name. Here the name is compulsory: it reaches the screen reader through
 * `aria-label` and the mouse through the tooltip, and the target stays 44px
 * whatever the glyph measures.
 */
export function IconButton({
  glyph,
  label,
  onClick,
  busy,
  disabled,
  tone = 'quiet',
  className,
}: {
  glyph: GlyphName
  label: string
  onClick: () => void
  busy?: boolean
  disabled?: boolean
  tone?: 'quiet' | 'danger'
  className?: string
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={busy || disabled}
      aria-label={label}
      title={label}
      className={cn(
        'grid size-11 shrink-0 cursor-pointer place-items-center rounded-card text-bone-faint',
        'transition-colors duration-150 hover:bg-ink-high',
        'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent',
        tone === 'danger' ? 'hover:text-vermillion' : 'hover:text-bone',
        className,
      )}
    >
      {busy ? <Spinner className="size-4" /> : <Glyph name={glyph} className="size-4" />}
    </button>
  )
}

/**
 * Work in flight, at the size of the text beside it.
 *
 * Exists because the administration side waits on the network in places too
 * small for a skeleton — inside a button, beside a panel head. Always
 * decorative: whatever it sits next to says in words what is happening.
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
