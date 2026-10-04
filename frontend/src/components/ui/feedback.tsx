/**
 * What a screen says while it has nothing to show, and when it asks.
 */

import { useEffect, useRef, type ReactNode } from 'react'
import { cn } from '../../lib/cn'

import { Glyph } from './glyphs'

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
  size = 'md',
}: {
  open: boolean
  title: ReactNode
  onClose: () => void
  children: ReactNode
  footer: ReactNode
  /** A question fits a column; a choice among pictures wants the room. */
  size?: 'md' | 'lg'
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
        'm-auto max-h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] overflow-hidden',
        size === 'lg' ? 'max-w-3xl' : 'max-w-md',
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
