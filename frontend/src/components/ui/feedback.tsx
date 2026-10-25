/**
 * What a screen says while it has nothing to show, and when it asks.
 *
 * Waiting is said the same way on every screen:
 *
 * - The region whose content is on its way — the grid, the list, the panel,
 *   not the page around it — carries `aria-busy="true"` from the moment it is
 *   asked for until it is filled, and loses it then. A screen reader holds
 *   back what changes inside it meanwhile, rather than reading the stand-in
 *   and then the page over it.
 * - What stands in keeps the room the content will take: a `Skeleton` where
 *   its shape is known — a grid of posters, a plate — which shows only after
 *   a beat, so that a quick answer never flashes one; a `Spinner` inside the
 *   control whose action is under way, which says in its own words what it
 *   is doing and is `disabled` while it does. Both are drawings, hidden from
 *   assistive technology.
 * - What came of it is said once, by a live region — `role="status"`, or
 *   `role="alert"` for a failure — never by `aria-busy` going away.
 * - Content already shown and being fetched again is not busy: the reader is
 *   reading it, and it changes in place.
 */

import { useEffect, useId, useLayoutEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react'

import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'

import { Glyph, type GlyphName } from './glyphs'

/** How long a dialog takes to leave (styles.css, `.dialog-in`), and a little over. */
const LEAVING_MS = 200

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
 *
 * It rises in and falls away (`.dialog-in`). Leaving, it keeps saying what it
 * said while open: by then the caller has usually let go of what it showed —
 * the account deleted, the choice made — and a frame fading out empty reads
 * as a glitch. Its content is unmounted once it has gone, as before, so a
 * dialog opened again starts afresh.
 *
 * States:
 * - closed: nothing is drawn.
 * - open: a frame on the overlay surface, a rule round it and between its
 *   head, body and foot, the page dimmed under the ink at 80%. It rises in
 *   (180ms), or is simply there for a reader who asked for less motion.
 * - focus: inside it from the moment it opens — the browser's choice, or a
 *   field the caller gave `autoFocus` — and back on what opened it after.
 * - leaving: falling away (150ms), still saying what it said.
 * - loading, error: the caller's, in its body and its foot — a button that
 *   waits, an alert line. It has no disabled or empty state.
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
  // Named by its title, as a screen reader announces it on opening.
  const titleId = useId()
  const said = useRef({ title, children, footer })
  if (open) said.current = { title, children, footer }

  // Closed and still on screen: the way out, until the transition is over.
  // Set when it closes — from here, or by the browser on Escape — and never
  // during a render, so no stale "gone" can overtake an opening.
  const [leaving, setLeaving] = useState(false)
  // Drawn while open, and through the way out. The render that closes it
  // comes while the element is still open, which is read so, until
  // `leaving` takes over once it has closed.
  const drawn = open || leaving || Boolean(ref.current?.open)

  // Each opening draws its words anew, so a form cancelled and opened again
  // while the last is still leaving starts empty, as it always has; the way
  // out keeps the instance it leaves with. Counted after each commit, never
  // during a render, which React may run twice or not at all.
  const wasOpen = useRef(false)
  const openings = useRef(0)
  const instance = open && !wasOpen.current ? openings.current + 1 : openings.current
  useLayoutEffect(() => {
    if (open && !wasOpen.current) openings.current += 1
    wasOpen.current = open
  })

  useEffect(() => {
    const node = ref.current
    if (!node) return

    if (open) {
      if (!node.open) node.showModal()
      return
    }
    if (node.open) {
      node.close()
      setLeaving(true)
    }
  }, [open])

  useEffect(() => {
    if (!leaving) return
    const gone = window.setTimeout(() => setLeaving(false), LEAVING_MS)
    return () => window.clearTimeout(gone)
  }, [leaving])

  return (
    <dialog
      ref={ref}
      aria-labelledby={drawn ? titleId : undefined}
      onClose={() => {
        setLeaving(true)
        onClose()
      }}
      className={cn(
        'dialog-in m-auto max-h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] overflow-hidden',
        size === 'lg' ? 'max-w-3xl' : 'max-w-md',
        'rounded-panel border border-rule bg-(--surface-overlay) p-0 text-bone backdrop:bg-ink/80',
      )}
    >
      {drawn ? (
        <div key={instance} className="flex max-h-[calc(100dvh-2rem)] flex-col">
          <header className="shrink-0 border-b border-rule px-5 py-4">
            <h2 id={titleId} className="font-display text-lg font-medium text-bone">
              {said.current.title}
            </h2>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 py-4 text-sm leading-relaxed text-bone-dim">
            {said.current.children}
          </div>
          <footer className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-rule px-5 py-3">
            {said.current.footer}
          </footer>
        </div>
      ) : null}
    </dialog>
  )
}

/**
 * The shape of what is on its way (see the top of this file).
 *
 * It waits a beat before it shows (`.wait-in`): an answer that comes back at
 * once never flashes a grey grid, and one that takes its time is marked as
 * coming as soon as the wait is worth marking.
 *
 * States: it is itself a loading state — the high ink in the shape of what
 * is coming, a sweep of light across it (still, for a reader who asked for
 * less motion), and under forced colours an outline in the system's grey.
 */
export function Skeleton({ className }: { className?: string }) {
  // A raised surface and a sweep of light, neither of which survives forced
  // colours: there the shape the page is waiting to fill is an outline in the
  // system's grey, the colour it gives what is not ready to be used.
  return (
    <div
      aria-hidden
      className={cn(
        'shimmer wait-in rounded-card bg-ink-high forced-colors:border forced-colors:border-[color:GrayText]',
        className,
      )}
    />
  )
}

/** Told of the browser going offline and back. */
function subscribeOnline(change: () => void): () => void {
  window.addEventListener('online', change)
  window.addEventListener('offline', change)
  return () => {
    window.removeEventListener('online', change)
    window.removeEventListener('offline', change)
  }
}

/**
 * Whether the browser believes it has a network: `navigator.onLine`, kept
 * current. A `false` is to be trusted — no network at all — where a `true`
 * only means there is one, not that the server is on it.
 */
export function useOnline(): boolean {
  return useSyncExternalStore(subscribeOnline, () => navigator.onLine, () => true)
}

/** Why there is nothing to show: see `EmptyState`. */
export type EmptyKind = 'empty' | 'filtered' | 'error'

/** How much room saying so takes: see `EmptyState`. */
export type EmptySize = 'page' | 'line'

const EMPTY_GLYPH: Record<EmptyKind, GlyphName> = {
  empty: 'reel',
  filtered: 'sliders',
  error: 'alert',
}

/**
 * Nothing to show, and why — for the four reasons are four different things
 * to do about it.
 *
 * - `empty`: there is nothing yet, a catalogue or a list with nothing in it.
 * - `filtered`: there is, but not under the filters chosen, which the reader
 *   can undo — the `action` that clears them belongs here.
 * - `error`: the server was asked and did not answer, said as a failure
 *   (`role="alert"`). The `action` is the way to ask again.
 * - and offline: an error while the browser says it has no network is said
 *   as that instead, in the interface's own words rather than the caller's,
 *   since it is not the server's doing and the page comes back by itself
 *   once the connection does (React Query asks again on reconnecting).
 *
 * In two sizes, which are the two ways the interface had of saying it:
 *
 * - `page`, where the nothing is the screen's or a grid's — its glyph, its
 *   words in the display face, centred, with room around them. Its words
 *   wrap evenly: a title of two lines reads as one thought.
 * - `line`, where it is one list's among others, inside a panel — one quiet
 *   sentence at the panel's own padding, as the "nothing here" lines under a
 *   panel's head were written by hand. A failure keeps its glyph and the
 *   error's colour.
 *
 * States: the kinds above are its states — at rest, in faint bone; failed,
 * in the error's colour; offline, in faint bone with the struck cloud. The
 * `action` it holds has its own.
 */
export function EmptyState({
  title,
  hint,
  action,
  kind = 'empty',
  size = 'page',
  className,
}: {
  title: ReactNode
  hint?: ReactNode
  action?: ReactNode
  kind?: EmptyKind
  size?: EmptySize
  /** Its spacing, where the line sits in something without a panel's padding. */
  className?: string
}) {
  const { t } = useI18n()
  const online = useOnline()
  const offline = kind === 'error' && !online
  const failed = kind === 'error' && !offline

  if (size === 'line') {
    const line = (
      <p
        role={kind === 'error' ? 'alert' : undefined}
        className={cn(
          'flex flex-wrap items-baseline gap-x-2 gap-y-1 px-5 py-4 text-sm',
          failed ? 'text-status-error' : 'text-bone-faint',
          !action && className,
        )}
      >
        {kind === 'error' ? <Glyph name={offline ? 'offline' : 'alert'} className="size-4 shrink-0 self-center" /> : null}
        <span>{offline ? t.common.offline : title}</span>
        {offline || hint ? <span className="text-bone-faint">{offline ? t.common.offlineHint : hint}</span> : null}
      </p>
    )
    return action ? (
      <div className={cn('flex flex-wrap items-center justify-between gap-x-4 pr-5', className)}>
        {line}
        {action}
      </div>
    ) : (
      line
    )
  }

  return (
    <div
      role={kind === 'error' ? 'alert' : undefined}
      className={cn('flex flex-col items-center gap-3 px-6 py-16 text-center', className)}
    >
      <Glyph name={offline ? 'offline' : EMPTY_GLYPH[kind]} className={cn('size-8', failed ? 'text-status-error' : 'text-bone-faint')} />
      <p className="font-display text-lg text-bone [text-wrap-style:balance]">{offline ? t.common.offline : title}</p>
      {offline || hint ? (
        <p className="max-w-sm text-sm text-bone-dim [text-wrap-style:balance]">{offline ? t.common.offlineHint : hint}</p>
      ) : null}
      {action}
    </div>
  )
}
