/**
 * The frames a page divides into, and its own index.
 */

import { useEffect, useRef, useState, type ReactNode } from 'react'
import { cn } from '../../lib/cn'

import { Label } from './typography'

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
  id,
}: {
  children: ReactNode
  className?: string
  style?: React.CSSProperties
  label?: string
  /** An anchor, for a page's own index (`OnThisPage`) to lead to. */
  id?: string
}) {
  const shell = cn(
    'plate rounded-panel border border-rule bg-ink-raised',
    // Room for the bars that stay at the top when the page scrolls to it.
    id && 'scroll-mt-28 lg:scroll-mt-16',
    className,
  )

  if (!label) {
    return (
      <div id={id} style={style} className={shell}>
        {children}
      </div>
    )
  }

  return (
    <section id={id} aria-label={label} style={style} className={shell}>
      {children}
    </section>
  )
}

/**
 * A long page's own index: its sections in a row that stays at the top, the
 * one in view marked. The work editor is ten panels and a screen or four
 * tall; getting from the fields to the sources meant scrolling past every
 * episode. Anchors, so the address and the back button know where the reader
 * went.
 */
export function OnThisPage({
  entries,
  label,
}: {
  entries: { id: string; label: string }[]
  label: string
}) {
  const [current, setCurrent] = useState<string | null>(null)
  const row = useRef<HTMLOListElement>(null)
  const ids = entries.map((entry) => entry.id).join(' ')

  // On a phone the row is wider than the screen: it follows the reader, so
  // the section in view is the one whose name shows.
  useEffect(() => {
    const list = row.current
    const active = list?.querySelector<HTMLElement>('[aria-current]')
    if (!list || !active || list.scrollWidth <= list.clientWidth) return
    const left = active.getBoundingClientRect().left - list.getBoundingClientRect().left + list.scrollLeft
    list.scrollTo({ left: left - (list.clientWidth - active.offsetWidth) / 2, behavior: 'smooth' })
  }, [current])

  useEffect(() => {
    if (typeof IntersectionObserver === 'undefined') return
    const targets = ids
      .split(' ')
      .map((id) => document.getElementById(id))
      .filter((element): element is HTMLElement => element !== null)
    if (!targets.length) return

    // The section whose top is nearest the top of the screen, of those
    // crossing a band below the bars.
    const observer = new IntersectionObserver(
      (seen) => {
        const first = seen
          .filter((entry) => entry.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0]
        if (first) setCurrent(first.target.id)
      },
      { rootMargin: '-25% 0px -60% 0px' },
    )
    targets.forEach((target) => observer.observe(target))
    return () => observer.disconnect()
  }, [ids])

  return (
    <nav aria-label={label} className="sticky top-14 z-20 -mx-1 mb-6 border-b border-rule bg-ink/95 px-1 backdrop-blur lg:top-0">
      <ol ref={row} className="flex gap-1 overflow-x-auto py-2 [scrollbar-width:none]">
        {entries.map((entry) => (
          <li key={entry.id} className="shrink-0">
            <a
              href={`#${entry.id}`}
              aria-current={current === entry.id ? 'location' : undefined}
              className={cn(
                'hit inline-flex min-h-9 items-center rounded-full px-3 text-[0.8125rem] transition-colors duration-150',
                current === entry.id ? 'bg-ink-top text-bone' : 'text-bone-dim hover:text-bone',
              )}
            >
              {entry.label}
            </a>
          </li>
        ))}
      </ol>
    </nav>
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
