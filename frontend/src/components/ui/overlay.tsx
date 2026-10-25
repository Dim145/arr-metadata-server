/**
 * What opens over the page beside the thing that asked for it: a panel of
 * actions or controls under its button, and the name of a button that shows
 * only a glyph.
 *
 * Both are popovers where the engine has them — the top layer, so no
 * `z-index` and no scroller's edge cuts them off — and are tied to their
 * button by anchor positioning where the engine draws it, flipped to the side
 * that has the room. Where it does not, they are placed from the button's box
 * by hand, as they were before; where there are no popovers at all, the panel
 * is shown and hidden by its button and the name goes back to the browser's
 * own tooltip. Nothing is downloaded to make up for an older engine.
 */

import { useCallback, useEffect, useId, useRef, useState, type CSSProperties, type ReactNode } from 'react'

import { cn } from '../../lib/cn'

/** Whether this engine keeps popovers itself: the top layer, a press elsewhere and Escape to close. */
export const POPOVER = typeof HTMLElement !== 'undefined' && 'popover' in HTMLElement.prototype

/** Whether it also places one beside what opened it, by anchor positioning. */
export const ANCHORED = POPOVER && typeof CSS !== 'undefined' && CSS.supports('position-area', 'block-end')

/** A name for anchor positioning to tie two elements by, unique to the component that asks. */
function useAnchorName(): string {
  return `--anchor-${useId().replace(/[^\w-]/g, '')}`
}

/** Whether a popover is showing, asked so that a second show or hide is not an error. */
function showing(element: HTMLElement): boolean {
  return element.matches(':popover-open')
}

/* ── Menu ─────────────────────────────────────────────────────────────────── */

/**
 * A button that opens a panel under it: a row's actions behind a glyph, a
 * small set of controls behind a word.
 *
 * Not an ARIA menu. `role="menu"` promises arrow keys between its items and
 * a contract of its own, where these are plain buttons, links and controls,
 * reached with Tab like any other — and the browser sends Tab from the button
 * straight into the panel it opened. A panel of controls rather than of
 * actions is a small dialog and says so, given a `title`.
 *
 * Escape and a press elsewhere close it, and the focus goes back to its
 * button. `children` may be a function, handed the way to close the panel
 * once one of its actions is done.
 *
 * States:
 * - closed: its button alone, drawn by the caller (`buttonClassName`, told
 *   whether the panel is open), with its name shown after a rest.
 * - hover, focus, active, disabled: the button's — the caller's classes, the
 *   ring, and `disabled`, which keeps the panel shut.
 * - open: the panel on the overlay surface, a bright rule round it, the
 *   card's corner and the lifted shadow, dropping a hair from its button
 *   (150ms), on the side with the room.
 * - leaving: lifting away (150ms) where the engine keeps it in the top
 *   layer until it has gone.
 * - loading, error, empty: what it holds says them.
 *
 * Its rows (`MENU_ITEM`): at rest dim words; under the pointer the high ink
 * and bone; reached by the keyboard, the ring; disabled at 45%.
 */
export function Menu({
  label,
  button,
  buttonClassName,
  title,
  align = 'end',
  disabled,
  className,
  children,
}: {
  /** The button's name, for a screen reader, and shown beside it to a pointer that rests on it (`useTooltip`). */
  label: string
  /** What the button shows. */
  button: ReactNode
  /** How the button is drawn, open or not. */
  buttonClassName?: string | ((open: boolean) => string)
  /** What the panel is, when it holds controls rather than actions: it is then a dialog of that name. */
  title?: string
  /** Which edge the panel lines up with: the button's start, or its end. */
  align?: 'start' | 'end'
  disabled?: boolean
  /** The panel's own: its width, its padding. */
  className?: string
  children: ReactNode | ((close: () => void) => ReactNode)
}) {
  const id = useId()
  const anchor = useAnchorName()
  const tooltip = useTooltip(label, anchor)
  const trigger = useRef<HTMLButtonElement>(null)
  const panel = useRef<HTMLDivElement>(null)
  const [open, setOpen] = useState(false)

  const close = useCallback(() => {
    if (POPOVER && panel.current && showing(panel.current)) panel.current.hidePopover()
    setOpen(false)
  }, [])

  // Under the button, or above it where the screen ends first: only where
  // the engine has popovers but cannot anchor one itself.
  const place = useCallback(() => {
    const button = trigger.current
    const menu = panel.current
    if (ANCHORED || !POPOVER || !button || !menu) return
    const at = button.getBoundingClientRect()
    const gap = 4
    const roomBelow = window.innerHeight - at.bottom - gap
    const above = menu.offsetHeight > roomBelow && at.top > roomBelow
    menu.style.top = `${above ? Math.max(gap, at.top - gap - menu.offsetHeight) : at.bottom + gap}px`
    if (align === 'end') {
      menu.style.right = `${Math.max(gap, document.documentElement.clientWidth - at.right)}px`
    } else {
      menu.style.left = `${Math.max(gap, Math.min(at.left, document.documentElement.clientWidth - menu.offsetWidth - gap))}px`
    }
  }, [align])

  // Kept beside its button while what is under it scrolls.
  useEffect(() => {
    if (!open || ANCHORED || !POPOVER) return
    window.addEventListener('scroll', place, { capture: true, passive: true })
    window.addEventListener('resize', place, { passive: true })
    return () => {
      window.removeEventListener('scroll', place, { capture: true })
      window.removeEventListener('resize', place)
    }
  }, [open, place])

  // Without popovers: shut by a press elsewhere, or by Escape.
  useEffect(() => {
    if (POPOVER || !open) return
    const away = (event: PointerEvent) => {
      const target = event.target as Node
      if (!panel.current?.contains(target) && !trigger.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setOpen(false)
      trigger.current?.focus()
    }
    document.addEventListener('pointerdown', away)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('pointerdown', away)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  return (
    <span className="relative inline-flex">
      <button
        ref={trigger}
        type="button"
        aria-label={label}
        aria-expanded={open}
        aria-controls={id}
        disabled={disabled}
        popoverTarget={POPOVER ? id : undefined}
        onClick={POPOVER ? undefined : () => setOpen((was) => !was)}
        {...tooltip.trigger}
        style={ANCHORED ? { anchorName: anchor } : undefined}
        className={typeof buttonClassName === 'function' ? buttonClassName(open) : buttonClassName}
      >
        {button}
        {tooltip.tip}
      </button>
      <div
        ref={panel}
        id={id}
        popover={POPOVER ? 'auto' : undefined}
        hidden={POPOVER ? undefined : !open}
        role={title ? 'dialog' : undefined}
        aria-label={title}
        // Placed before it is drawn, and again once it has a height, in case
        // it has to go above.
        onBeforeToggle={(event) => {
          if (event.newState === 'open') place()
        }}
        onToggle={(event) => {
          const opened = event.newState === 'open'
          setOpen(opened)
          if (opened) place()
        }}
        style={ANCHORED ? { positionAnchor: anchor } : undefined}
        className={cn(
          'm-0 w-max max-w-[min(20rem,calc(100vw-2rem))] rounded-card border border-rule-bright bg-(--surface-overlay) p-1',
          'popover-in text-left text-bone shadow-[var(--shadow-lift)]',
          POPOVER
            ? ANCHORED
              ? cn(
                  'inset-auto mt-1 [position-try-fallbacks:flip-block,flip-inline]',
                  align === 'end' ? '[position-area:block-end_span-inline-start]' : '[position-area:block-end_span-inline-end]',
                )
              : 'inset-auto'
            : cn('absolute top-full z-40 mt-1', align === 'end' ? 'right-0' : 'left-0'),
          className,
        )}
      >
        {typeof children === 'function' ? children(close) : children}
      </div>
    </span>
  )
}

/**
 * One action in a `Menu`: a full-width row, 44 pixels tall, its glyph and
 * its words. A plain button — the panel is not a menu (see `Menu`).
 */
export const MENU_ITEM =
  'flex min-h-11 w-full cursor-pointer items-center gap-2.5 rounded-md px-3 text-left text-sm transition-colors duration-150 ' +
  'text-bone-dim hover:bg-ink-high hover:text-bone ' +
  'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent ' +
  'forced-colors:hover:bg-[color:Highlight] forced-colors:hover:text-[color:HighlightText]'

/* ── Tooltip ──────────────────────────────────────────────────────────────── */

/** How long a pointer rests on a button before its name is shown: long enough not to flicker across a toolbar. */
const TIP_DELAY = 400

/**
 * The name of a control that shows a glyph alone, shown beside it to a mouse
 * that rests on it and to a keyboard that reaches it.
 *
 * The browser's own tooltip, from `title`, was all there was: late, small,
 * never shown to a keyboard, and drawn in the system's colours. This one is
 * the control's name and nothing more, so it is hidden from assistive
 * technology, which has the name already (`aria-label`). It goes with
 * Escape, with the focus, with a press, and stays while the pointer is on it
 * — it is inside the control, so moving onto it is not leaving — and a
 * finger, which cannot rest, is not shown one.
 *
 * A hook rather than a wrapper, so the control keeps its place in whatever
 * row it stands in: spread `trigger` on it and put `tip` inside it.
 *
 * States: hidden; shown — the name in caption size on the top ink, inside
 * a bright rule at the item's corner, above its control or under it where
 * the screen ends first, after 400ms of rest or at once for the keyboard.
 */
export function useTooltip(
  text: string,
  /** The control's anchor name, where it has one already — a menu's button. */
  shared?: string,
): {
  trigger: {
    title?: string
    style?: CSSProperties
    onPointerEnter: (event: React.PointerEvent) => void
    onPointerLeave: () => void
    onPointerDown: () => void
    onFocus: (event: React.FocusEvent) => void
    onBlur: () => void
  }
  tip: ReactNode
} {
  const own = useAnchorName()
  const anchor = shared ?? own
  const tip = useRef<HTMLSpanElement>(null)
  const timer = useRef<number | undefined>(undefined)
  const [open, setOpen] = useState(false)

  const hide = useCallback(() => {
    window.clearTimeout(timer.current)
    const element = tip.current
    if (element && POPOVER && showing(element)) element.hidePopover()
    setOpen(false)
  }, [])

  const show = useCallback(() => {
    window.clearTimeout(timer.current)
    const element = tip.current
    if (!element || !POPOVER) return
    if (!showing(element)) element.showPopover()
    setOpen(true)
    // Above its control, or under it where the screen ends first, and kept
    // inside the screen's width: where anchors are not drawn.
    if (ANCHORED) return
    const control = element.parentElement?.getBoundingClientRect()
    if (!control) return
    const gap = 6
    const width = document.documentElement.clientWidth
    const top = control.top - gap - element.offsetHeight
    element.style.top = `${top < gap ? control.bottom + gap : top}px`
    element.style.left = `${Math.min(Math.max(gap, control.left + control.width / 2 - element.offsetWidth / 2), width - element.offsetWidth - gap)}px`
  }, [])

  // Escape puts a shown name away; a page that scrolls under a placed one
  // would leave it standing over something else.
  useEffect(() => {
    if (!open) return
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') hide()
    }
    document.addEventListener('keydown', escape)
    if (!ANCHORED) window.addEventListener('scroll', hide, { capture: true, passive: true })
    return () => {
      document.removeEventListener('keydown', escape)
      window.removeEventListener('scroll', hide, { capture: true })
    }
  }, [open, hide])

  useEffect(() => () => window.clearTimeout(timer.current), [])

  if (!POPOVER) {
    return {
      trigger: {
        title: text,
        onPointerEnter: () => {},
        onPointerLeave: () => {},
        onPointerDown: () => {},
        onFocus: () => {},
        onBlur: () => {},
      },
      tip: null,
    }
  }

  return {
    trigger: {
      style: ANCHORED ? { anchorName: anchor } : undefined,
      onPointerEnter: (event) => {
        if (event.pointerType !== 'touch') timer.current = window.setTimeout(show, TIP_DELAY)
      },
      onPointerLeave: hide,
      onPointerDown: hide,
      onFocus: (event) => {
        if (event.currentTarget.matches(':focus-visible')) show()
      },
      onBlur: hide,
    },
    // The words are drawn rather than written: the name is the control's
    // already, and a second copy in the page's text would be found by a
    // search for it, copied with it, and counted by whatever reads the page.
    tip: (
      <span
        ref={tip}
        popover="manual"
        aria-hidden
        data-tip={text}
        style={ANCHORED ? { positionAnchor: anchor } : undefined}
        // As wide as its words: the area above a button is the button's
        // width, which would otherwise wrap a name a word to a line.
        className={cn(
          'popover-in m-0 w-max max-w-64 rounded-md border border-rule-bright bg-ink-top px-2 py-1',
          'text-left text-caption font-medium text-bone shadow-[var(--shadow-lift)] before:content-[attr(data-tip)]',
          ANCHORED ? 'inset-auto mb-1.5 [position-area:block-start] [position-try-fallbacks:flip-block]' : 'inset-auto',
        )}
      />
    ),
  }
}
