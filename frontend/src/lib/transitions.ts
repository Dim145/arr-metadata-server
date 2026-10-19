/**
 * From one page to the next, drawn rather than cut.
 *
 * The router here is not a data router, which is where React Router keeps its
 * own `viewTransition`; so the move is wrapped by hand. The page as it stands
 * is captured, the route changes, and the browser animates the one into the
 * other: the page itself slides a short way in the direction the reader went,
 * and an element named `poster` on both sides — the card that was opened, the
 * plate it opens onto — travels from the one box to the other.
 *
 * Going on, through `useTransitionNavigate`, the page leaves towards the
 * start of the line and the next comes in from its end. Going back with the
 * browser's button, it is the other way round, so the page that returns comes
 * from the side it went (`drawTraversals` below; the slides are in
 * styles.css, read from the transition's type: `forward` or `back`).
 *
 * A browser without the API, or a reader who has asked their system for less
 * motion, simply arrives, as before.
 */

import { useCallback, type MouseEvent } from 'react'
import { flushSync } from 'react-dom'
import { useNavigate, type NavigateOptions, type To } from 'react-router'

/** The name the travelling poster goes by, on both pages. */
export const POSTER = 'poster'

/** Which way the reader went: on into the catalogue, or back the way they came. */
type Way = 'forward' | 'back'

/**
 * How long the picture of the next page waits for React to have drawn it. A
 * page whose code is still on its way takes longer; past this the transition
 * goes ahead with what is there, and the page arrives when it does.
 */
const PATIENCE_MS = 300

/** Whether the browser draws transitions, and the reader wants them drawn. */
function drawn(): boolean {
  return (
    typeof document !== 'undefined' &&
    typeof document.startViewTransition === 'function' &&
    !window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

/** Whether the browser can tell the stylesheet which way a transition goes. */
function typed(): boolean {
  return typeof ViewTransition !== 'undefined' && 'types' in ViewTransition.prototype
}

/**
 * Settles once React has put the next page in: at the first change to what
 * it draws, or after `PATIENCE_MS` for a page slower than that.
 *
 * The router hands each move to React inside a transition of its own, which
 * React renders when it can rather than in the call that asked — `flushSync`
 * does not hurry it. The browser takes its picture of the new page once the
 * update has settled; settled at once, the picture was taken whenever the
 * next frame came, which was the old page again whenever React had not quite
 * finished, and the page then changed under a finished transition.
 */
function committed(): Promise<void> {
  return new Promise((resolve) => {
    const done = () => {
      observer.disconnect()
      window.clearTimeout(patience)
      resolve()
    }
    const observer = new MutationObserver(done)
    const patience = window.setTimeout(done, PATIENCE_MS)
    observer.observe(document.getElementById('root') ?? document.body, { childList: true, subtree: true })
  })
}

/** A move, drawn: `update` makes it, and the transition is told which way it went. */
function draw(update: () => void, way: Way): ViewTransition {
  const run = () => {
    const settled = committed()
    update()
    return settled
  }
  const transition = typed() ? document.startViewTransition({ update: run, types: [way] }) : document.startViewTransition(run)
  // A transition the browser declines — a page hidden meanwhile, two
  // elements of one name after all — is a move all the same.
  transition.ready.catch(() => {})
  return transition
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

      const transition = draw(() => {
        flushSync(() => navigate(to, rest))
        // Captured from the top, where the new page is read from.
        window.scrollTo(0, 0)
      }, 'forward')
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

/** A move through the history the Navigation API saw coming, for the `popstate` after it to draw. */
let coming: { way: Way; landed: () => void } | undefined

/** Set while the router hears again a `popstate` held back for a transition. */
let retelling = false

/**
 * Back and forward through the history, drawn too.
 *
 * The browser moves first and tells the router after, with `popstate`; the
 * router renders the page it lands on when React can. To draw the move, that
 * event is held back from the router until the browser has its picture of
 * the page being left, and told again inside the transition.
 *
 * Which way it went, the Navigation API says: the entry it lands on against
 * the one it leaves. (`navigation.activation` says the same of how the
 * document itself was reached — once, for an application that is one
 * document; a move inside it is the `navigate` event's to describe.) That
 * event also lets the move be taken in hand, which keeps the browser's own
 * scroll restoration until the page being restored is the one on screen:
 * applied to the page being left, as it was, it fell short whenever that page
 * was the shorter of the two. Focus is left where it is, as before. Without
 * the API, a move is taken for a step back, which nearly every one is.
 *
 * A move the browser animates itself — a swipe back on a phone — is left to
 * it, and so is one to another place on the same page.
 */
function drawTraversals() {
  if (typeof window === 'undefined') return
  const guided = typeof navigation !== 'undefined'

  if (guided) {
    navigation.addEventListener('navigate', (event) => {
      if (event.navigationType !== 'traverse' || !event.canIntercept || event.hashChange) return
      if (event.hasUAVisualTransition || !drawn()) return
      const left = navigation.currentEntry?.index ?? -1
      let landed = () => {}
      const restored = new Promise<void>((resolve) => {
        landed = resolve
      })
      coming = { way: event.destination.index < left ? 'back' : 'forward', landed }
      // Should the `popstate` never come, the move is not left hanging.
      const sure = window.setTimeout(landed, PATIENCE_MS * 4)
      event.intercept({
        focusReset: 'manual',
        scroll: 'after-transition',
        handler: () => restored.finally(() => window.clearTimeout(sure)),
      })
    })
  }

  window.addEventListener(
    'popstate',
    (event) => {
      if (retelling) return
      const held = coming
      coming = undefined
      // With the API, a move its event did not take in hand is not drawn.
      if ((guided && !held) || event.hasUAVisualTransition || !drawn()) {
        held?.landed()
        return
      }

      // Ahead of the router's own listener, which a capturing one on the
      // same target always is.
      event.stopImmediatePropagation()
      const transition = draw(() => {
        retelling = true
        try {
          window.dispatchEvent(new PopStateEvent('popstate', { state: event.state as unknown }))
        } finally {
          retelling = false
        }
      }, held?.way ?? 'back')
      void transition.updateCallbackDone.finally(() => held?.landed())
    },
    { capture: true },
  )
}

drawTraversals()
