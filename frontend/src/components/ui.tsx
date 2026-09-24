/**
 * The pieces every screen is built from.
 *
 * A catalogue page separates with rules rather than enclosing in boxes, so the
 * panel here is a hairline frame, not a floating card with a shadow. Radii are
 * nearly square: the register is printed matter.
 */

import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Link } from 'react-router'

import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { genreLabel } from '../lib/labels'

/* ── Text ─────────────────────────────────────────────────────────────────── */

/** The caption above a value on an index card. */
export function Label({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn('label', className)}>{children}</span>
}

export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span className={cn('font-mono text-[0.8125rem] tabular-nums', className)}>{children}</span>
  )
}

/** A section heading: rule, then the name in small caps, then the content. */
export function SectionTitle({
  children,
  action,
  className,
}: {
  children: ReactNode
  action?: ReactNode
  className?: string
}) {
  return (
    <div className={cn('mb-5', className)}>
      <div className="flex items-baseline justify-between gap-4 pb-3">
        <h2 className="font-display text-xl font-medium tracking-tight text-bone sm:text-2xl">
          {children}
        </h2>
        {action}
      </div>
      <div
        aria-hidden
        className="h-px bg-[linear-gradient(to_right,var(--color-vermillion),color-mix(in_srgb,var(--color-vermillion)_35%,var(--color-rule))_18%,var(--color-rule)_55%,transparent)]"
      />
    </div>
  )
}

/* ── Containers ───────────────────────────────────────────────────────────── */

/**
 * A panel.
 *
 * With a `label` it is a landmark a screen reader can jump to; without one it
 * is a plain box. That is the rule rather than a preference: an unnamed
 * `<section>` is not a region to assistive technology, so rendering one would
 * only add an element nobody can navigate to and everybody has to step through.
 */
export function Panel({
  children,
  className,
  style,
  label,
}: {
  children: ReactNode
  className?: string
  style?: React.CSSProperties
  label?: string
}) {
  const shell = cn('plate rounded-panel border border-rule bg-ink-raised', className)

  if (!label) {
    return (
      <div style={style} className={shell}>
        {children}
      </div>
    )
  }

  return (
    <section aria-label={label} style={style} className={shell}>
      {children}
    </section>
  )
}

export function PanelHead({ title, action }: { title: ReactNode; action?: ReactNode }) {
  return (
    <header className="flex items-center justify-between gap-4 border-b border-rule px-5 py-3">
      <Label>{title}</Label>
      {action}
    </header>
  )
}

/**
 * The fact table.
 *
 * Exists because the administration side is four history tables that must read
 * as one: the row height, the hairline between rows and the right-aligned
 * figures are decided here rather than re-derived on each screen. The scroller
 * is part of the set for a reason — a table wide enough to need it must scroll
 * inside its own frame, never by dragging the page sideways.
 */
export function TableScroll({
  children,
  className,
  label,
}: {
  children: ReactNode
  className?: string
  /** What the table is, for the region a keyboard user lands on. */
  label: string
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [overflowing, setOverflowing] = useState(false)

  // A tab stop only while there is something to scroll to. On a phone these
  // tables are wider than the screen, and on the screens whose rows hold no
  // link or button — the job history, the audit trail — a keyboard had no way
  // to reach the columns off the edge. On a desktop, where nothing overflows,
  // an extra stop per table would only be in the way.
  useEffect(() => {
    const element = ref.current
    if (!element) return

    const measure = () => setOverflowing(element.scrollWidth > element.clientWidth + 1)
    measure()

    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  return (
    <div
      ref={ref}
      role={overflowing ? 'region' : undefined}
      aria-label={overflowing ? label : undefined}
      tabIndex={overflowing ? 0 : undefined}
      className={cn('w-full max-w-full overflow-x-auto overscroll-x-contain', className)}
    >
      {children}
    </div>
  )
}

export function Th({
  children,
  align = 'left',
  className,
}: {
  children?: ReactNode
  align?: 'left' | 'right'
  className?: string
}) {
  return (
    <th
      scope="col"
      className={cn(
        'label border-b border-rule px-4 py-2.5 whitespace-nowrap',
        align === 'right' ? 'text-right' : 'text-left',
        className,
      )}
    >
      {children}
    </th>
  )
}

export function Td({
  children,
  align = 'left',
  className,
}: {
  children?: ReactNode
  align?: 'left' | 'right'
  className?: string
}) {
  return (
    <td
      className={cn(
        'px-4 py-2 align-middle text-sm text-bone-dim',
        align === 'right' ? 'text-right' : 'text-left',
        className,
      )}
    >
      {children}
    </td>
  )
}

/** A body row: 45px of it, and a colour change on hover. Nothing moves. */
export function Tr({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <tr
      className={cn(
        // Positioned so a row's primary link can stretch a pseudo-element over
        // the whole row: a title is 20px tall and a row is not, and the gap is
        // where taps land on nothing.
        'relative h-[45px] border-b border-rule transition-colors duration-150 last:border-0',
        'hover:bg-ink-high',
        className,
      )}
    >
      {children}
    </tr>
  )
}

/** A label/value row, the unit a catalogue entry is made of. */
export function Field({
  label,
  children,
  hint,
}: {
  label: ReactNode
  children: ReactNode
  hint?: ReactNode
}) {
  // A term and its description, for a `<dl>` — which is where every use of
  // this sits. It used to be a span and a div, so a screen reader announced a
  // definition list and then read it without a single term in it.
  return (
    <div className="flex items-baseline justify-between gap-6 px-5 py-2.5">
      <dt className="label shrink-0">{label}</dt>
      <dd className="min-w-0 text-right text-sm text-bone">
        {children}
        {hint ? <div className="mt-0.5 text-xs text-bone-faint">{hint}</div> : null}
      </dd>
    </div>
  )
}

/* ── Controls ─────────────────────────────────────────────────────────────── */

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
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

/* ── Markers ──────────────────────────────────────────────────────────────── */

type ChipTone = 'neutral' | 'manual' | 'provider' | 'accent'

const CHIP_TONE: Record<ChipTone, string> = {
  neutral: 'border-rule-bright text-bone-dim',
  manual: 'border-brass-deep text-brass',
  provider: 'border-slate-deep text-slate',
  accent: 'border-vermillion-deep text-vermillion',
}

export function Chip({
  children,
  tone = 'neutral',
  className,
}: {
  children: ReactNode
  tone?: ChipTone
  className?: string
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1',
        'font-mono text-[0.6875rem] font-medium tracking-[0.12em] uppercase',
        CHIP_TONE[tone],
        className,
      )}
    >
      {children}
    </span>
  )
}

/**
 * A genre, in its own colour.
 *
 * The tint is a stable function of the name, so Drama is the same green on
 * every page and in every language of the interface — and a crime thriller does
 * not look like a comedy. It carries no meaning beyond identity, which is why
 * the name is always written out rather than the colour standing for it.
 */
export function Genre({
  name,
  to,
  className,
}: {
  name: string
  /** Where the genre leads — the catalogue narrowed to it — when it is a link. */
  to?: string
  className?: string
}) {
  const { lang } = useI18n()
  // The tint follows the stored name, so a genre keeps its colour whichever
  // language it is read in.
  const stock = stockOf(name)

  const style = {
    color: `var(--color-stock-${stock})`,
    borderColor: `color-mix(in srgb, var(--color-stock-${stock}) 35%, transparent)`,
    backgroundColor: `color-mix(in srgb, var(--color-stock-${stock}) 10%, transparent)`,
  }
  const classes = cn(
    'inline-flex items-center rounded-full border px-2.5 py-1 text-xs font-medium',
    'transition-colors duration-150',
    to && 'hover:border-current focus-visible:outline-offset-2',
    className,
  )

  return to ? (
    <Link to={to} style={style} className={classes}>
      {genreLabel(name, lang)}
    </Link>
  ) : (
    <span style={style} className={classes}>
      {genreLabel(name, lang)}
    </span>
  )
}

/** One of eight film-stock tints, the same one every time for a given name. */
function stockOf(name: string): number {
  let hash = 0
  for (let index = 0; index < name.length; index += 1) {
    hash = (hash * 31 + name.charCodeAt(index)) | 0
  }

  return (Math.abs(hash) % 8) + 1
}

/**
 * Who decided this value.
 *
 * Carries a glyph and a word as well as its colour: someone who cannot tell
 * brass from slate still has to be able to tell a person's edit from a
 * provider's answer, since that distinction is the point of this server.
 */
export function Provenance({ manual, label }: { manual: boolean; label: string }) {
  return (
    <Chip tone={manual ? 'manual' : 'provider'}>
      <Glyph name={manual ? 'lock' : 'cloud'} className="size-3" />
      {label}
    </Chip>
  )
}

/* ── States ───────────────────────────────────────────────────────────────── */

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

/**
 * Asking before something cannot be taken back.
 *
 * Built on `<dialog>`: the platform brings the focus trap, the Escape key and
 * an inert background, none of which is worth reimplementing for the half-dozen
 * places this interface has to ask. Being in the top layer, it also escapes the
 * `overflow-x-clip` the shells impose on everything else.
 *
 * The body is what scrolls, not the dialog: a form long enough to overflow a
 * phone would otherwise push its own footer past the bottom of the screen, and
 * the way out of a dialog has to be on screen the whole time it is open. The
 * two caps have to agree — the element's, so the browser centres a short one on
 * its content, and the column's, so a tall one gives the middle the remainder.
 */
export function Dialog({
  open,
  title,
  onClose,
  children,
  footer,
}: {
  open: boolean
  title: ReactNode
  onClose: () => void
  children: ReactNode
  footer: ReactNode
}) {
  const ref = useRef<HTMLDialogElement>(null)

  useEffect(() => {
    const node = ref.current
    if (!node) return

    if (open && !node.open) node.showModal()
    if (!open && node.open) node.close()
  }, [open])

  return (
    <dialog
      ref={ref}
      onClose={onClose}
      className={cn(
        'm-auto max-h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] max-w-md overflow-hidden',
        'rounded-panel border border-rule bg-ink-raised p-0 text-bone backdrop:bg-ink/80',
      )}
    >
      {open ? (
        <div className="fade-in flex max-h-[calc(100dvh-2rem)] flex-col">
          <header className="shrink-0 border-b border-rule px-5 py-4">
            <h2 className="font-display text-lg font-medium text-bone">{title}</h2>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 py-4 text-sm leading-relaxed text-bone-dim">
            {children}
          </div>
          <footer className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-rule px-5 py-3">
            {footer}
          </footer>
        </div>
      ) : null}
    </dialog>
  )
}

export function Skeleton({ className }: { className?: string }) {
  return <div className={cn('shimmer rounded-card bg-ink-high', className)} />
}

export function EmptyState({
  title,
  hint,
  action,
}: {
  title: ReactNode
  hint?: ReactNode
  action?: ReactNode
}) {
  return (
    <div className="flex flex-col items-center gap-3 px-6 py-16 text-center">
      <Glyph name="reel" className="size-8 text-bone-faint" />
      <p className="font-display text-lg text-bone">{title}</p>
      {hint ? <p className="max-w-sm text-sm text-bone-dim">{hint}</p> : null}
      {action}
    </div>
  )
}

/* ── Glyphs ───────────────────────────────────────────────────────────────── */

/**
 * One stroke weight, one corner treatment, drawn here rather than pulled from a
 * package: the set is small enough that a dependency would cost more than it
 * saves, and every glyph inherits the palette by using `currentColor`.
 */
const PATHS = {
  search: <path d="M11 11 15 15M7 12.5a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11Z" />,
  close: <path d="M4 4 12 12M12 4 4 12" />,
  menu: <path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" />,
  chevronRight: <path d="M6 3.5 10.5 8 6 12.5" />,
  chevronLeft: <path d="M10 3.5 5.5 8 10 12.5" />,
  chevronDown: <path d="M3.5 6 8 10.5 12.5 6" />,
  lock: (
    <>
      <path d="M5 7V5a3 3 0 1 1 6 0v2" />
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.5" />
    </>
  ),
  cloud: <path d="M4.5 12.5a3 3 0 0 1 .3-6 4 4 0 0 1 7.6 1.2 2.4 2.4 0 0 1-.4 4.8Z" />,
  alert: <path d="M8 5v4M8 11.5v.01M8 2 1.5 13.5h13Z" />,
  reel: (
    <>
      <circle cx="8" cy="8" r="6" />
      <circle cx="8" cy="5.2" r="1.3" />
      <circle cx="10.4" cy="9.4" r="1.3" />
      <circle cx="5.6" cy="9.4" r="1.3" />
    </>
  ),
  film: (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M5.5 3v10M10.5 3v10" />
    </>
  ),
  tv: (
    <>
      <rect x="2" y="4.5" width="12" height="8" rx="1.5" />
      <path d="M5.5 2 8 4.5 10.5 2" />
    </>
  ),
  star: <path d="m8 2 1.8 3.9 4.2.5-3.1 2.9.8 4.2L8 11.4 4.3 13.5l.8-4.2L2 6.4l4.2-.5Z" />,
  settings: (
    <>
      <circle cx="8" cy="8" r="2.2" />
      <path d="M8 1.5v1.8M8 12.7v1.8M14.5 8h-1.8M3.3 8H1.5M12.6 3.4l-1.3 1.3M4.7 11.3l-1.3 1.3M12.6 12.6l-1.3-1.3M4.7 4.7 3.4 3.4" />
    </>
  ),
  user: (
    <>
      <circle cx="8" cy="5.5" r="2.8" />
      <path d="M2.8 14a5.2 5.2 0 0 1 10.4 0" />
    </>
  ),
  globe: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M2 8h12M8 2a9 9 0 0 1 0 12M8 2a9 9 0 0 0 0 12" />
    </>
  ),
  arrowLeft: <path d="M13 8H3M6.5 4.5 3 8l3.5 3.5" />,
  external: <path d="M9 3h4v4M13 3 7 9M11.5 9.5v3h-9v-9h3" />,

  /* The administration side: one glyph per section of the building, then the
     verbs an operator performs once inside it. */
  gauge: (
    <>
      <path d="M2.5 12a5.5 5.5 0 1 1 11 0" />
      <path d="M8 12 10.9 8.1" />
    </>
  ),
  list: <path d="M5.5 4h8M5.5 8h8M5.5 12h8M2.6 4h.01M2.6 8h.01M2.6 12h.01" />,
  key: (
    <>
      <circle cx="10.5" cy="5.5" r="3" />
      <path d="M8.4 7.6 2.5 13.5M4.6 11.4 6.4 13.2" />
    </>
  ),
  clock: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.5V8l2.6 1.8" />
    </>
  ),
  journal: (
    <>
      <rect x="3.5" y="2" width="9" height="12" rx="1.5" />
      <path d="M6 5.5h4M6 8h4M6 10.5h2.5" />
    </>
  ),
  signOut: <path d="M6.5 14h-3a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1h3M10.5 11.5 14 8l-3.5-3.5M14 8H6" />,
  plus: <path d="M8 3v10M3 8h10" />,
  trash: (
    <>
      <path d="M2.5 4.5h11" />
      <path d="M6.3 4.5V3.2a1 1 0 0 1 1-1h1.4a1 1 0 0 1 1 1v1.3" />
      <path d="M4.1 4.5l.6 8.2a1 1 0 0 0 1 .9h4.6a1 1 0 0 0 1-.9l.6-8.2" />
    </>
  ),
  refresh: (
    <>
      <path d="M13.5 8a5.5 5.5 0 1 1-5.5-5.5c1.5 0 2.9.6 4 1.6l1.5 1.4" />
      <path d="M13.5 2v3.5H10" />
    </>
  ),
  copy: (
    <>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
      <path d="M10.5 5.5v-2a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2" />
    </>
  ),
  check: <path d="M3 8.5 6.4 12 13 4.4" />,
  unlock: (
    <>
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.5" />
      <path d="M5 7V5a3 3 0 0 1 5.9-.8" />
    </>
  ),
  power: <path d="M8 2.5v5.6M11.9 4.7a5.2 5.2 0 1 1-7.8 0" />,
  download: <path d="M8 2.5v8M4.6 7.4 8 10.8l3.4-3.4M2.5 13.5h11" />,
  database: (
    <>
      <ellipse cx="8" cy="3.8" rx="5.5" ry="2" />
      <path d="M2.5 3.8v8.4c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2V3.8" />
      <path d="M13.5 8c0 1.1-2.5 2-5.5 2s-5.5-.9-5.5-2" />
    </>
  ),
  /** A value that follows another scope's, rather than being set here. */
  link: (
    <>
      <path d="M6.4 9.6 9.6 6.4" />
      <path d="M7.2 4.7 8.6 3.3a2.9 2.9 0 0 1 4.1 4.1l-1.4 1.4" />
      <path d="M8.8 11.3 7.4 12.7a2.9 2.9 0 0 1-4.1-4.1l1.4-1.4" />
    </>
  ),
  /** Searching a provider in order to take something from it. */
  discover: (
    <>
      <path d="M11 11 15 15" />
      <circle cx="7" cy="7" r="5.5" />
      <path d="M7 4.6v4.8M4.6 7h4.8" />
    </>
  ),
  play: <path d="M5 3.3v9.4l7.6-4.7Z" />,
  calendar: (
    <>
      <path d="M2.5 4.5h11v8.8h-11Z" />
      <path d="M2.5 7.2h11M5.5 2.7v3M10.5 2.7v3" />
    </>
  ),
  image: (
    <>
      <path d="M2.5 3.5h11v9h-11Z" />
      <path d="m2.5 10.8 3.1-3.1 2.6 2.6 1.9-1.9 3.4 3.4" />
      <path d="M10.6 6.3h.01" />
    </>
  ),
  sliders: <path d="M2.5 4.5h6.5M12 4.5h1.5M2.5 11.5h1.5M7 11.5h6.5M10.5 3v3M5.5 10v3" />,
  sort: <path d="M5 2.8v10.4M2.9 11.1 5 13.2l2.1-2.1M11 13.2V2.8M8.9 4.9 11 2.8l2.1 2.1" />,
  pencil: (
    <>
      <path d="M2.5 13.5 3 10.8l7.4-7.4a1.7 1.7 0 0 1 2.4 2.4l-7.4 7.4Z" />
      <path d="M9.6 4.2 12 6.6" />
    </>
  ),
} as const

export type GlyphName = keyof typeof PATHS

export function Glyph({
  name,
  className,
  title,
}: {
  name: GlyphName
  className?: string
  title?: string
}) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : true}
      role={title ? 'img' : undefined}
      className={cn('size-4 shrink-0', className)}
    >
      {title ? <title>{title}</title> : null}
      {PATHS[name]}
    </svg>
  )
}
