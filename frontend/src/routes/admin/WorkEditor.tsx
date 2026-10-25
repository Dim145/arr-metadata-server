/**
 * One entry, and everything a person may claim of it.
 *
 * The record is a card in five sections, each a tab: the fields and the
 * sources behind them, the seasons and their episodes, the artwork, the
 * people and the other titles, and the work as the rest of the world files
 * it — and a sixth, its journal of what changed. The address carries which
 * is open, so a season's own page, the back button and a bookmark all land
 * where they meant to. A refresh shows what it would change before it is
 * run.
 *
 * A field is edited where it is shown (`editor/FieldRow.tsx`), and what is
 * typed is kept as a draft until it is saved: the field and its tab marked
 * in brass, a bar along the foot of the page with *Save and stay* and *Save
 * and open the next* — the next work of the catalogue's list, which arrows
 * beside the way back walk too — and a question before anything leaves the
 * page with a change unsaved (`editor/drafts.tsx`). The public page can be
 * kept in view beside the editor, reloaded at each save: a panel on a wide
 * screen, the whole screen on a narrow one.
 *
 * The lock is the product, so it is the loudest thing on every row: a brass
 * edge, a padlock beside the field's name and the word itself in a chip.
 * Saving is what locks — that is the server's model, and the interface
 * refuses to invent a friendlier one, because an operator who thinks a field
 * is locked when it is not will lose their edit to the next sweep.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { Link, useLocation, useNavigate, useParams, useSearchParams } from 'react-router'

import { useAdminTitle } from '../../components/AdminShell'
import { useCommands, type Command } from '../../components/CommandPalette'
import { Artwork } from '../../components/media'
import { Button, ButtonLink, Chip, Dialog, Glyph, IconButton, Provenance, Skeleton, Spinner } from '../../components/ui'
import { api, query, segment } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { statusLabel } from '../../lib/labels'
import { leaving } from '../../lib/leave'
import { backdrop, hasSources, poster } from '../../lib/media'
import { usePreference } from '../../lib/preferences'
import type {
  CuratedListPage,
  CuratedLists,
  FieldRegistry,
  MediaItem,
  MergeList,
  Override,
  ProvenanceReport,
  Uploaded,
} from '../../lib/types'

import { ArtworkTab } from './editor/ArtworkTab'
import { DraftsProvider, SaveBar, tabOfScope, useDraftStore, useLeaveGuard } from './editor/drafts'
import { ElsewhereTab } from './editor/ElsewhereTab'
import { parse } from './editor/FieldRow'
import { ImagePicker, type Candidate } from './editor/ImagePicker'
import { JournalTab } from './editor/JournalTab'
import { EditorLocksContext, lockOf, type EditorLocks } from './editor/locks'
import { PeopleTab } from './editor/PeopleTab'
import { RecordTab } from './editor/RecordTab'
import { RefreshReview } from './editor/RefreshReview'
import { SeasonsTab } from './editor/SeasonsTab'
import { Count, Facts, LockBadge, TABS, isTab, tabOfAnchor, type Tab } from './editor/shared'
import { around, nextOf, readRun, type Neighbour } from './editor/walk'

/** The tab an address asks for: its anchor first, then `?tab=`, then a season named. */
function askedTab(params: URLSearchParams, hash: string): Tab {
  const fromAnchor = tabOfAnchor(hash)
  if (fromAnchor) return fromAnchor
  const named = params.get('tab')
  if (isTab(named)) return named
  return params.has('season') ? 'seasons' : 'record'
}

/** From what width the preview stands beside the editor rather than over it: `xl`. */
const WIDE = '(width >= 80rem)'

/** Whether the screen is as wide as `WIDE`, and told again when it changes. */
function useWide(): boolean {
  return useSyncExternalStore(
    (changed) => {
      const media = window.matchMedia(WIDE)
      media.addEventListener('change', changed)
      return () => media.removeEventListener('change', changed)
    },
    () => window.matchMedia(WIDE).matches,
    () => false,
  )
}

export function WorkEditor() {
  const { id = '' } = useParams()
  // A work of its own each time: what was typed into one is never carried
  // over to the next, which the same page shows under another address.
  return <Editor key={id} id={id} />
}

function Editor({ id }: { id: string }) {
  const { t, locale } = useI18n()
  const queryClient = useQueryClient()
  const [params, setParams] = useSearchParams()
  const location = useLocation()

  const [language, setLanguage] = useState('')
  const [tab, setTab] = useState<Tab>(() => askedTab(params, location.hash))
  // An anchor to bring into view once the tab that holds it is on the page.
  const anchor = useRef<string | null>(location.hash ? location.hash.slice(1) : null)

  const item = useQuery({
    queryKey: ['item', id, language],
    queryFn: () => api.get<MediaItem>(`/items/${segment(id)}${query({ language })}`),
  })
  const registry = useQuery({
    queryKey: ['fields'],
    queryFn: () => api.get<FieldRegistry>('/fields'),
    staleTime: Infinity,
  })
  // Who gave what, and whom the work can be synced from.
  const report = useQuery({
    queryKey: ['item', id, 'provenance'],
    queryFn: () => api.get<ProvenanceReport>(`/items/${segment(id)}/provenance`),
  })
  const overrides = useQuery({
    queryKey: ['overrides', id],
    queryFn: () => api.get<Override[]>(`/items/${segment(id)}/overrides`),
  })

  useAdminTitle(item.data?.title ?? null)

  // A work merged into another is opened as that one, under its own address:
  // a lock set here would otherwise be set on the record that is gone. The
  // work's own slug is an address of its own, and stays as it was typed.
  const navigate = useNavigate()
  const canonical = item.data?.id
  const slug = item.data?.slug
  useEffect(() => {
    if (canonical && id !== canonical && id !== slug) {
      navigate(`/admin/catalogue/${canonical}${location.search}${location.hash}`, { replace: true })
    }
  }, [canonical, slug, id, location.search, location.hash, navigate])

  // The back button and a `?tab=` typed in: the address leads.
  useEffect(() => {
    const named = params.get('tab')
    if (isTab(named)) setTab(named)
  }, [params])

  // An anchor arriving later — the record's own "view" links, a public page
  // — opens the tab that holds it. After the one above, so the anchor wins
  // on first load when both are in the address.
  useEffect(() => {
    const target = tabOfAnchor(location.hash)
    if (!target) return
    setTab(target)
    anchor.current = location.hash.slice(1)
  }, [location.hash])

  useEffect(() => {
    if (!anchor.current || !item.data) return
    const target = anchor.current
    const frame = requestAnimationFrame(() => {
      document.getElementById(target)?.scrollIntoView({ block: 'start' })
      anchor.current = null
    })
    return () => cancelAnimationFrame(frame)
  }, [tab, item.data])

  // ── The preview ──────────────────────────────────────────────────────────
  const wide = useWide()
  // Beside the editor on a wide screen, kept with the person's account; over
  // it on a narrow one, opened each time it is wanted.
  const [panel, setPanel] = usePreference<boolean>('editor.preview', false, (raw) => (typeof raw === 'boolean' ? raw : undefined))
  const [sheet, setSheet] = useState(false)
  const previewShown = wide ? panel : sheet
  const togglePreview = () => (wide ? setPanel(panel ? undefined : true) : setSheet((open) => !open))
  const frame = useRef<HTMLIFrameElement>(null)
  const reloadPreview = () => {
    const shown = frame.current
    try {
      // Where the reader was in it, put back once it has drawn itself again.
      if (shown) shown.dataset.scroll = String(shown.contentWindow?.scrollY ?? 0)
      shown?.contentWindow?.location.reload()
    } catch {
      // Not this origin's to reach: it shows what it shows.
    }
  }

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', id] })
    // The catalogue's own pages read the same work under another key.
    void queryClient.invalidateQueries({ queryKey: ['work', id] })
    void queryClient.invalidateQueries({ queryKey: ['overrides', id] })
    void queryClient.invalidateQueries({ queryKey: ['items'] })
    void queryClient.invalidateQueries({ queryKey: ['stats'] })
    void queryClient.invalidateQueries({ queryKey: ['gaps'] })
  }
  // Anything saved from a tab shows at once beside it too.
  const changed = () => {
    invalidate()
    reloadPreview()
  }

  const refresh = useMutation({
    mutationFn: () => api.post<MediaItem>(`/items/${segment(id)}/refresh`),
    onSuccess: changed,
  })
  // What a refresh would change is shown first, and it is run from there.
  const [reviewing, setReviewing] = useState(false)

  // The poster and the background the work leads with, chosen from the
  // masthead: the same lock the artwork tab's star sets, reached from where
  // the pictures are seen.
  const [choosing, setChoosing] = useState<'poster' | 'fanart' | null>(null)
  const lead = (kind: 'poster' | 'fanart', address: string) =>
    api
      .put(`/items/${segment(id)}/overrides`, { scope: 'item', field: kind === 'poster' ? 'primaryPoster' : 'primaryFanart', value: address })
      .then(changed)

  // ── The drafts ───────────────────────────────────────────────────────────
  const [drafts, { clear, fail }] = useDraftStore()
  const count = drafts.all.length
  const [saving, setSaving] = useState<{ at: number; of: number } | null>(null)
  // What the last save, or the last thing done from the palette, came to.
  const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null)
  useEffect(() => {
    if (count) setSaid((was) => (was && !was.failed ? null : was))
  }, [count])
  useEffect(() => {
    if (!said || said.failed || count) return
    const timer = window.setTimeout(() => setSaid(null), 6_000)
    return () => window.clearTimeout(timer)
  }, [said, count])

  // The catalogue's list as it was last shown: the works on each side.
  const [run] = useState(readRun)
  const place = around(run, canonical ?? id, slug)
  const [further, setFurther] = useState<Neighbour | null>(null)
  const next: Neighbour | undefined = place?.next ?? further ?? undefined
  // At the end of the page the catalogue showed, the next is asked of the server.
  useEffect(() => {
    if (!place || place.next || place.at >= place.total) return
    let gone = false
    nextOf(canonical ?? id, slug)
      .then((found) => {
        if (!gone && found) setFurther(found)
      })
      .catch(() => {
        // Nothing further: the way on is not offered.
      })
    return () => {
      gone = true
    }
  }, [place?.next, place?.at, place?.total, canonical, id, slug]) // eslint-disable-line react-hooks/exhaustive-deps

  /**
   * Every draft sent, one after the other, each its own lock and its own
   * line in the journal: whether all of them were saved. One refused stays
   * a draft, its field saying why; the rest are saved all the same.
   */
  const save = useCallback(
    async (then?: 'next'): Promise<boolean> => {
      const work = item.data
      const list = drafts.all
      if (!work || saving) return false
      let failed = 0
      if (list.length) {
        setSaid(null)
        setSaving({ at: 0, of: list.length })
        for (const [index, draft] of list.entries()) {
          try {
            await api.put(`/items/${segment(work.id)}/overrides`, {
              scope: draft.scope,
              field: draft.field,
              value: parse(draft.text, draft.def),
              language: draft.shelf && draft.reach === 'alone' ? draft.shelf : undefined,
            })
            drafts.drop(draft.key)
          } catch (error) {
            failed += 1
            fail(draft.key, error instanceof Error ? error.message : t.common.actionFailed)
          }
          setSaving({ at: index + 1, of: list.length })
        }
        setSaving(null)
        changed()
        const s = t.admin.editor.save
        setSaid(failed ? { text: s.failed(failed), failed: true } : { text: s.saved(list.length), failed: false })
      }
      if (failed) return false
      if (then === 'next') {
        const target = next ?? (await nextOf(work.id, work.slug).catch(() => undefined))
        if (target) navigate(`/admin/catalogue/${target.id}`)
      }
      return true
    },
    // `changed` reads the same query client and frame whatever the drawing.
    [item.data, drafts, saving, fail, t, next, navigate], // eslint-disable-line react-hooks/exhaustive-deps
  )
  const guard = useLeaveGuard(count, () => save(), clear)

  // ── Whether the clients are served it, from the palette ──────────────────
  const [disabling, setDisabling] = useState(false)
  const setEnabled = useMutation({
    mutationFn: (enabled: boolean) => api.patch(`/items/${segment(id)}`, { isEnabled: enabled }),
    onSuccess: () => {
      setDisabling(false)
      changed()
    },
  })

  // The registry too, and not only the work: it is asked for once and cached
  // forever, so a failure there never retries. Without this the page waits on
  // `registry.data` that is never coming and shimmers for as long as it is open.
  const failedToLoad = item.isError || registry.isError
  const work = item.data
  const e = t.admin.editor
  const c = t.palette.command

  // A work with no source elsewhere is never refreshed, so nothing keeps a
  // field from anything: its edits are its record, and none is shown as a
  // lock. The server still files them as overrides, which is what makes
  // them stick the day a source is given.
  const synced = work ? hasSources(work) : false
  const locks = synced ? (overrides.data ?? []) : []
  // Every row reads its lock in the language shown, whichever tab drew it.
  const lockContext: EditorLocks | null =
    work && registry.data
      ? {
          language,
          overrides: locks,
          lockedFields: work.lockedFields ?? [],
          translated: registry.data.translated ?? [],
        }
      : null
  const overviewLock = lockOf(
    lockContext,
    'item',
    'overview',
    locks.find((one) => one.scope === 'item' && one.field === 'overview' && !one.language),
  )

  // What this page offers the palette: the work, as it is now.
  const commands: Command[] = work
    ? [
        ...(count
          ? [
              { id: 'save', label: c.save, glyph: 'check' as const, keys: 'mod+s', run: () => void save() },
              {
                id: 'save-next',
                label: c.saveNext,
                glyph: 'chevronRight' as const,
                keys: 'shift+mod+enter',
                note: next?.title,
                disabled: !next,
                hint: e.save.noNext,
                run: () => void save('next'),
              },
              { id: 'discard', label: c.discard, glyph: 'close' as const, run: clear },
            ]
          : []),
        ...(work.isManual
          ? []
          : [{ id: 'refresh', label: c.refresh, glyph: 'refresh' as const, keys: 'r', run: () => setReviewing(true) }]),
        ...(synced
          ? [
              overviewLock
                ? {
                    id: 'unlock-overview',
                    label: c.unlockOverview,
                    glyph: 'unlock' as const,
                    run: () =>
                      api
                        .delete(`/items/${segment(work.id)}/overrides/item/overview${query({ language: overviewLock.language })}`)
                        .then(changed),
                  }
                : {
                    id: 'lock-overview',
                    label: c.lockOverview,
                    glyph: 'lock' as const,
                    // As an edit of it is: in the language shown, else in every one.
                    run: () =>
                      api
                        .put(`/items/${segment(work.id)}/overrides`, {
                          scope: 'item',
                          field: 'overview',
                          value: work.overview ?? null,
                          language: language || undefined,
                        })
                        .then(changed),
                  },
            ]
          : []),
        work.isEnabled
          ? { id: 'disable', label: c.disable, glyph: 'power' as const, run: () => setDisabling(true) }
          : { id: 'enable', label: c.enable, glyph: 'power' as const, run: () => setEnabled.mutateAsync(true) },
        {
          id: 'add-to-list',
          label: c.addToList,
          glyph: 'list' as const,
          choose: {
            title: t.palette.listsTitle,
            load: async () => {
              const all = await queryClient.fetchQuery({
                queryKey: ['admin-lists'],
                queryFn: () => api.get<CuratedLists>('/lists'),
                staleTime: 60_000,
              })
              const manual = all.lists.filter((list) => list.mode === 'manual')
              if (!manual.length) return [{ id: 'none', label: t.palette.noLists, disabled: true }]
              return manual.map((list) => ({
                id: list.id,
                label: list.name,
                run: async () => {
                  const page = await api.get<CuratedListPage>(`/lists/${list.id}`)
                  const members = page.items.map((one) => one.id)
                  if (!members.includes(work.id)) await api.put(`/lists/${list.id}/items`, { items: [...members, work.id] })
                  void queryClient.invalidateQueries({ queryKey: ['admin-lists'] })
                  void queryClient.invalidateQueries({ queryKey: ['lists'] })
                  setSaid({ text: t.admin.works.bulk.added(1, list.name), failed: false })
                },
              }))
            },
          },
        },
        {
          id: 'preview',
          label: previewShown ? c.hidePreview : c.preview,
          glyph: 'reel' as const,
          keys: 'p',
          run: togglePreview,
        },
        {
          id: 'public',
          label: c.publicPage,
          glyph: 'external' as const,
          run: () => leaving(() => navigate(`/work/${work.id}`)),
        },
        ...(place?.previous
          ? [
              {
                id: 'previous',
                label: c.previous,
                note: place.previous.title,
                glyph: 'chevronLeft' as const,
                keys: 'k',
                run: () => leaving(() => navigate(`/admin/catalogue/${place.previous!.id}`)),
              },
            ]
          : []),
        ...(next
          ? [
              {
                id: 'next',
                label: c.next,
                note: next.title,
                glyph: 'chevronRight' as const,
                keys: 'j',
                run: () => leaving(() => navigate(`/admin/catalogue/${next.id}`)),
              },
            ]
          : []),
      ]
    : []
  useCommands(work ? { id: 'work', heading: t.palette.heading.work, commands } : null)

  if (failedToLoad) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-status-error">
        <Glyph name="alert" className="size-4" />
        {t.admin.editor.loadFailed}
      </p>
    )
  }

  // A row needs both the value and its definition, so wait for the pair.
  if (!work || !registry.data || !lockContext) {
    return <EditorSkeleton />
  }

  const ownLocks = locks.filter((o) => o.scope === 'item').length
  const deeperLocks = locks.filter((o) => o.scope.startsWith('season:') || o.scope.startsWith('episode:')).length
  // What the sources gave and a person hid or corrected, counted on the tab
  // that holds it.
  const peopleLocks = locks.filter((o) => o.scope.startsWith('credit:') || o.scope.startsWith('alternativeTitle:')).length
  const artworkLocks = locks.filter((o) => o.scope.startsWith('image:')).length
  const tabs = work.kind === 'series' ? TABS : TABS.filter((one) => one !== 'seasons')
  const shown: Tab = tabs.includes(tab) ? tab : 'record'
  const sheetArt = poster(work)
  const art = backdrop(work)
  const images = (work.images?.length ?? 0) + (work.seasons ?? []).reduce((sum, season) => sum + (season.images?.length ?? 0), 0)
  const identifiers = Object.values(work.externalIds).filter((value) => value !== undefined && value !== null && !(Array.isArray(value) && !value.length)).length
  // The tabs holding a change not saved yet.
  const unsavedIn = new Set(drafts.all.map((draft) => tabOfScope(draft.scope)))

  const choose = (next: Tab) => {
    setTab(next)
    anchor.current = null
    setParams(
      (prev) => {
        prev.set('tab', next)
        return prev
      },
      { replace: true },
    )
  }

  const onTabKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const at = tabs.indexOf(shown)
    const next =
      event.key === 'ArrowRight'
        ? (at + 1) % tabs.length
        : event.key === 'ArrowLeft'
          ? (at - 1 + tabs.length) % tabs.length
          : event.key === 'Home'
            ? 0
            : event.key === 'End'
              ? tabs.length - 1
              : undefined
    if (next === undefined) return
    event.preventDefault()
    choose(tabs[next]!)
    document.getElementById(`tab-${tabs[next]}`)?.focus()
  }

  // A figure beside each tab's name where there is one to give: "0 · 0" on
  // a work entered by hand said nothing a blank does not.
  const seasonsCount = (work.seasons?.length ?? 0) + (work.episodes?.length ?? 0)
  const peopleCount = (work.credits?.length ?? 0) + (work.alternativeTitles?.length ?? 0)
  const counts: Record<Tab, React.ReactNode> = {
    record: <LockBadge n={ownLocks} />,
    seasons: (
      <>
        {seasonsCount ? (
          <Count>
            {work.seasons?.length ?? 0} · {work.episodes?.length ?? 0}
          </Count>
        ) : null}
        <LockBadge n={deeperLocks} />
      </>
    ),
    artwork: (
      <>
        {images ? <Count>{images}</Count> : null}
        <LockBadge n={artworkLocks} />
      </>
    ),
    people: (
      <>
        {peopleCount ? (
          <Count>
            {work.credits?.length ?? 0} · {work.alternativeTitles?.length ?? 0}
          </Count>
        ) : null}
        <LockBadge n={peopleLocks} />
      </>
    ),
    elsewhere: identifiers ? <Count>{identifiers}</Count> : null,
    journal: null,
  }
  const besidePreview = wide && panel

  return (
    <div className={cn('mx-auto', besidePreview ? 'max-w-[110rem] xl:grid xl:grid-cols-[minmax(0,1fr)_minmax(24rem,34rem)] xl:gap-8' : 'max-w-7xl', count && 'pb-24')}>
      <div className="min-w-0">
        <div className="hidden min-h-11 items-center gap-3 lg:flex">
          <Link
            to="/admin/catalogue"
            className="label inline-flex min-h-11 items-center gap-2 transition-colors duration-150 hover:text-vermillion"
          >
            <Glyph name="arrowLeft" className="size-3.5" />
            {t.admin.catalogue}
          </Link>
          {place ? <Walk at={place.at} total={place.total} previous={place.previous} next={next} /> : null}
        </div>

        {/* ── The masthead ───────────────────────────────────────────── */}
        <header className="rise relative mb-2">
          {/* The work's own backdrop, quietly, behind the card: the room is for
              working in, so the wash is a sixth of the public page's and fades
              out before the tabs. */}
          {art ? (
            <div
              aria-hidden
              className="pointer-events-none absolute -inset-x-4 -top-6 bottom-0 overflow-hidden [mask-image:linear-gradient(to_bottom,black,rgba(0,0,0,0.55)_55%,transparent)] sm:-inset-x-6 lg:-inset-x-10 lg:-top-10"
            >
              <Artwork url={art} role="backdrop" alt="" className="size-full object-cover object-[center_20%] opacity-[0.16] saturate-[0.75]" />
            </div>
          ) : null}

          <div
            className={cn(
              'relative grid grid-cols-1 gap-5 pt-4 pb-6 sm:grid-cols-[7.5rem_minmax(0,1fr)] sm:items-end',
              besidePreview ? '' : 'xl:grid-cols-[7.5rem_minmax(0,1fr)_auto]',
            )}
          >
            <div className="hidden aspect-2/3 w-30 overflow-hidden rounded-card border border-rule-bright bg-ink-high shadow-[var(--shadow-plate)] sm:block">
              {sheetArt ? (
                <Artwork url={sheetArt} role="poster" eager alt={t.a11y.poster(work.title)} className="size-full object-cover" />
              ) : (
                <div className="grid size-full place-items-center">
                  <Glyph name={work.kind === 'series' ? 'tv' : 'film'} className="size-7 text-bone-faint" />
                </div>
              )}
            </div>

            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-1.5">
                <Chip tone="accent">
                  <Glyph name={work.kind === 'series' ? 'tv' : 'film'} className="size-3" />
                  {work.kind === 'series' ? t.nav.series : t.nav.films}
                </Chip>
                {work.status ? <Chip tone="provider">{statusLabel(work.status, t)}</Chip> : null}
                {work.isManual ? <Provenance manual label={t.work.manualEntry} /> : null}
                {work.isEnabled ? null : (
                  <Chip tone="accent">
                    <Glyph name="power" className="size-3" />
                    {t.admin.works.disabled}
                  </Chip>
                )}
                {locks.length > 0 ? (
                  <Chip tone="manual">
                    <Glyph name="lock" className="size-3" />
                    {e.lockCount(locks.length)}
                  </Chip>
                ) : null}
              </div>

              {/* Title and year at one size, told apart by weight; a space
                  between them in the text too, or a screen reader runs them
                  together as one word. */}
              <h1 className="mt-2.5 font-display text-3xl leading-[1.08] font-medium text-bone sm:text-4xl">
                {work.title}
                {work.year ? (
                  <>
                    {' '}
                    <span className="ml-1 font-normal text-bone-dim opacity-80">{work.year}</span>
                  </>
                ) : null}
                {work.titleQualifier ? (
                  <>
                    {' '}
                    <span className="ml-1 font-normal text-bone-faint opacity-80">({work.titleQualifier})</span>
                  </>
                ) : null}
              </h1>
              {work.originalTitle && work.originalTitle !== work.title ? (
                <p className="mt-1 text-sm text-bone-faint italic">{work.originalTitle}</p>
              ) : null}

              <Facts
                className="mt-3"
                items={[
                  work.network ?? work.studio ? <b className="font-medium text-bone-dim">{work.network ?? work.studio}</b> : undefined,
                  fmt.runtime(work.runtime, locale),
                  work.kind === 'series' && (work.seasons?.length || work.episodes?.length)
                    ? e.contentValue(work.seasons?.length ?? 0, work.episodes?.length ?? 0)
                    : undefined,
                  work.contentRating ? `${work.contentRating}${work.contentRatingCountry ? ` · ${work.contentRatingCountry}` : ''}` : undefined,
                  work.externalIds.tmdb ? `TMDB ${work.externalIds.tmdb}` : undefined,
                  work.externalIds.tvdb ? `TheTVDB ${work.externalIds.tvdb}` : undefined,
                ]}
              />

              <p className="mt-2.5 flex items-start gap-2 text-body-sm text-bone-dim">
                {work.isManual ? (
                  <>
                    <Glyph name="lock" className="mt-0.5 size-3.5 shrink-0 text-brass" />
                    {e.mast.manual}
                  </>
                ) : work.refreshError ? (
                  <>
                    <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0 text-vermillion" />
                    <span>
                      {e.refreshFailed}
                      <span className="mt-0.5 block font-mono text-xs break-words text-vermillion">{work.refreshError}</span>
                    </span>
                  </>
                ) : (
                  <>
                    <span aria-hidden className="mt-1.5 size-2 shrink-0 rounded-full bg-status-ok shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-status-ok)_18%,transparent)]" />
                    <span>
                      {work.refreshedAt ? e.mast.refreshedWhen(fmt.relative(work.refreshedAt, locale) ?? '') : e.mast.never}
                      {work.refreshAfter ? ` · ${e.mast.nextWhen(fmt.relative(work.refreshAfter, locale) ?? '')}` : ''}
                    </span>
                  </>
                )}
              </p>
              {refresh.isError ? (
                <p role="alert" className="mt-2 flex items-center gap-2 text-sm text-status-error">
                  <Glyph name="alert" className="size-4 shrink-0" />
                  {refresh.error.message}
                </p>
              ) : null}

              {/* The pictures the work leads with, chosen from where they are
                  seen — quietly, under the facts, not among the actions. */}
              <div className="-ml-2 mt-2 flex flex-wrap items-center gap-1">
                <Button size="sm" variant="quiet" onClick={() => setChoosing('poster')}>
                  <Glyph name="image" className="size-3.5" />
                  {e.picker.changePoster}
                </Button>
                <Button size="sm" variant="quiet" onClick={() => setChoosing('fanart')}>
                  <Glyph name="image" className="size-3.5" />
                  {e.picker.changeBackdrop}
                </Button>
              </div>
            </div>

            {/* Two to a row on a phone, a row of their own at the widest. */}
            <div
              className={cn(
                'grid grid-cols-2 gap-2 sm:col-span-2 sm:flex sm:flex-wrap',
                besidePreview ? '' : 'xl:col-span-1 xl:justify-end xl:self-end xl:pb-1',
              )}
            >
              {work.isManual ? null : (
                <Button onClick={() => setReviewing(true)} disabled={refresh.isPending}>
                  {refresh.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
                  {/* The whole phrase where there is room; the verb alone on a
                      phone, where three lines of button were half the masthead. */}
                  <span className="sm:hidden">{refresh.isPending ? e.refreshing : t.common.refresh}</span>
                  <span className="hidden sm:inline">{refresh.isPending ? e.refreshing : e.refresh}</span>
                </Button>
              )}
              <Button aria-pressed={previewShown} onClick={togglePreview}>
                <Glyph name="reel" className="size-4" />
                {e.preview.show}
              </Button>
              <Link
                to={`/work/${id}`}
                className="inline-flex min-h-11 items-center justify-center gap-2 rounded-full border border-rule-bright px-5 text-sm font-medium text-bone transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high"
              >
                <Glyph name="external" className="size-4" />
                {e.publicPage}
              </Link>
              <ButtonLink href={`/api/v1/items/${encodeURIComponent(id)}/nfo`} target="_blank" rel="noreferrer" title={e.nfoHint}>
                <Glyph name="download" className="size-4" />
                <span className="sm:hidden">.nfo</span>
                <span className="hidden sm:inline">{e.nfo}</span>
              </ButtonLink>
            </div>
          </div>
        </header>

        <MergedBanner id={work.id} onChanged={changed} />

        {reviewing ? (
          <RefreshReview work={work} registry={registry.data} onClose={() => setReviewing(false)} onRefresh={() => refresh.mutate()} />
        ) : null}

        {choosing ? (
          <ImagePicker
            open
            onClose={() => setChoosing(null)}
            title={choosing === 'poster' ? e.picker.titlePoster : e.picker.titleBackdrop}
            subtitle={work.title}
            work={work}
            shape={choosing === 'poster' ? 'poster' : 'wide'}
            candidates={(work.images ?? [])
              .filter((image) => image.coverType === choosing && (image.seasonNumber === undefined || image.seasonNumber === null))
              .map((image): Candidate => ({ image, group: 'work' }))}
            current={choosing === 'poster' ? sheetArt : art}
            uploadFields={{ coverType: choosing }}
            onChoose={(address) => lead(choosing, address)}
            // A picture sent as a file is one of the work's; it is then chosen.
            onUploaded={(kept: Uploaded) => lead(choosing, kept.origin)}
          />
        ) : null}

        {/* ── The tabs ───────────────────────────────────────────────── */}
        <nav className="sticky top-14 z-20 -mx-4 mb-6 border-b border-rule bg-ink/95 px-4 backdrop-blur sm:-mx-6 sm:px-6 lg:top-0 lg:mx-0 lg:px-0">
          {/* eslint-disable-next-line jsx-a11y/interactive-supports-focus -- the tabs take the focus, one at a time; the list hears their arrows */}
          <div role="tablist" aria-label={e.tabs.label} onKeyDown={onTabKey} className="flex gap-1 overflow-x-auto [scrollbar-width:none]">
            {tabs.map((one) => {
              const selected = one === shown
              return (
                <button
                  key={one}
                  id={`tab-${one}`}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  // Only the open tab has a panel in the page to point at.
                  aria-controls={selected ? `panel-${one}` : undefined}
                  tabIndex={selected ? 0 : -1}
                  onClick={() => choose(one)}
                  className={cn(
                    'relative inline-flex min-h-12 shrink-0 cursor-pointer items-center gap-2 rounded-card px-3.5 text-sm whitespace-nowrap transition-colors duration-150',
                    // The ring drawn inside the tab: the row scrolls sideways,
                    // which clips anything drawn past its top and bottom edges.
                    'focus-visible:outline-offset-[-3px]',
                    selected ? 'text-bone' : 'text-bone-dim hover:text-bone',
                  )}
                >
                  {e.tabs[one]}
                  {counts[one]}
                  {unsavedIn.has(one as 'record' | 'seasons') ? (
                    <span className="inline-flex items-center">
                      <span aria-hidden className="size-1.5 rounded-full bg-brass" />
                      <span className="sr-only">, {e.tabChanged}</span>
                    </span>
                  ) : null}
                  {selected ? <span aria-hidden className="absolute inset-x-3 -bottom-px h-0.5 rounded-full bg-vermillion" /> : null}
                </button>
              )
            })}
          </div>
        </nav>

        <DraftsProvider value={drafts}>
          <EditorLocksContext value={lockContext}>
            <div key={shown} id={`panel-${shown}`} role="tabpanel" aria-labelledby={`tab-${shown}`} className="rise" style={{ animationDelay: '40ms' }}>
              {shown === 'record' ? (
                <RecordTab
                  work={work}
                  registry={registry.data}
                  overrides={locks}
                  lockable={synced}
                  report={report.data}
                  reportFailed={report.isError}
                  onRetryReport={() => void report.refetch()}
                  language={language}
                  onLanguage={setLanguage}
                  refreshing={refresh.isPending}
                  onRefresh={() => setReviewing(true)}
                  onChanged={changed}
                />
              ) : shown === 'seasons' ? (
                <SeasonsTab work={work} registry={registry.data} overrides={locks} lockable={synced} report={report.data} onChanged={changed} />
              ) : shown === 'artwork' ? (
                <ArtworkTab work={work} onChanged={changed} />
              ) : shown === 'people' ? (
                <PeopleTab work={work} report={report.data} onChanged={changed} />
              ) : shown === 'journal' ? (
                <JournalTab work={work} registry={registry.data} onChanged={changed} />
              ) : (
                <ElsewhereTab work={work} overrides={locks} onChanged={changed} />
              )}
            </div>
          </EditorLocksContext>
        </DraftsProvider>
      </div>

      {besidePreview ? (
        <aside aria-label={e.preview.title} className="hidden xl:block">
          <div className="sticky top-6 h-[calc(100dvh-3rem)]">
            <Preview work={work} frame={frame} onClose={togglePreview} />
          </div>
        </aside>
      ) : null}
      {!wide && sheet ? <PreviewSheet work={work} frame={frame} onClose={() => setSheet(false)} /> : null}

      <SaveBar
        count={count}
        saving={saving}
        said={said}
        lockable={synced}
        next={next}
        onSave={(then) => void save(then)}
        onDiscard={clear}
      />
      {guard.dialog}

      <Dialog
        open={disabling}
        title={t.admin.works.disableTitle}
        onClose={() => setDisabling(false)}
        footer={
          <>
            <Button onClick={() => setDisabling(false)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={setEnabled.isPending} onClick={() => setEnabled.mutate(false)}>
              {t.admin.works.disable}
            </Button>
          </>
        }
      >
        {t.admin.works.disableBody(work.title)}
      </Dialog>
    </div>
  )
}

/**
 * Where the work stands in the catalogue's list, and the works on each side
 * of it: links, so a change not saved yet is asked about before they lead
 * away.
 */
function Walk({ at, total, previous, next }: { at: number; total: number; previous?: Neighbour; next?: Neighbour }) {
  const { t } = useI18n()
  const w = t.admin.editor.walk
  const step = 'grid size-11 place-items-center rounded-card text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone'

  return (
    <nav aria-label={w.label} className="flex items-center gap-0.5">
      {previous ? (
        <Link to={`/admin/catalogue/${previous.id}`} aria-label={w.previous(previous.title)} title={w.previous(previous.title)} className={step}>
          <Glyph name="chevronLeft" className="size-4" />
        </Link>
      ) : (
        <span aria-hidden className={cn(step, 'opacity-30 hover:bg-transparent')}>
          <Glyph name="chevronLeft" className="size-4" />
        </span>
      )}
      <span className="font-mono text-xs text-bone-faint tabular-nums">{w.position(at, total)}</span>
      {next ? (
        <Link to={`/admin/catalogue/${next.id}`} aria-label={w.next(next.title)} title={w.next(next.title)} className={step}>
          <Glyph name="chevronRight" className="size-4" />
        </Link>
      ) : (
        <span aria-hidden className={cn(step, 'opacity-30 hover:bg-transparent')}>
          <Glyph name="chevronRight" className="size-4" />
        </span>
      )}
    </nav>
  )
}

/**
 * The work's public page as a visitor sees it, framed from this same origin
 * (the server's policy lets this origin alone frame its pages), and reloaded
 * by the editor at each save — where the reader was in it kept.
 */
function Preview({
  work,
  frame,
  onClose,
}: {
  work: MediaItem
  frame: React.RefObject<HTMLIFrameElement | null>
  onClose: () => void
}) {
  const { t } = useI18n()
  const p = t.admin.editor.preview

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-panel border border-rule bg-ink-raised">
      <div className="flex items-start gap-2 border-b border-rule py-2 pr-1.5 pl-4">
        <div className="min-w-0 flex-1 py-1">
          <p className="label">{p.title}</p>
          <p className="mt-0.5 text-xs leading-relaxed text-bone-faint">{p.note}</p>
        </div>
        <a
          href={`/work/${work.id}`}
          target="_blank"
          rel="noreferrer"
          aria-label={p.open}
          title={p.open}
          className="grid size-11 shrink-0 place-items-center rounded-card text-bone-faint transition-colors duration-150 hover:bg-ink-high hover:text-bone"
        >
          <Glyph name="external" className="size-4" />
        </a>
        <IconButton glyph="close" label={p.close} onClick={onClose} />
      </div>
      <iframe
        ref={frame}
        src={`/work/${work.id}`}
        title={p.frame(work.title)}
        className="min-h-0 w-full flex-1 bg-ink"
        onLoad={(event) => {
          // Back where the reader was before the reload, once the page has
          // drawn itself again.
          const view = event.currentTarget.contentWindow
          const y = Number(event.currentTarget.dataset.scroll ?? 0)
          if (!view || !y) return
          window.setTimeout(() => {
            try {
              view.scrollTo(0, y)
            } catch {
              // Another origin's page: nothing to put back.
            }
          }, 400)
        }}
      />
    </div>
  )
}

/** The preview over the whole screen, below the width where it stands beside the editor. */
function PreviewSheet({
  work,
  frame,
  onClose,
}: {
  work: MediaItem
  frame: React.RefObject<HTMLIFrameElement | null>
  onClose: () => void
}) {
  const { t } = useI18n()
  const sheet = useRef<HTMLDialogElement>(null)

  useEffect(() => {
    const node = sheet.current
    if (node && !node.open) node.showModal()
    return () => {
      if (node?.open) node.close()
    }
  }, [])

  return (
    <dialog
      ref={sheet}
      aria-label={t.admin.editor.preview.title}
      onClose={onClose}
      className="m-0 h-dvh max-h-none w-full max-w-none bg-ink p-2 backdrop:bg-ink/80 sm:p-4"
    >
      <Preview work={work} frame={frame} onClose={onClose} />
    </dialog>
  )
}

/**
 * What was merged into the work, while the merge can be undone: one line a
 * merge, with the way back. Undone, the other record is a work of its own
 * again, and the line says so and leads to it.
 */
function MergedBanner({ id, onChanged }: { id: string; onChanged: () => void }) {
  const { t, locale } = useI18n()
  const b = t.admin.editor.merged
  const queryClient = useQueryClient()

  const merges = useQuery({
    queryKey: ['item', id, 'merges'],
    queryFn: () => api.get<MergeList>(`/items/${segment(id)}/merges`),
  })
  // The title travels with the request: once undone, the merge is gone from
  // the list the banner reads, and the line that says so still names it.
  const undo = useMutation({
    mutationFn: (gone: { id: string; title: string }) =>
      api.post<{ restored: string; taken: unknown[] }>(`/items/${segment(gone.id)}/unmerge`),
    onSuccess: () => {
      onChanged()
      void queryClient.invalidateQueries({ queryKey: ['item', id, 'merges'] })
      void queryClient.invalidateQueries({ queryKey: ['duplicates'] })
    },
  })

  const restored = undo.data
  const open = merges.data?.merges.filter((one) => one.work.id !== restored?.restored) ?? []
  if (!open.length && !restored) return null

  return (
    <div className="rise mb-4 space-y-2">
      {open.map(({ work: gone, undoUntil }) => (
        <p
          key={gone.id}
          className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-panel border border-brass-deep bg-brass/[0.06] px-4 py-2.5 text-sm text-bone"
        >
          <Glyph name="copy" className="size-4 shrink-0 text-brass" />
          <span className="min-w-0 flex-1">
            {b.banner(gone.title, fmt.relative(gone.mergedAt, locale) ?? '')}{' '}
            <span className="text-bone-dim">{b.until(fmt.dateTime(undoUntil, locale) ?? undoUntil)}</span>
          </span>
          <Button size="sm" disabled={undo.isPending} onClick={() => undo.mutate({ id: gone.id, title: gone.title })}>
            {undo.isPending && undo.variables.id === gone.id ? <Spinner className="size-3.5" /> : <Glyph name="arrowLeft" className="size-3.5" />}
            {undo.isPending && undo.variables.id === gone.id ? b.undoing : b.undo}
          </Button>
        </p>
      ))}
      {restored ? (
        <p role="status" className="flex flex-wrap items-center gap-x-3 gap-y-1 px-1 text-sm text-bone-dim">
          <span>
            {b.undone(undo.variables?.title ?? '')}
            {restored.taken.length ? ` ${b.taken(restored.taken.length)}` : ''}
          </span>
          <Link
            to={`/admin/catalogue/${restored.restored}`}
            className="inline-flex min-h-11 items-center text-bone underline decoration-rule-bright underline-offset-4 hover:text-vermillion"
          >
            {b.open}
          </Link>
        </p>
      ) : null}
      {undo.isError ? (
        <p role="alert" className="flex items-center gap-2 px-1 text-sm text-status-error">
          <Glyph name="alert" className="size-4 shrink-0" />
          {undo.error.message || b.failed}
        </p>
      ) : null}
    </div>
  )
}

function EditorSkeleton() {
  return (
    <div className="mx-auto max-w-7xl space-y-6">
      <div className="flex gap-5">
        <Skeleton className="hidden h-45 w-30 sm:block" />
        <div className="flex-1 space-y-3">
          <Skeleton className="h-5 w-40" />
          <Skeleton className="h-10 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      </div>
      <Skeleton className="h-12 w-full" />
      <Skeleton className="h-96 w-full" />
    </div>
  )
}
