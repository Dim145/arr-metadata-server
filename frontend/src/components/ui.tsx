import type { ComponentProps, ReactNode } from 'react'
import { cn } from '../lib/cn'

/* ── Type ─────────────────────────────────────────────────────────────────── */

export function Display({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <h1 className={cn('font-display text-4xl leading-[1.05] tracking-tight text-paper', className)}>
      {children}
    </h1>
  )
}

/** Small caps label. Used for every section header and field name. */
export function Label({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'font-mono text-[10px] uppercase tracking-[0.18em] text-faint select-none',
        className,
      )}
    >
      {children}
    </span>
  )
}

export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn('font-mono text-[13px] tabular', className)}>{children}</span>
}

/* ── Containers ───────────────────────────────────────────────────────────── */

export function Panel({
  children,
  className,
  ...rest
}: ComponentProps<'section'>) {
  return (
    <section
      className={cn('border border-line bg-surface rounded-[2px]', className)}
      {...rest}
    >
      {children}
    </section>
  )
}

export function PanelHead({ title, aside }: { title: ReactNode; aside?: ReactNode }) {
  return (
    <header className="flex items-baseline justify-between gap-4 border-b border-line px-5 py-3">
      <Label>{title}</Label>
      {aside}
    </header>
  )
}

/* ── Controls ─────────────────────────────────────────────────────────────── */

type ButtonVariant = 'primary' | 'ghost' | 'danger'

const buttonStyles: Record<ButtonVariant, string> = {
  primary:
    'bg-phos text-void hover:bg-phos-glow active:translate-y-px disabled:bg-phos-dim disabled:text-void/60',
  ghost:
    'border border-line text-dim hover:border-line-bright hover:text-paper active:translate-y-px',
  danger: 'border border-rust/40 text-rust hover:bg-rust hover:text-void active:translate-y-px',
}

export function Button({
  variant = 'ghost',
  className,
  ...rest
}: ComponentProps<'button'> & { variant?: ButtonVariant }) {
  return (
    <button
      className={cn(
        'inline-flex items-center gap-2 rounded-[2px] px-3 py-1.5',
        'font-mono text-[11px] uppercase tracking-[0.12em]',
        'transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-60',
        buttonStyles[variant],
        className,
      )}
      {...rest}
    />
  )
}

export function Input({ className, ...rest }: ComponentProps<'input'>) {
  return (
    <input
      className={cn(
        'w-full rounded-[2px] border border-line bg-pit px-3 py-2',
        'text-[14px] text-paper placeholder:text-faint',
        'transition-colors focus:border-phos focus:outline-none',
        className,
      )}
      {...rest}
    />
  )
}

export function Textarea({ className, ...rest }: ComponentProps<'textarea'>) {
  return (
    <textarea
      className={cn(
        'w-full rounded-[2px] border border-line bg-pit px-3 py-2',
        'text-[14px] leading-relaxed text-paper placeholder:text-faint',
        'transition-colors focus:border-phos focus:outline-none',
        className,
      )}
      {...rest}
    />
  )
}

export function Select({ className, ...rest }: ComponentProps<'select'>) {
  return (
    <select
      className={cn(
        'rounded-[2px] border border-line bg-pit px-3 py-2',
        'font-mono text-[12px] text-paper',
        'transition-colors focus:border-phos focus:outline-none',
        className,
      )}
      {...rest}
    />
  )
}

/* ── Indicators ───────────────────────────────────────────────────────────── */

type Tone = 'neutral' | 'manual' | 'auto' | 'good' | 'bad'

const toneStyles: Record<Tone, string> = {
  neutral: 'border-line text-dim',
  manual: 'border-phos/45 text-phos',
  auto: 'border-signal/40 text-signal',
  good: 'border-sage/40 text-sage',
  bad: 'border-rust/45 text-rust',
}

export function Tag({
  tone = 'neutral',
  children,
  className,
}: {
  tone?: Tone
  children: ReactNode
  className?: string
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-[2px] border px-1.5 py-0.5',
        'font-mono text-[10px] uppercase tracking-[0.12em] whitespace-nowrap',
        toneStyles[tone],
        className,
      )}
    >
      {children}
    </span>
  )
}

/** The padlock. This app's whole premise in 14 pixels. */
export function Lock({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden
      className={cn('h-[13px] w-[13px] shrink-0', className)}
      fill="none"
    >
      <path
        d="M8 10V7a4 4 0 0 1 8 0v3"
        stroke="currentColor"
        strokeWidth="2.2"
        strokeLinecap="round"
      />
      <rect x="5" y="10" width="14" height="10" rx="2" fill="currentColor" />
    </svg>
  )
}

export function Spinner({ className }: { className?: string }) {
  return (
    <span
      role="status"
      aria-label="Loading"
      className={cn(
        'inline-block h-3.5 w-3.5 animate-spin rounded-full',
        'border-[1.5px] border-line-bright border-t-phos',
        className,
      )}
    />
  )
}

export function Empty({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="flex flex-col items-center gap-2 px-6 py-16 text-center">
      <p className="font-display text-2xl text-dim">{title}</p>
      {hint && <p className="max-w-sm text-[13px] text-faint">{hint}</p>}
    </div>
  )
}

export function Alert({ children, tone = 'bad' }: { children: ReactNode; tone?: Tone }) {
  return (
    <div
      className={cn(
        'rounded-[2px] border px-3 py-2 text-[13px]',
        tone === 'bad' && 'border-rust/40 bg-rust/8 text-rust',
        tone === 'good' && 'border-sage/40 bg-sage/8 text-sage',
        tone === 'manual' && 'border-phos/40 bg-phos/8 text-phos',
        tone === 'neutral' && 'border-line text-dim',
        tone === 'auto' && 'border-signal/40 text-signal',
      )}
    >
      {children}
    </div>
  )
}
