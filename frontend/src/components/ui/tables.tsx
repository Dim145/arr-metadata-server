/**
 * The fact table: one register for every history the administration shows.
 */

import { useEffect, useRef, useState, type HTMLAttributes, type ReactNode } from 'react'
import { cn } from '../../lib/cn'

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

/**
 * A body row: 45px of it, and a colour change on hover. Nothing moves. Any
 * other attribute — a `data-` handle on what the row is about — goes through.
 */
export function Tr({
  children,
  className,
  ...rest
}: { children: ReactNode; className?: string } & Omit<HTMLAttributes<HTMLTableRowElement>, 'className' | 'children'>) {
  return (
    <tr
      {...rest}
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
