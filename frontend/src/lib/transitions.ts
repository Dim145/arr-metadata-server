/**
 * From one page to the next, drawn rather than cut.
 *
 * The router here is not a data router, which is where React Router keeps its
 * own `viewTransition`; so the move is wrapped by hand. The page as it stands
 * is captured, the route changes in one synchronous render, and the browser
 * animates the one into the other: a cross-fade for the page, and for an
 * element named `poster` on both sides — the card that was opened, the plate
 * it opens onto — a travel from the one box to the other.
 *
 * A browser without the API, or a reader who has asked their system for less
 * motion, simply arrives, as before.
 */

import { useCallback, type MouseEvent } from 'react'
import { flushSync } from 'react-dom'
import { useNavigate, type NavigateOptions, type To } from 'react-router'

/** The name the travelling poster goes by, on both pages. */
export const POSTER = 'poster'

/** Whether the browser draws transitions, and the reader wants them drawn. */
function drawn(): boolean {
  return (
    typeof document !== 'undefined' &&
    typeof document.startViewTransition === 'function' &&
    !window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

/**
 * `navigate`, with the move drawn where it can be.
 *
 * `from` is the element that travels — a card's poster — named for the length
 * of the transition, so that no two cards ever claim the name at once; a
 * plate on the page before, which wears the name always, gives it up for the
 * same length.
 */
export function useTransitionNavigate() {
  const navigate = useNavigate()

  return useCallback(
    (to: To, options: NavigateOptions & { from?: HTMLElement | null } = {}) => {
      const { from, ...rest } = options
      if (!drawn()) {
        navigate(to, rest)
        return
      }

      const others = from
        ? [...document.querySelectorAll<HTMLElement>(`[data-travels="${POSTER}"]`)].filter((element) => element !== from)
        : []
      for (const other of others) other.style.viewTransitionName = 'none'
      from?.style.setProperty('view-transition-name', POSTER)

      const transition = document.startViewTransition(() => {
        flushSync(() => navigate(to, rest))
        // Captured from the top, where the new page is read from.
        window.scrollTo(0, 0)
      })
      // A transition the browser declines — a page hidden meanwhile, two
      // elements of one name after all — is a navigation all the same.
      transition.ready.catch(() => {})
      void transition.finished.finally(() => {
        from?.style.removeProperty('view-transition-name')
        for (const other of others) other.style.removeProperty('view-transition-name')
      })
    },
    [navigate],
  )
}

/**
 * Whether a click on a link is the plain kind — no modifier asking for a new
 * tab, no other target — that the page may handle itself.
 */
export function plainClick(event: MouseEvent<HTMLAnchorElement>): boolean {
  return (
    event.button === 0 &&
    !event.metaKey &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.shiftKey &&
    (!event.currentTarget.target || event.currentTarget.target === '_self') &&
    !event.defaultPrevented
  )
}
