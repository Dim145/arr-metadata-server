/**
 * Type set as the page's own: labels, figures and section heads.
 *
 * States: none — they are read, and take the states of what holds them. The
 * steps they are set in are the type scale's (styles.css, `@theme`).
 */

import { type ReactNode } from 'react'
import { cn } from '../../lib/cn'

/** The caption above a value on an index card. */
export function Label({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn('label', className)}>{children}</span>
}

export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span className={cn('font-mono text-body-sm tabular-nums', className)}>{children}</span>
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
      {/* A gradient, which forced colours remove: there the rule is drawn
          plain, in the text's colour, so the heading still heads something. */}
      <div
        aria-hidden
        className="h-px bg-[linear-gradient(to_right,var(--color-vermillion),color-mix(in_srgb,var(--color-vermillion)_35%,var(--color-rule))_18%,var(--color-rule)_55%,transparent)] forced-colors:bg-[color:CanvasText]"
      />
    </div>
  )
}
