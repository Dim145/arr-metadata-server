/**
 * The changes typed into the editor and not saved yet.
 *
 * A field is edited where it is shown: pressed, its value becomes a box in
 * its place, and what is typed is kept here as a draft — the field marked
 * with a brass dot, and its tab too — until it is saved. Saving sends every
 * draft at once, each its own lock and its own line in the journal, from the
 * bar along the foot of the page: *Save and stay*, or *Save and open the
 * next* work of the catalogue's list.
 *
 * Nothing typed is lost by leaving without a word: a link pressed, the
 * palette, a key that goes elsewhere, the back button where the browser
 * lets a page ask, all ask first (`useLeaveGuard`); closing the tab or
 * reloading it is asked about by the browser itself.
 */

import { createContext, use, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useLocation, useNavigate } from 'react-router'

import { Keys } from '../../../components/CommandPalette'
import { Button, Dialog, Glyph, MENU_ITEM, Menu, Spinner } from '../../../components/ui'
import { cn } from '../../../lib/cn'
import { useI18n } from '../../../lib/i18n'
import { guardLeaving } from '../../../lib/leave'
import type { FieldDef } from '../../../lib/types'

/** One field's change, as typed. */
export interface Draft {
  /** `scope|field|shelf`: see `draftKey`. */
  key: string
  /** `item`, `season:3` or `episode:3x7`. */
  scope: string
  field: string
  /** How what was typed is read back into the field's own shape. */
  def: FieldDef
  /** What was typed. */
  text: string
  /** The value as it was, as it reads in a box: a draft that comes back to it is no change at all. */
  before: string
  /** The language the text may be kept to alone (`fra`), or empty: every language. */
  shelf: string
  reach: 'alone' | 'every'
  /** Why the server refused it, the last time it was sent. */
  error?: string
}

/** A draft's name: the field it changes, and the language it is typed in where that matters. */
export function draftKey(scope: string, field: string, shelf: string): string {
  return `${scope}|${field}|${shelf}`
}

/** Which tab holds a draft: the work's own fields, or a season's and its episodes'. */
export function tabOfScope(scope: string): 'record' | 'seasons' {
  return scope.startsWith('season:') || scope.startsWith('episode:') ? 'seasons' : 'record'
}

export interface Drafts {
  get: (key: string) => Draft | undefined
  /** Keep a draft, or forget it when it has come back to what was there. */
  put: (draft: Draft) => void
  drop: (key: string) => void
  all: Draft[]
}

const DraftsContext = createContext<Drafts | null>(null)

/** The drafts of the work being edited; nothing outside the editor. */
export function useDrafts(): Drafts | null {
  return use(DraftsContext)
}

/** Whether a draft changes nothing: back to the value it started from, in the same reach. */
function unchanged(draft: Draft): boolean {
  return draft.text === draft.before && !draft.error && (!draft.shelf || draft.reach === 'alone')
}

/** The drafts' store, for the editor to hold and hand down. */
export function useDraftStore(): [Drafts, { clear: () => void; fail: (key: string, why: string) => void }] {
  const [drafts, setDrafts] = useState<Map<string, Draft>>(() => new Map())

  const put = useCallback((draft: Draft) => {
    setDrafts((was) => {
      const next = new Map(was)
      if (unchanged(draft)) next.delete(draft.key)
      else next.set(draft.key, draft)
      return next
    })
  }, [])
  const drop = useCallback((key: string) => {
    setDrafts((was) => {
      if (!was.has(key)) return was
      const next = new Map(was)
      next.delete(key)
      return next
    })
  }, [])
  const clear = useCallback(() => setDrafts(new Map()), [])
  const fail = useCallback((key: string, why: string) => {
    setDrafts((was) => {
      const draft = was.get(key)
      if (!draft) return was
      return new Map(was).set(key, { ...draft, error: why })
    })
  }, [])

  const store = useMemo<Drafts>(
    () => ({ get: (key) => drafts.get(key), put, drop, all: [...drafts.values()] }),
    [drafts, put, drop],
  )
  return [store, { clear, fail }]
}

export function DraftsProvider({ value, children }: { value: Drafts; children: ReactNode }) {
  return <DraftsContext value={value}>{children}</DraftsContext>
}

/* ── Leaving ──────────────────────────────────────────────────────────────── */

/** The Navigation API, where the engine has it: what lets the back button be asked about. */
interface Navigation extends EventTarget {
  currentEntry: { index: number } | null
}
interface NavigateEvent extends Event {
  navigationType: 'push' | 'replace' | 'reload' | 'traverse'
  cancelable: boolean
  destination: { index: number; sameDocument: boolean }
}

/**
 * Ask before the page is left while `count` changes are unsaved: a link
 * pressed anywhere — but one to this same work's editor, whose drafts stay —
 * the palette, a key, and the back and forward buttons where the browser
 * lets a page ask; the browser itself asks before the tab is closed or
 * reloaded. `save` is tried first by "Save, then leave", which leaves only
 * once every change is saved.
 */
export function useLeaveGuard(count: number, save: () => Promise<boolean>, discard: () => void) {
  const { t } = useI18n()
  const l = t.admin.editor.leave
  const navigate = useNavigate()
  const location = useLocation()
  const [pending, setPending] = useState<(() => void) | null>(null)
  const [saving, setSaving] = useState(false)
  const here = useRef(location.pathname)
  here.current = location.pathname

  const ask = useCallback((proceed: () => void) => setPending(() => proceed), [])

  // Closing the tab, reloading it, an address typed: the browser's own question.
  useEffect(() => {
    if (!count) return
    const hold = (event: BeforeUnloadEvent) => {
      event.preventDefault()
      // What older engines need to ask at all.
      event.returnValue = ''
    }
    window.addEventListener('beforeunload', hold)
    return () => window.removeEventListener('beforeunload', hold)
  }, [count])

  // A link pressed, before the router follows it.
  useEffect(() => {
    if (!count) return
    const onClick = (event: MouseEvent) => {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
      const anchor = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href]') : null
      if (!anchor || (anchor.target && anchor.target !== '_self') || anchor.hasAttribute('download')) return
      const url = new URL(anchor.href, window.location.href)
      // Another site: the browser asks, as it does for a closed tab.
      if (url.origin !== window.location.origin) return
      // This work's editor still: another tab of it, an anchor.
      if (url.pathname === here.current) return
      event.preventDefault()
      event.stopPropagation()
      ask(() => navigate(`${url.pathname}${url.search}${url.hash}`))
    }
    document.addEventListener('click', onClick, true)
    return () => document.removeEventListener('click', onClick, true)
  }, [count, ask, navigate])

  // The back and forward buttons, where the browser lets the page ask.
  useEffect(() => {
    const navigation = (window as Window & { navigation?: Navigation }).navigation
    if (!count || !navigation) return
    const onNavigate = (event: Event) => {
      const going = event as NavigateEvent
      if (going.navigationType !== 'traverse' || !going.cancelable || !going.destination.sameDocument) return
      const from = navigation.currentEntry?.index
      if (from === undefined) return
      going.preventDefault()
      const delta = going.destination.index - from
      ask(() => window.history.go(delta))
    }
    navigation.addEventListener('navigate', onNavigate)
    return () => navigation.removeEventListener('navigate', onNavigate)
  }, [count, ask])

  // The palette, a key, the next work: what moves on by itself.
  useEffect(() => {
    if (!count) return
    guardLeaving((proceed) => {
      ask(proceed)
      return true
    })
    return () => guardLeaving(null)
  }, [count, ask])

  const leave = () => {
    const proceed = pending
    setPending(null)
    discard()
    // Once the drafts are gone, so that nothing asks again on the way out.
    if (proceed) window.setTimeout(proceed, 0)
  }
  const saveThenLeave = async () => {
    const proceed = pending
    setSaving(true)
    const saved = await save()
    setSaving(false)
    setPending(null)
    if (saved && proceed) window.setTimeout(proceed, 0)
  }

  const dialog = (
    <Dialog
      open={pending !== null}
      title={l.title}
      onClose={() => setPending(null)}
      footer={
        <>
          <Button onClick={() => setPending(null)}>{l.stay}</Button>
          <Button variant="danger" disabled={saving} onClick={leave}>
            {l.leave}
          </Button>
          <Button variant="primary" disabled={saving} onClick={() => void saveThenLeave()}>
            {saving ? <Spinner className="size-4" /> : <Glyph name="check" className="size-4" />}
            {l.save}
          </Button>
        </>
      }
    >
      {l.body(count)}
    </Dialog>
  )

  return { ask, dialog }
}

/* ── The bar ──────────────────────────────────────────────────────────────── */

/**
 * What is not saved yet, along the foot of the page while there is any: how
 * many changes, the way to drop them — asked once more, in place — and the
 * two ways to save them, *Save and stay* first, *Save and open the next*
 * behind its arrow with the work it would open.
 */
export function SaveBar({
  count,
  saving,
  said,
  lockable,
  next,
  onSave,
  onDiscard,
}: {
  count: number
  /** How far a save has gone, while it goes. */
  saving: { at: number; of: number } | null
  /** What the last save came to, said until the next change. */
  said: { text: string; failed: boolean } | null
  /** Whether saving locks: not on a work no refresh touches. */
  lockable: boolean
  /** The work "Save and open the next" opens, when there is one. */
  next: { id: string; title: string } | null | undefined
  onSave: (then?: 'next') => void
  onDiscard: () => void
}) {
  const { t } = useI18n()
  const s = t.admin.editor.save
  const [discarding, setDiscarding] = useState(false)

  useEffect(() => {
    if (!count) setDiscarding(false)
  }, [count])

  if (!count && !saving && !said) return null

  return (
    <div
      role="region"
      aria-label={count ? s.unsaved(count) : s.stay}
      className={cn(
        'rise fixed inset-x-4 bottom-[calc(4.5rem+env(safe-area-inset-bottom))] z-40 mx-auto max-w-2xl lg:bottom-6',
        'flex flex-wrap items-center gap-x-3 gap-y-2 rounded-panel border bg-ink-top py-2.5 pr-2.5 pl-4 shadow-[var(--shadow-plate)]',
        count ? 'border-brass-deep' : 'border-rule-bright',
      )}
    >
      <p className="flex min-w-0 flex-1 items-center gap-2 text-sm text-bone">
        {saving ? (
          <>
            <Spinner className="size-4 shrink-0" />
            <span role="status">{s.saving(saving.at, saving.of)}</span>
          </>
        ) : count ? (
          <>
            <span aria-hidden className="size-2 shrink-0 rounded-full bg-brass shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-brass)_18%,transparent)]" />
            <span>
              {s.unsaved(count)}
              <span className="block text-xs text-bone-faint">{lockable ? s.locks : s.noLocks}</span>
            </span>
          </>
        ) : null}
        {said && !saving ? (
          <span role={said.failed ? 'alert' : 'status'} className={cn('text-sm', said.failed ? 'text-vermillion' : 'text-bone-dim', count && 'sr-only')}>
            {said.text}
          </span>
        ) : null}
      </p>

      {count && !saving ? (
        <div className="flex items-center gap-1.5">
          {discarding ? (
            <Button size="sm" variant="danger" onClick={onDiscard}>
              {s.discardConfirm(count)}
            </Button>
          ) : (
            <Button size="sm" variant="quiet" onClick={() => setDiscarding(true)}>
              {s.discard}
            </Button>
          )}
          <span className="inline-flex items-stretch">
            <Button size="sm" variant="primary" className="rounded-r-none pr-3" onClick={() => onSave()}>
              <Glyph name="check" className="size-4" />
              {s.stay}
            </Button>
            <Menu
              label={s.more}
              align="end"
              buttonClassName={(open) =>
                cn(
                  'inline-flex min-h-11 cursor-pointer items-center rounded-r-full border-l border-ink/30 px-2.5 text-ink',
                  'bg-[linear-gradient(135deg,color-mix(in_srgb,var(--color-vermillion)_85%,var(--color-brass)),color-mix(in_srgb,var(--color-vermillion)_70%,var(--color-brass)))]',
                  'transition-[filter] duration-200 hover:brightness-110',
                  'forced-colors:border forced-colors:border-[color:ButtonText]',
                  open && 'brightness-110',
                )
              }
              button={<Glyph name="chevronUp" className="size-4" />}
              className="w-80"
            >
              {(close) => (
                <>
                  <button
                    type="button"
                    className={MENU_ITEM}
                    onClick={() => {
                      close()
                      onSave()
                    }}
                  >
                    <Glyph name="check" className="size-4" />
                    <span className="flex-1">{s.stay}</span>
                    <Keys keys="mod+s" />
                  </button>
                  <button
                    type="button"
                    className={MENU_ITEM}
                    disabled={!next}
                    title={next ? undefined : s.noNext}
                    onClick={() => {
                      close()
                      onSave('next')
                    }}
                  >
                    <Glyph name="chevronRight" className="size-4" />
                    <span className="min-w-0 flex-1 truncate">{next ? s.nextTo(next.title) : s.next}</span>
                    <Keys keys="shift+mod+enter" />
                  </button>
                  {next ? null : <p className="px-3 pb-2 text-xs leading-relaxed text-bone-faint">{s.noNext}</p>}
                </>
              )}
            </Menu>
          </span>
        </div>
      ) : null}
    </div>
  )
}
