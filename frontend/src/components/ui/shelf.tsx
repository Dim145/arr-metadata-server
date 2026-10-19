/**
 * A row that scrolls sideways, with its ends said.
 *
 * A card cut at the edge says there is more — until the cut falls between two
 * cards, where the row looks complete. So the edge fades where the row goes
 * on, and two arrows walk it a screenful at a time: shown to a pointer that
 * rests on the row and to a keyboard that reaches them, always to a finger,
 * which has no hover to show them on. The list itself stays the scroller a
 * finger, a wheel and a tabbing keyboard already had; the arrows are a way
 * along it, not the only one.
 */

import { useCallback, useEffect, useId, useRef, useState, type KeyboardEventHandler, type ReactNode, type Ref } from 'react'

import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'

import { Glyph } from './glyphs'

export function Shelf({
  label,
  children,
  className,
  listClassName,
  tabIndex,
  ref,
  onKeyDown,
}: {
  /** What the row holds, for a screen reader: the list's name. */
  label?: string
  /** The `<li>` items, each sized and snapped as the row wants them. */
  children: ReactNode
  /** The room the row bleeds into: its margins, which the fades sit at. */
  className?: string
  /** The list's own: its padding, its gap. */
  listClassName?: string
  /** For a row of nothing focusable, which a keyboard must still reach. */
  tabIndex?: number
  /** The list itself, for whoever has to scroll it somewhere. */
  ref?: Ref<HTMLUListElement>
  /** What its items' keys do beyond Tab: the arrows between cards, for a row of them. */
  onKeyDown?: KeyboardEventHandler<HTMLUListElement>
}) {
  const { t } = useI18n()
  const id = useId()
  const list = useRef<HTMLUListElement | null>(null)
  const [more, setMore] = useState({ before: false, after: false })

  // Measured, never assumed: a row that fits its room has no arrows, and one
  // scrolled to its end has only the way back.
  const measure = useCallback(() => {
    const element = list.current
    if (!element) return
    const before = element.scrollLeft > 1
    const after = element.scrollLeft + element.clientWidth < element.scrollWidth - 1
    setMore((was) => (was.before === before && was.after === after ? was : { before, after }))
  }, [])

  const attach = useCallback(
    (node: HTMLUListElement | null) => {
      list.current = node
      if (typeof ref === 'function') ref(node)
      else if (ref) ref.current = node
    },
    [ref],
  )

  useEffect(() => {
    const element = list.current
    if (!element) return
    measure()
    element.addEventListener('scroll', measure, { passive: true })
    // The room changes with the window, the row with what it holds — and
    // with what joins it after the first paint.
    const sizes = new ResizeObserver(measure)
    sizes.observe(element)
    const watchChildren = () => {
      for (const child of element.children) sizes.observe(child)
      measure()
    }
    watchChildren()
    const members = new MutationObserver(watchChildren)
    members.observe(element, { childList: true })
    return () => {
      element.removeEventListener('scroll', measure)
      sizes.disconnect()
      members.disconnect()
    }
  }, [measure])

  // A screenful less a card, so the card cut at the edge is the first whole
  // one after the step and the reader keeps their place.
  const step = (direction: -1 | 1) => {
    const element = list.current
    if (!element) return
    const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    element.scrollBy({ left: direction * element.clientWidth * 0.8, behavior: still ? 'auto' : 'smooth' })
  }

  return (
    <div className={cn('group/shelf relative', className)}>
      {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- its items take the focus; the list hears their keys */}
      <ul
        ref={attach}
        id={id}
        aria-label={label || undefined}
        tabIndex={tabIndex}
        onKeyDown={onKeyDown}
        className={cn('flex snap-x snap-proximity scroll-pl-2 overflow-x-auto', listClassName)}
      >
        {children}
      </ul>

      {/* Where the row goes on, its edge dissolves into the page. */}
      <div
        aria-hidden
        className={cn(
          'pointer-events-none absolute inset-y-0 left-0 w-10 bg-gradient-to-r from-ink to-transparent',
          'transition-opacity duration-150',
          more.before ? 'opacity-100' : 'opacity-0',
        )}
      />
      <div
        aria-hidden
        className={cn(
          'pointer-events-none absolute inset-y-0 right-0 w-10 bg-gradient-to-l from-ink to-transparent',
          'transition-opacity duration-150',
          more.after ? 'opacity-100' : 'opacity-0',
        )}
      />

      {more.before || more.after ? (
        <>
          <Arrow direction="before" enabled={more.before} label={t.a11y.shelfBack} list={id} onClick={() => step(-1)} />
          <Arrow direction="after" enabled={more.after} label={t.a11y.shelfOn} list={id} onClick={() => step(1)} />
        </>
      ) : null}
    </div>
  )
}

/**
 * One way along the row. At the end it stays, dimmed and said to be no way
 * on, rather than going: a button that vanishes under the pointer that was
 * pressing it, or under the focus that had it, leaves both with nothing.
 */
function Arrow({
  direction,
  enabled,
  label,
  list,
  onClick,
}: {
  direction: 'before' | 'after'
  enabled: boolean
  label: string
  /** The id of the row it moves. */
  list: string
  onClick: () => void
}) {
  return (
    <button
      type="button"
      aria-label={label}
      aria-controls={list}
      aria-disabled={!enabled}
      onClick={enabled ? onClick : undefined}
      className={cn(
        'absolute top-1/2 grid size-11 -translate-y-1/2 place-items-center rounded-full',
        'border border-rule-bright bg-ink/90 text-bone shadow-[var(--shadow-lift)]',
        'forced-colors:border-[color:ButtonText]',
        'transition-opacity duration-150',
        direction === 'before' ? 'left-1' : 'right-1',
        // Out of the way of a pointer until it rests on the row, and of a
        // keyboard until it reaches them — out of reach as well as out of
        // sight, or an arrow nobody can see takes the tap meant for the card
        // under it. A finger has neither, and sees the way there is to go:
        // not the dimmed one, which would sit on the first card of a row
        // nobody has moved yet.
        'pointer-events-none opacity-0',
        'group-hover/shelf:pointer-events-auto group-hover/shelf:opacity-100',
        'group-focus-within/shelf:pointer-events-auto group-focus-within/shelf:opacity-100',
        enabled
          ? 'cursor-pointer hover:border-bone-faint pointer-coarse:pointer-events-auto pointer-coarse:opacity-100'
          : 'cursor-default',
      )}
    >
      <Glyph name={direction === 'before' ? 'chevronLeft' : 'chevronRight'} className={cn('size-4', !enabled && 'opacity-40')} />
    </button>
  )
}
