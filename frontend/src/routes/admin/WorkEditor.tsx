/**
 * One entry, and everything a person may claim of it.
 *
 * The record is a card in five sections, each a tab: the fields and the
 * sources behind them, the seasons and their episodes, the artwork, the
 * people and the other titles, and the work as the rest of the world files
 * it. The address carries which is open, so a season's own page, the back
 * button and a bookmark all land where they meant to.
 *
 * The lock is the product, so it is the loudest thing on every row: a brass
 * edge, a padlock beside the field's name and the word itself in a chip.
 * Saving is what locks — that is the server's model, and the interface
 * refuses to invent a friendlier one, because an operator who thinks a field
 * is locked when it is not will lose their edit to the next sweep.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { Link, useLocation, useParams, useSearchParams } from 'react-router'

import { useAdminTitle } from '../../components/AdminShell'
import { Artwork } from '../../components/media'
import { Button, ButtonLink, Chip, Glyph, Provenance, Skeleton, Spinner } from '../../components/ui'
import { api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { statusLabel } from '../../lib/labels'
import { backdrop, hasSources, poster } from '../../lib/media'
import type { FieldRegistry, MediaItem, Override, ProvenanceReport } from '../../lib/types'

import { ArtworkTab } from './editor/ArtworkTab'
import { ElsewhereTab } from './editor/ElsewhereTab'
import { PeopleTab } from './editor/PeopleTab'
import { RecordTab } from './editor/RecordTab'
import { SeasonsTab } from './editor/SeasonsTab'
import { Count, Facts, LockBadge, TABS, isTab, tabOfAnchor, type Tab } from './editor/shared'

/** The tab an address asks for: its anchor first, then `?tab=`, then a season named. */
function askedTab(params: URLSearchParams, hash: string): Tab {
  const fromAnchor = tabOfAnchor(hash)
  if (fromAnchor) return fromAnchor
  const named = params.get('tab')
  if (isTab(named)) return named
  return params.has('season') ? 'seasons' : 'record'
}

export function WorkEditor() {
  const { id = '' } = useParams()
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
    queryFn: () => api.get<MediaItem>(`/items/${id}${query({ language })}`),
  })
  const registry = useQuery({
    queryKey: ['fields'],
    queryFn: () => api.get<FieldRegistry>('/fields'),
    staleTime: Infinity,
  })
  // Who gave what, and whom the work can be synced from.
  const report = useQuery({
    queryKey: ['item', id, 'provenance'],
    queryFn: () => api.get<ProvenanceReport>(`/items/${id}/provenance`),
  })
  const overrides = useQuery({
    queryKey: ['overrides', id],
    queryFn: () => api.get<Override[]>(`/items/${id}/overrides`),
  })

  useAdminTitle(item.data?.title ?? null)

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

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', id] })
    // The catalogue's own pages read the same work under another key.
    void queryClient.invalidateQueries({ queryKey: ['work', id] })
    void queryClient.invalidateQueries({ queryKey: ['overrides', id] })
    void queryClient.invalidateQueries({ queryKey: ['items'] })
    void queryClient.invalidateQueries({ queryKey: ['stats'] })
  }

  const refresh = useMutation({
    mutationFn: () => api.post<MediaItem>(`/items/${id}/refresh`),
    onSuccess: invalidate,
  })

  // The registry too, and not only the work: it is asked for once and cached
  // forever, so a failure there never retries. Without this the page waits on
  // `registry.data` that is never coming and shimmers for as long as it is open.
  if (item.isError || registry.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {t.admin.editor.loadFailed}
      </p>
    )
  }

  // A row needs both the value and its definition, so wait for the pair.
  if (!item.data || !registry.data) {
    return <EditorSkeleton />
  }

  const work = item.data
  const e = t.admin.editor
  // A work with no source elsewhere is never refreshed, so nothing keeps a
  // field from anything: its edits are its record, and none is shown as a
  // lock. The server still files them as overrides, which is what makes
  // them stick the day a source is given.
  const synced = hasSources(work)
  const locks = synced ? (overrides.data ?? []) : []
  const ownLocks = locks.filter((o) => o.scope === 'item').length
  const deeperLocks = locks.length - ownLocks
  const tabs = work.kind === 'series' ? TABS : TABS.filter((one) => one !== 'seasons')
  const shown: Tab = tabs.includes(tab) ? tab : 'record'
  const sheet = poster(work)
  const art = backdrop(work)
  const images = (work.images?.length ?? 0) + (work.seasons ?? []).reduce((sum, season) => sum + (season.images?.length ?? 0), 0)
  const identifiers = Object.values(work.externalIds).filter((value) => value !== undefined && value !== null && !(Array.isArray(value) && !value.length)).length

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
    artwork: images ? <Count>{images}</Count> : null,
    people: peopleCount ? (
      <Count>
        {work.credits?.length ?? 0} · {work.alternativeTitles?.length ?? 0}
      </Count>
    ) : null,
    elsewhere: identifiers ? <Count>{identifiers}</Count> : null,
  }

  return (
    <div className="mx-auto max-w-7xl">
      <Link
        to="/admin/catalogue"
        className="label hidden min-h-11 items-center gap-2 transition-colors duration-150 hover:text-vermillion lg:inline-flex"
      >
        <Glyph name="arrowLeft" className="size-3.5" />
        {t.admin.catalogue}
      </Link>

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

        <div className="relative grid grid-cols-1 gap-5 pt-4 pb-6 sm:grid-cols-[7.5rem_minmax(0,1fr)] sm:items-end xl:grid-cols-[7.5rem_minmax(0,1fr)_auto]">
          <div className="hidden aspect-2/3 w-30 overflow-hidden rounded-card border border-rule-bright bg-ink-high shadow-[var(--shadow-plate)] sm:block">
            {sheet ? (
              <Artwork url={sheet} role="poster" eager alt={t.a11y.poster(work.title)} className="size-full object-cover" />
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

            <p className="mt-2.5 flex items-start gap-2 text-[0.8125rem] text-bone-dim">
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
                  <span aria-hidden className="mt-1.5 size-2 shrink-0 rounded-full bg-moss shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-moss)_18%,transparent)]" />
                  <span>
                    {work.refreshedAt ? e.mast.refreshedWhen(fmt.relative(work.refreshedAt, locale) ?? '') : e.mast.never}
                    {work.refreshAfter ? ` · ${e.mast.nextWhen(fmt.relative(work.refreshAfter, locale) ?? '')}` : ''}
                  </span>
                </>
              )}
            </p>
            {refresh.isError ? (
              <p role="alert" className="mt-2 flex items-center gap-2 text-sm text-vermillion">
                <Glyph name="alert" className="size-4 shrink-0" />
                {refresh.error.message}
              </p>
            ) : null}
          </div>

          {/* Two to a row on a phone, a row of their own at the widest. */}
          <div className="grid grid-cols-2 gap-2 sm:col-span-2 sm:flex sm:flex-wrap xl:col-span-1 xl:justify-end xl:self-end xl:pb-1">
            {work.isManual ? null : (
              <Button onClick={() => refresh.mutate()} disabled={refresh.isPending}>
                {refresh.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
                {/* The whole phrase where there is room; the verb alone on a
                    phone, where three lines of button were half the masthead. */}
                <span className="sm:hidden">{refresh.isPending ? e.refreshing : t.common.refresh}</span>
                <span className="hidden sm:inline">{refresh.isPending ? e.refreshing : e.refresh}</span>
              </Button>
            )}
            <Link
              to={`/work/${id}`}
              className="inline-flex min-h-11 items-center justify-center gap-2 rounded-full border border-rule-bright px-5 text-sm font-medium text-bone transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high"
            >
              <Glyph name="reel" className="size-4" />
              {e.publicPage}
            </Link>
            <ButtonLink href={`/api/v1/items/${id}/nfo`} target="_blank" rel="noreferrer" title={e.nfoHint}>
              <Glyph name="download" className="size-4" />
              <span className="sm:hidden">.nfo</span>
              <span className="hidden sm:inline">{e.nfo}</span>
            </ButtonLink>
          </div>
        </div>
      </header>

      {/* ── The tabs ───────────────────────────────────────────────── */}
      <nav className="sticky top-14 z-20 -mx-4 mb-6 border-b border-rule bg-ink/95 px-4 backdrop-blur sm:-mx-6 sm:px-6 lg:top-0 lg:mx-0 lg:px-0">
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
                aria-controls={`panel-${one}`}
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
                {selected ? <span aria-hidden className="absolute inset-x-3 -bottom-px h-0.5 rounded-full bg-vermillion" /> : null}
              </button>
            )
          })}
        </div>
      </nav>

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
            onRefresh={() => refresh.mutate()}
            onChanged={invalidate}
          />
        ) : shown === 'seasons' ? (
          <SeasonsTab work={work} registry={registry.data} overrides={locks} lockable={synced} report={report.data} onChanged={invalidate} />
        ) : shown === 'artwork' ? (
          <ArtworkTab work={work} onChanged={invalidate} />
        ) : shown === 'people' ? (
          <PeopleTab work={work} report={report.data} onChanged={invalidate} />
        ) : (
          <ElsewhereTab work={work} overrides={locks} onChanged={invalidate} />
        )}
      </div>
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
