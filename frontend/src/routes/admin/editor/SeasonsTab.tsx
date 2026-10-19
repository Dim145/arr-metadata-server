/**
 * A series' seasons and episodes, as a workbench: the seasons down one side,
 * the one chosen laid out beside them — its own fields, then every episode,
 * each opening onto the fields a person may claim of it.
 *
 * One season at a time, and one episode open at a time: a series of a
 * thousand episodes drawn as a thousand forms is not a page anybody can use.
 * Arriving from a season's or an episode's public page opens straight onto it,
 * and the address keeps which season is open, so the back button does too.
 */

import { useMutation } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { Link, useSearchParams } from 'react-router'

import { ExternalLink } from '../../../components/elsewhere'
import { Placement, finaleLabel } from '../../../components/episodes'
import { Artwork } from '../../../components/media'
import {
  Button,
  Chip,
  Dialog,
  FormField,
  Glyph,
  Input,
  Panel,
  PanelHead,
  Provenance,
  Segmented,
  Spinner,
  Textarea,
} from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import * as fmt from '../../../lib/format'
import { useOrders } from '../../../lib/hooks'
import { useI18n } from '../../../lib/i18n'
import { providerName } from '../../../lib/labels'
import { episodeLinks } from '../../../lib/links'
import { airTime, airValue, episodeCode, episodesOf, seasonImages, seasonName, seasonNumbers, seasonPoster } from '../../../lib/media'
import type { Episode, FieldRegistry, Image, MediaItem, Override, ProvenanceReport, Uploaded } from '../../../lib/types'

import { FieldRow, hasValue } from './FieldRow'
import { ImagePicker, type Candidate } from './ImagePicker'
import { NumberingBanner } from './NumberingBanner'
import { orderName } from './orders'
import { OrdersPanel } from './OrdersPanel'
import { seasonPictures } from './pictures'
import { AddForm, Count, Empty, Facts, LockBadge, Yours, hostOf, useRemove } from './shared'

/** The kinds of picture an episode's still is cut from: the wide ones. */
const WIDE = new Set(['landscape', 'fanart', 'screenshot'])

/** What is being chosen: the season's poster, or an episode's still. */
type Picking = { for: 'season' } | { for: 'episode'; episode: Episode }

type Filter = 'all' | 'locked' | 'yours'

const scopeOf = (episode: Pick<Episode, 'seasonNumber' | 'episodeNumber'>) => `episode:${episode.seasonNumber}x${episode.episodeNumber}`

/** Where a special belongs: a question only a special is asked, unless an answer is already held. */
const PLACEMENT = new Set(['airedAfterSeasonNumber', 'airedBeforeSeasonNumber', 'airedBeforeEpisodeNumber'])

export function SeasonsTab({
  work,
  registry,
  overrides,
  lockable = true,
  report,
  onChanged,
}: {
  work: MediaItem
  registry: FieldRegistry
  overrides: Override[]
  /** Whether a lock means anything here: not on a work no refresh ever touches. */
  lockable?: boolean
  report?: ProvenanceReport
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const e = t.admin.editor
  const c = e.children
  const s = e.seasonsTab
  const [params, setParams] = useSearchParams()
  const numbers = seasonNumbers(work)

  // Absent is not zero: `Number(null)` opened every series with specials on
  // its specials.
  const asked = params.has('season') ? Number(params.get('season')) : Number.NaN
  const [season, setSeason] = useState<number>(numbers.includes(asked) ? asked : (numbers.find((n) => n > 0) ?? numbers[0] ?? 1))
  const [open, setOpen] = useState<number | null>(
    numbers.includes(asked) && params.get('episode') ? Number(params.get('episode')) : null,
  )
  const [filter, setFilter] = useState<Filter>('all')
  const [picking, setPicking] = useState<Picking | null>(null)
  // The season's dates moved together: a dialog that starts afresh each time
  // it opens, and stays mounted while it closes so the focus goes back.
  const [shifting, setShifting] = useState(false)
  const [shiftings, setShiftings] = useState(0)
  const orders = useOrders(work.id)
  const { ask, dialog } = useRemove(work, onChanged)
  const p = e.picker

  // A season's poster is a choice among pictures, locked like any edit:
  // `season:N / primaryPoster`, as the work's own choice is made.
  const choosePoster = (n: number, address: string) =>
    api.put(`/items/${work.id}/overrides`, { scope: `season:${n}`, field: 'primaryPoster', value: address }).then(onChanged)
  const unlockPoster = useMutation({
    mutationFn: (n: number) => api.delete(`/items/${work.id}/overrides/${encodeURIComponent(`season:${n}`)}/primaryPoster`),
    onSuccess: onChanged,
  })
  // An episode's still is the `image` field, locked as the row would lock it.
  const chooseStill = (episode: Episode, address: string) =>
    api
      .put(`/items/${work.id}/overrides`, { scope: scopeOf(episode), field: 'image', value: address })
      .then(onChanged)

  /** The posters on offer for a season: its own, the work's, the other seasons'. */
  const posterCandidates = (n: number): Candidate[] => [
    ...seasonImages(work, n)
      .filter((image) => image.coverType === 'poster')
      .map((image) => ({ image, group: 'season' as const })),
    ...(work.images ?? [])
      .filter((image) => image.coverType === 'poster' && (image.seasonNumber === undefined || image.seasonNumber === null))
      .map((image) => ({ image, group: 'work' as const })),
    ...numbers
      .filter((m) => m !== n)
      .flatMap((m) =>
        seasonImages(work, m)
          .filter((image) => image.coverType === 'poster')
          .map((image) => ({ image, group: 'seasons' as const, groupLabel: seasonName(work.seasons?.find((x) => x.seasonNumber === m)?.title, m, t.work.season) })),
      ),
  ]
  /** The wide pictures on offer for a still: the season's, the work's, the other seasons'. */
  const stillCandidates = (n: number): Candidate[] => {
    const wide = (image: Image) => WIDE.has(image.coverType)
    return [
      ...seasonImages(work, n).filter(wide).map((image) => ({ image, group: 'season' as const })),
      ...(work.images ?? [])
        .filter((image) => wide(image) && (image.seasonNumber === undefined || image.seasonNumber === null))
        .map((image) => ({ image, group: 'work' as const })),
      ...numbers
        .filter((m) => m !== n)
        .flatMap((m) =>
          seasonImages(work, m)
            .filter(wide)
            .map((image) => ({ image, group: 'seasons' as const, groupLabel: seasonName(work.seasons?.find((x) => x.seasonNumber === m)?.title, m, t.work.season) })),
        ),
    ]
  }

  const choose = (n: number) => {
    setSeason(n)
    setOpen(null)
    setParams(
      (prev) => {
        prev.set('tab', 'seasons')
        prev.set('season', String(n))
        prev.delete('episode')
        return prev
      },
      { replace: true },
    )
  }

  const meta = work.seasons?.find((x) => x.seasonNumber === season)
  const episodes = episodesOf(work, season)
  const name = seasonName(meta?.title, season, t.work.season)
  const lockOf = (scope: string, field: string) => overrides.find((o) => o.scope === scope && o.field === field)
  const locksIn = (scope: string) => overrides.filter((o) => o.scope === scope).length
  /** Every lock a season carries: its own, and its episodes'. */
  const locksOfSeason = (n: number) =>
    overrides.filter((o) => o.scope === `season:${n}` || o.scope.startsWith(`episode:${n}x`)).length

  const shown = episodes.filter((episode) =>
    filter === 'all' ? true : filter === 'locked' ? locksIn(scopeOf(episode)) > 0 : episode.isManual,
  )
  const counts = {
    locked: episodes.filter((episode) => locksIn(scopeOf(episode)) > 0).length,
    yours: episodes.filter((episode) => episode.isManual).length,
  }

  // The season's span, from its episodes' days: specials are numbered as they
  // were found, not as they aired, so the dates are sorted rather than taken
  // from the first and the last.
  const days = episodes
    .map((episode) => airTime(episode)?.day)
    .filter((day): day is string => !!day)
    .sort()
  const first = days[0] ?? meta?.airDate?.slice(0, 10)
  const last = days.at(-1)
  const span = first
    ? last && last !== first
      ? `${fmt.shortDate(first, locale)} → ${fmt.shortDate(last, locale)}`
      : fmt.shortDate(first, locale)
    : undefined
  const sheet = seasonPoster(work, season)
  const spine = report?.provenance?.episodes
  const otherOrders = orders.data?.orders ?? []

  const episodeFilter = (
    <Segmented<Filter>
      label={e.filter.label}
      value={filter}
      onChange={setFilter}
      options={[
        { value: 'all', label: e.filter.all },
        ...(lockable
          ? [
              {
                value: 'locked' as const,
                label: (
                  <span className="inline-flex items-center gap-1.5">
                    {e.filter.locked}
                    <Count>{counts.locked}</Count>
                  </span>
                ),
              },
            ]
          : []),
        {
          value: 'yours',
          label: (
            <span className="inline-flex items-center gap-1.5">
              {e.filter.yours}
              <Count>{counts.yours}</Count>
            </span>
          ),
        },
      ]}
    />
  )

  return (
    <div className="grid grid-cols-1 gap-6 lg:grid-cols-[16rem_minmax(0,1fr)] lg:items-start">
      {/* While TMDB numbers the series in TheTVDB's place: said above
          everything the numbering moves, with the way back. */}
      {orders.data ? (
        <NumberingBanner workId={work.id} orders={orders.data} canWrite onChanged={onChanged} className="lg:col-span-2" />
      ) : null}

      {/* ── The seasons ────────────────────────────────────────────── */}
      <nav aria-label={c.seasons} id="seasons" className="scroll-mt-28 lg:sticky lg:top-16 lg:scroll-mt-16">
        {numbers.length ? (
        <ol
          className={cn(
            '-mx-4 flex gap-2 overflow-x-auto px-4 pb-1 [scrollbar-width:none] sm:-mx-6 sm:px-6',
            // Sticky beside the fields, the rail scrolls within itself past
            // the viewport's height: a series of forty seasons kept its last
            // ones under the fold, where a sticky box reaches only at the
            // page's end.
            'lg:mx-0 lg:block lg:max-h-[calc(100dvh-5rem)] lg:space-y-0.5 lg:overflow-y-auto lg:rounded-panel lg:border lg:border-rule lg:p-2 lg:px-4 lg:film-strip-y',
          )}
        >
          {numbers.map((n) => {
            const own = work.seasons?.find((x) => x.seasonNumber === n)
            const eps = episodesOf(work, n)
            const year = eps.map((episode) => airTime(episode)?.day).filter(Boolean).sort()[0] ?? own?.airDate
            const art = seasonPoster(work, n)
            const current = n === season
            return (
              <li key={n} className="shrink-0 lg:shrink">
                <button
                  type="button"
                  aria-current={current ? 'true' : undefined}
                  onClick={() => choose(n)}
                  className={cn(
                    'flex min-h-13 w-full cursor-pointer items-center gap-3 rounded-card border px-3 py-1.5 text-left',
                    'transition-colors duration-150 lg:border-0',
                    current
                      ? 'border-vermillion/60 bg-ink-top text-bone lg:shadow-[inset_2px_0_0_var(--color-vermillion)]'
                      : 'border-rule bg-ink-raised text-bone-dim hover:bg-ink-high hover:text-bone lg:bg-transparent',
                  )}
                >
                  <span className="aspect-2/3 w-7 shrink-0 overflow-hidden rounded-[4px] border border-rule bg-ink-high">
                    {art ? <Artwork url={art} role="thumb" alt="" className="size-full object-cover" /> : null}
                  </span>
                  <span className="min-w-0 lg:flex-1">
                    <span className="block truncate text-sm font-medium whitespace-nowrap">{seasonName(own?.title, n, t.work.season)}</span>
                    <span className="block font-mono text-[0.6875rem] text-bone-faint tabular-nums whitespace-nowrap">
                      S{String(n).padStart(2, '0')} · {t.work.episodeCount(eps.length)}
                      {year ? ` · ${fmt.year(year)}` : ''}
                    </span>
                  </span>
                  {own?.isManual ? (
                    <span className="size-1.5 shrink-0 rounded-full bg-brass" title={c.yours}>
                      <span className="sr-only">{c.yours}</span>
                    </span>
                  ) : null}
                  <LockBadge n={locksOfSeason(n)} className="shrink-0" />
                </button>
              </li>
            )
          })}
        </ol>
        ) : null}
        <div className={cn('rounded-panel border border-rule bg-ink-raised', numbers.length && 'mt-2')}>
          <AddSeason work={work} numbers={numbers} onChanged={onChanged} onAdded={choose} />
        </div>
      </nav>

      {/* ── The season chosen ─────────────────────────────────────── */}
      <div className="min-w-0 space-y-6">
        {numbers.length === 0 ? (
          <Panel className="rise">
            <Empty>{s.noSeasons}</Empty>
          </Panel>
        ) : (
          <>
        <Panel id="season-fields" label={name} className="rise">
          <div className="grid grid-cols-[5rem_minmax(0,1fr)] gap-5 p-5 sm:grid-cols-[6rem_minmax(0,1fr)]">
            {/* The poster, and the way to choose it: decided where it is seen. */}
            <div>
              <div className="aspect-2/3 overflow-hidden rounded-card border border-rule-bright bg-ink-high shadow-[var(--shadow-lift)]">
                {sheet ? (
                  <Artwork url={sheet} role="thumb" alt="" className="size-full object-cover" />
                ) : (
                  <div className="grid size-full place-items-center">
                    <Glyph name="tv" className="size-5 text-bone-faint" />
                  </div>
                )}
              </div>
              {meta ? (
                <>
                  <Button size="sm" className="mt-2 w-full px-2" onClick={() => setPicking({ for: 'season' })} aria-label={p.changePoster}>
                    <Glyph name="image" className="size-3.5" />
                    {p.change}
                  </Button>
                  {lockOf(`season:${season}`, 'primaryPoster') ? (
                    <div className="mt-2 flex flex-col items-start gap-1" data-field="primaryPoster">
                      <Provenance manual label={p.chosenPoster} />
                      <Button size="sm" variant="quiet" className="px-2" onClick={() => unlockPoster.mutate(season)} disabled={unlockPoster.isPending}>
                        <Glyph name="unlock" className="size-3.5" />
                        {e.unlock}
                      </Button>
                    </div>
                  ) : null}
                </>
              ) : null}
            </div>
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-1.5">
                {meta?.isManual ? <Provenance manual label={t.work.manualEntry} /> : null}
                {locksOfSeason(season) ? (
                  <Chip tone="manual">
                    <Glyph name="lock" className="size-3" />
                    {e.lockCount(locksOfSeason(season))}
                  </Chip>
                ) : null}
              </div>
              <h2 className="mt-1.5 font-display text-2xl leading-tight font-medium text-bone">{name}</h2>
              <Facts
                className="mt-2"
                items={[
                  t.work.episodeCount(episodes.length),
                  span,
                  meta?.tmdbId ? `${providerName('tmdb')} ${meta.tmdbId}` : undefined,
                  meta?.tvdbId ? `${providerName('tvdb')} ${meta.tvdbId}` : undefined,
                ]}
              />
              <div className="mt-4 flex flex-wrap items-center gap-2">
                <Link
                  to={`/work/${work.id}/season/${season}`}
                  className="inline-flex min-h-11 items-center gap-1.5 rounded-full border border-rule-bright px-4 text-[0.8125rem] font-medium text-bone transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high"
                >
                  <Glyph name="reel" className="size-4" />
                  {s.seasonPage}
                </Link>
                <Button
                  size="sm"
                  variant="quiet"
                  disabled={!episodes.some(isDated)}
                  title={episodes.some(isDated) ? undefined : s.shift.none}
                  onClick={() => {
                    setShiftings((n) => n + 1)
                    setShifting(true)
                  }}
                >
                  <Glyph name="calendar" className="size-4" />
                  {s.shift.open}
                </Button>
                {meta?.isManual ? (
                  <Button size="sm" variant="quiet" onClick={() => ask({ path: `seasons/${season}`, label: name })}>
                    <Glyph name="trash" className="size-4" />
                    {s.removeSeason}
                  </Button>
                ) : null}
              </div>
            </div>
          </div>

          {meta ? (
            <ul className="divide-y divide-rule border-t border-rule">
              {/* The poster's choice is made from the poster above, not a row. */}
              {registry.season.filter((def) => def.name !== 'primaryPoster').map((def) => (
                <FieldRow
                  key={`season:${season}:${def.name}`}
                  itemId={work.id}
                  scope={`season:${season}`}
                  def={def}
                  value={(meta as unknown as Record<string, unknown>)[def.name]}
                  lock={lockOf(`season:${season}`, def.name)}
                  onChanged={onChanged}
                />
              ))}
            </ul>
          ) : (
            <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{s.noSeasonRow}</p>
          )}
        </Panel>

        <SeasonPictures work={work} season={season} chosen={meta?.primaryImages?.poster ?? undefined} ask={ask} onChanged={onChanged} />

        <Panel id="episodes" label={c.episodes} className="rise" style={{ animationDelay: '40ms' }}>
          {/* The filter beside the head where there is room, and under it on
              a phone, where the two shared one line badly. */}
          <PanelHead
            title={
              <span className="flex items-center gap-2">
                {c.episodes}
                <Count>{episodes.length}</Count>
              </span>
            }
            action={episodes.length ? <div className="hidden sm:block">{episodeFilter}</div> : undefined}
          />
          {episodes.length ? <div className="border-b border-rule px-5 py-2 sm:hidden">{episodeFilter}</div> : null}

          {shown.length ? (
            <ol className="divide-y divide-rule">
              {shown.map((episode) => {
                const scope = scopeOf(episode)
                const expanded = open === episode.episodeNumber
                return (
                  <EpisodeRow
                    key={episode.id}
                    work={work}
                    episode={episode}
                    expanded={expanded}
                    locks={locksIn(scope)}
                    onToggle={() => setOpen(expanded ? null : episode.episodeNumber)}
                    onRemove={
                      episode.isManual
                        ? () =>
                            ask({
                              path: `episodes/${episode.seasonNumber}/${episode.episodeNumber}`,
                              label: `${episodeCode(episode)} ${episode.title}`.trim(),
                            })
                        : undefined
                    }
                  >
                    <ul className="divide-y divide-rule">
                      {registry.episode
                        .filter(
                          (def) =>
                            !PLACEMENT.has(def.name) ||
                            episode.seasonNumber === 0 ||
                            hasValue(def, (episode as unknown as Record<string, unknown>)[def.name]) ||
                            lockOf(scope, def.name) !== undefined,
                        )
                        .map((def) => (
                          <FieldRow
                            key={`${scope}:${def.name}`}
                            itemId={work.id}
                            scope={scope}
                            def={def}
                            value={(episode as unknown as Record<string, unknown>)[def.name]}
                            lock={lockOf(scope, def.name)}
                            // The still is a picture: chosen as one, not typed.
                            onEdit={def.name === 'image' ? () => setPicking({ for: 'episode', episode }) : undefined}
                            onChanged={onChanged}
                          />
                        ))}
                    </ul>
                  </EpisodeRow>
                )
              })}
            </ol>
          ) : (
            <Empty>{episodes.length ? e.filter.none : s.noEpisodes}</Empty>
          )}

          <AddEpisode work={work} season={season} episodes={episodes} onChanged={onChanged} onAdded={setOpen} />

          <p className="flex items-start gap-2 border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">
            <Glyph name="cloud" className="mt-0.5 size-3.5 shrink-0 text-slate" />
            <span>
              {spine ? e.origin.episodesFrom(providerName(spine)) : t.work.numbering}
              {otherOrders.length ? (
                <>
                  {' '}
                  {s.otherOrders(fmt.list(otherOrders.map((order) => orderName(order, t.season.order)), locale))}{' '}
                  <a href="#order-editor" className="text-vermillion underline-offset-4 hover:underline">
                    {s.seeOrders}
                  </a>
                </>
              ) : null}{' '}
              {c.onlyManual}
            </span>
          </p>
        </Panel>
          </>
        )}
      </div>

      {/* ── The orders: the whole width, beneath the seasons ───────── */}
      <OrdersPanel work={work} onChanged={onChanged} />

      {dialog}

      <ShiftDates
        key={`shift-${season}-${shiftings}`}
        open={shifting}
        work={work}
        name={name}
        episodes={episodes}
        lockable={lockable}
        onClose={() => setShifting(false)}
        onChanged={onChanged}
      />

      {/* One picker for the tab: the season's poster or an episode's still,
          whichever was asked for last. */}
      {picking?.for === 'episode' ? (
        <ImagePicker
          open
          onClose={() => setPicking(null)}
          title={p.titleStill}
          subtitle={`${work.title} · ${episodeCode(picking.episode)} ${picking.episode.title}`.trim()}
          work={work}
          shape="wide"
          candidates={stillCandidates(picking.episode.seasonNumber)}
          current={picking.episode.image}
          uploadFields={{ episode: `${picking.episode.seasonNumber}x${picking.episode.episodeNumber}` }}
          onChoose={(address) => chooseStill(picking.episode, address)}
          // A still sent as a file locks the field as it lands: nothing more to claim.
          onUploaded={() => onChanged()}
        />
      ) : (
        <ImagePicker
          open={picking?.for === 'season'}
          onClose={() => setPicking(null)}
          title={p.titlePoster}
          subtitle={`${work.title} · ${name}`}
          work={work}
          shape="poster"
          candidates={posterCandidates(season)}
          current={sheet}
          uploadFields={{ coverType: 'poster', seasonNumber: String(season) }}
          onChoose={(address) => choosePoster(season, address)}
          // A poster sent as a file is one of the season's; it is then chosen.
          onUploaded={(kept: Uploaded) => choosePoster(season, kept.origin)}
        />
      )}
    </div>
  )
}

/* ── A season's pictures ──────────────────────────────────────────────────── */

/**
 * Every picture attached to the season, hidden ones included. A source's is
 * hidden rather than removed, as the work's own are on the Artwork tab — a
 * lock on its address, kept through every refresh — and this is the one
 * place a season's are listed, so the one place a hidden one is shown again.
 * The poster chosen for the season is served whatever is hidden, as the
 * server has it, so it offers no hide; a picture added by hand is removed
 * instead.
 */
function SeasonPictures({
  work,
  season,
  chosen,
  ask,
  onChanged,
}: {
  work: MediaItem
  season: number
  /** The id of the poster chosen for the season, when one was. */
  chosen?: string
  ask: (target: { path: string; label: string }) => void
  onChanged: () => void
}) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const s = t.admin.editor.seasonsTab.pictures
  const hide = useMutation({
    mutationFn: ({ id, hidden }: { id: string; hidden: boolean }) => api.patch(`/items/${work.id}/images/${id}`, { hidden }),
    onSuccess: onChanged,
  })
  const pictures = seasonPictures(work, season)
  const hiddenCount = pictures.filter((picture) => picture.hidden).length
  const kindLabel = (value: string) => (t.gallery.kind as Record<string, string>)[value] ?? value

  return (
    <Panel id="season-pictures" label={s.title} className="rise" style={{ animationDelay: '20ms' }}>
      <PanelHead
        title={
          <span className="flex items-center gap-2">
            {s.title}
            <Count>{pictures.length}</Count>
          </span>
        }
        action={hiddenCount ? <span className="label hidden sm:inline">{s.hiddenCount(hiddenCount)}</span> : undefined}
      />
      {pictures.length === 0 ? (
        <Empty>{s.none}</Empty>
      ) : (
        <ul className="divide-y divide-rule">
          {pictures.map(({ image, hidden }) => {
            const isChosen = image.id === chosen
            const wide = image.coverType !== 'poster'
            const from = image.isManual ? c.yours : image.source && image.source !== 'manual' && image.source !== 'unknown' ? providerName(image.source) : undefined
            const name = [kindLabel(image.coverType), image.language].filter(Boolean).join(' · ')
            const busy = hide.isPending && hide.variables?.id === image.id
            return (
              <li
                key={image.id}
                data-hidden={hidden || undefined}
                className={cn(
                  'flex items-center gap-3 px-5 py-2.5',
                  (image.isManual || hidden) && 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]',
                )}
              >
                <span
                  className={cn(
                    'shrink-0 overflow-hidden rounded-[4px] border border-rule bg-ink-high',
                    wide ? 'aspect-video w-14' : 'aspect-2/3 w-9',
                    hidden && 'opacity-40 grayscale',
                  )}
                >
                  <Artwork url={image.url} role="thumb" alt="" className="size-full object-cover" />
                </span>
                {/* Faded and struck through when hidden: the row stays, to be
                    read and shown again, and reads as what it is. */}
                <span className={cn('min-w-0 flex-1', hidden && 'opacity-60')}>
                  <span className={cn('block truncate text-sm text-bone', hidden && 'line-through decoration-bone-faint')}>{name}</span>
                  <span className="block truncate font-mono text-[0.6875rem] text-bone-faint" title={image.url}>
                    {[from, hostOf(image.url)].filter(Boolean).join(' · ')}
                  </span>
                </span>
                {isChosen ? (
                  <Chip tone="manual">
                    <Glyph name="star" className="size-3" />
                    {t.admin.editor.picker.chosenPoster}
                  </Chip>
                ) : null}
                {hidden ? (
                  <Chip tone="manual">
                    <Glyph name="lock" className="size-3" />
                    {c.hidden}
                  </Chip>
                ) : null}
                {image.isManual ? (
                  <Yours onRemove={() => ask({ path: `images/${image.id}`, label: image.url })} />
                ) : isChosen ? null : (
                  <Button
                    size="sm"
                    variant={hidden ? undefined : 'quiet'}
                    disabled={busy}
                    aria-label={hidden ? c.showNamed(name) : c.hideNamed(name)}
                    onClick={() => hide.mutate({ id: image.id, hidden: !hidden })}
                  >
                    {busy ? <Spinner className="size-4" /> : <Glyph name={hidden ? 'unlock' : 'lock'} className="size-4" />}
                    {hidden ? c.showAgain : c.hide}
                  </Button>
                )}
              </li>
            )
          })}
        </ul>
      )}
      {hide.isError ? (
        <p role="alert" className="border-t border-rule px-5 py-3 text-xs text-vermillion">
          {hide.error.message || t.common.actionFailed}
        </p>
      ) : null}
      <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">
        {c.hiddenHint} {s.chosen}
      </p>
    </Panel>
  )
}

/* ── A season's dates, moved together ─────────────────────────────────────── */

/** The furthest a season's dates are moved at once: ten years, either way. */
const SHIFT_MOST = 3650

/** Whether an episode has a date to move. */
const isDated = (episode: Episode) => Boolean(episode.airDate || episode.airDateUtc)

/** An instant `days` × 24 hours on — or back: in UTC a day is always that long. */
function shiftInstant(value: string | undefined, days: number): string | undefined {
  if (!value) return undefined
  const at = Date.parse(value)
  if (Number.isNaN(at)) return undefined
  return new Date(at + days * 86_400_000).toISOString().replace(/\.000Z$/, 'Z')
}

/** A day `days` on — or back — in the calendar, in the form the field holds it: a day, or an instant. */
function shiftDay(value: string | undefined, days: number): string | undefined {
  const day = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value ?? '')
  if (!day) return shiftInstant(value, days)
  return new Date(Date.UTC(Number(day[1]), Number(day[2]) - 1, Number(day[3]) + days)).toISOString().slice(0, 10)
}

/**
 * Every episode of a season moved by the same number of days — a season that
 * a channel pushed back a week, a broadcast that ran a day late abroad: the
 * day each aired and, where it is held, the instant. Each date is locked at
 * its new value, episode by episode, as an edit by hand would lock it, and
 * what changes is laid out before anything does.
 */
function ShiftDates({
  open,
  work,
  name,
  episodes,
  lockable,
  onClose,
  onChanged,
}: {
  open: boolean
  work: MediaItem
  name: string
  episodes: Episode[]
  lockable: boolean
  onClose: () => void
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const s = t.admin.editor.seasonsTab.shift
  const [days, setDays] = useState('7')
  const [tried, setTried] = useState(false)
  const [progress, setProgress] = useState<{ at: number; total: number } | null>(null)
  const [report, setReport] = useState<{ text: string; failures: string[] } | null>(null)

  // The minus sign some keyboards type is the hyphen's.
  const typed = days.trim().replace('−', '-')
  const valid = /^-?\d{1,4}$/.test(typed) && Number(typed) !== 0 && Math.abs(Number(typed)) <= SHIFT_MOST
  const steps = valid ? Number(typed) : 0
  const plan = episodes.filter(isDated).map((episode) => ({
    episode,
    airDate: shiftDay(episode.airDate, steps),
    airDateUtc: shiftInstant(episode.airDateUtc, steps),
  }))
  const dayOf = (episode: Pick<Episode, 'airDate' | 'airDateUtc'>) => episode.airDate ?? episode.airDateUtc
  const busy = progress !== null

  const apply = async () => {
    setTried(true)
    if (!valid || !plan.length) return
    setReport(null)
    setProgress({ at: 0, total: plan.length })
    let done = 0
    const failures: string[] = []
    const queue = [...plan]
    // Each episode its own requests, four episodes at a time: one refused
    // keeps the others from nothing, and each date is its own line in the
    // audit trail.
    const worker = async () => {
      for (let next = queue.shift(); next !== undefined; next = queue.shift()) {
        const scope = `episode:${next.episode.seasonNumber}x${next.episode.episodeNumber}`
        try {
          if (next.airDate) {
            await api.put(`/items/${work.id}/overrides`, { scope, field: 'airDate', value: next.airDate })
          }
          if (next.airDateUtc) {
            await api.put(`/items/${work.id}/overrides`, { scope, field: 'airDateUtc', value: next.airDateUtc })
          }
          done += 1
        } catch (error) {
          failures.push(`${episodeCode(next.episode)} — ${error instanceof Error ? error.message : t.common.actionFailed}`)
        }
        setProgress((now) => (now ? { ...now, at: now.at + 1 } : now))
      }
    }
    await Promise.all(Array.from({ length: Math.min(4, plan.length) }, worker))
    setProgress(null)
    setReport({ text: failures.length ? s.failed(done, failures.length) : s.done(done), failures })
    onChanged()
  }

  return (
    <Dialog
      open={open}
      size="lg"
      title={s.title(name)}
      onClose={() => {
        if (!busy) onClose()
      }}
      footer={
        report ? (
          <Button onClick={onClose}>{s.close}</Button>
        ) : (
          <>
            <Button disabled={busy} onClick={onClose}>
              {t.common.cancel}
            </Button>
            <Button type="submit" form="shift-dates" variant="primary" disabled={busy || !plan.length}>
              {busy ? <Spinner className="size-4" /> : <Glyph name="calendar" className="size-4" />}
              {s.apply(plan.length)}
            </Button>
          </>
        )
      }
    >
      <form
        id="shift-dates"
        noValidate
        className="space-y-4"
        onSubmit={(event) => {
          event.preventDefault()
          void apply()
        }}
      >
        <div className="max-w-48">
          <FormField label={s.days} htmlFor="shift-days" error={tried && !valid ? s.invalid : undefined}>
            <Input
              id="shift-days"
              inputMode="numeric"
              value={days}
              disabled={busy || report !== null}
              aria-invalid={tried && !valid ? true : undefined}
              aria-describedby="shift-days-hint"
              onChange={(event) => setDays(event.target.value)}
            />
          </FormField>
        </div>
        <p id="shift-days-hint" className="text-xs leading-relaxed text-bone-faint">
          {s.daysHint} {lockable ? s.locks : s.edits}
        </p>
      </form>

      {report ? (
        <div className="mt-4 space-y-2">
          <p role="status" className="text-sm text-bone">
            {report.text}
          </p>
          {report.failures.length ? (
            <ul className="space-y-1 text-xs text-vermillion">
              {report.failures.map((failure) => (
                <li key={failure}>{failure}</li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : progress ? (
        <p className="mt-4 font-mono text-xs text-bone-dim tabular-nums">
          {s.applying(progress.at, progress.total)}
        </p>
      ) : plan.length ? (
        <table className="mt-4 w-full border-collapse text-left text-sm">
          <caption className="label mb-2 text-left">{s.preview}</caption>
          <thead>
            <tr className="border-b border-rule">
              <th scope="col" className="label py-2 pr-3 font-normal">
                {s.episode}
              </th>
              <th scope="col" className="label py-2 pr-3 font-normal">
                {s.now}
              </th>
              <th scope="col" className="label py-2 font-normal">
                {s.after}
              </th>
            </tr>
          </thead>
          <tbody>
            {plan.map(({ episode, airDate, airDateUtc }) => (
              <tr key={episode.id} className="border-b border-rule last:border-0">
                <th scope="row" className="py-2 pr-3 font-normal text-bone">
                  <span className="font-mono text-xs text-bone-faint tabular-nums">{episodeCode(episode)}</span>{' '}
                  <span className="break-words">{episode.title || t.episode.untitled(episode.episodeNumber)}</span>
                </th>
                <td className="py-2 pr-3 font-mono text-xs whitespace-nowrap text-bone-dim tabular-nums">
                  {fmt.shortDate(dayOf(episode), locale)}
                </td>
                <td className="py-2 font-mono text-xs whitespace-nowrap text-bone tabular-nums">
                  {valid ? fmt.shortDate(airDate ?? airDateUtc, locale) : '—'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="mt-4 text-sm text-bone-faint">{s.none}</p>
      )}
    </Dialog>
  )
}

/* ── One episode ──────────────────────────────────────────────────────────── */

function EpisodeRow({
  work,
  episode,
  expanded,
  locks,
  onToggle,
  onRemove,
  children,
}: {
  work: MediaItem
  episode: Episode
  expanded: boolean
  locks: number
  onToggle: () => void
  onRemove?: () => void
  children: React.ReactNode
}) {
  const { t, locale } = useI18n()
  const s = t.admin.editor.seasonsTab
  const ref = useRef<HTMLLIElement>(null)

  // Opened from an episode's public page: bring it into view once.
  useEffect(() => {
    if (expanded) ref.current?.scrollIntoView({ block: 'nearest' })
    // Only on first open; later toggles are the reader's own doing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const panel = `episode-fields-${episode.seasonNumber}x${episode.episodeNumber}`
  const when = airTime(episode)
  const time = when?.moment ? fmt.clock(when.moment.toISOString(), locale) : undefined
  const links = episodeLinks(work, episode)
  const meta = [
    fmt.shortDate(airValue(when), locale),
    time,
    fmt.runtime(episode.runtime, locale),
    episode.rating?.value ? `★ ${fmt.score(episode.rating.value, locale)}` : undefined,
    episode.absoluteEpisodeNumber && episode.absoluteEpisodeNumber !== episode.episodeNumber
      ? t.episode.absolute(episode.absoluteEpisodeNumber)
      : undefined,
  ]
    .filter(Boolean)
    .join(' · ')

  return (
    <li ref={ref}>
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={panel}
        onClick={onToggle}
        className={cn(
          'grid w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 px-4 py-2.5 text-left',
          'transition-colors duration-150 hover:bg-ink-high sm:grid-cols-[3.75rem_5.5rem_minmax(0,1fr)_auto] sm:gap-x-4 sm:px-5',
          expanded && 'bg-ink-high',
        )}
      >
        <span className="font-mono text-xs tracking-wider text-bone-faint tabular-nums">{episodeCode(episode)}</span>
        <span className="hidden aspect-video overflow-hidden rounded-[6px] bg-ink-high sm:block">
          {episode.image ? <Artwork url={episode.image} role="still" alt="" className="size-full object-cover" /> : null}
        </span>
        <span className="min-w-0">
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm font-medium text-bone">
            <span className="min-w-0 truncate">{episode.title || t.episode.untitled(episode.episodeNumber)}</span>
            {episode.finaleType ? <Chip tone="accent">{finaleLabel(episode.finaleType, t)}</Chip> : null}
            {episode.isManual ? (
              <Chip tone="manual">
                <Glyph name="lock" className="size-3" />
                {t.admin.editor.children.yours}
              </Chip>
            ) : null}
          </span>
          <span className="mt-0.5 block truncate font-mono text-[0.6875rem] text-bone-faint tabular-nums">{meta || t.episode.noDate}</span>
        </span>
        <span className="flex items-center gap-2 text-bone-faint">
          <LockBadge n={locks} />
          <Glyph name={expanded ? 'chevronDown' : 'chevronRight'} className="size-3.5" />
        </span>
      </button>

      {expanded ? (
        <div id={panel} className="border-t border-rule bg-ink/40">
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-rule px-5 py-2.5 text-xs text-bone-faint">
            <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
              {episode.rating?.value ? (
                <span className="tabular-nums">
                  {s.rating(fmt.score(episode.rating.value, locale) ?? '', t.work.votes(fmt.count(episode.rating.votes, locale), episode.rating.votes))}
                </span>
              ) : null}
              {links.map((link) => (
                <ExternalLink key={link.source} href={link.href} className="font-mono tabular-nums">
                  {providerName(link.source)} {link.source === 'tvdb' ? episode.tvdbId : episode.tmdbId}
                </ExternalLink>
              ))}
              <Placement episode={episode} className="text-xs" />
            </span>
            <span className="flex items-center gap-1">
              <Link
                to={`/work/${work.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
                className="hit inline-flex min-h-8 items-center gap-1 rounded-full px-2 text-xs text-vermillion transition-colors duration-150 hover:bg-vermillion/10"
              >
                {s.episodePage}
                <Glyph name="chevronRight" className="size-3" />
              </Link>
              {onRemove ? (
                <button
                  type="button"
                  onClick={onRemove}
                  className="hit inline-flex min-h-8 cursor-pointer items-center gap-1 rounded-full px-2 text-xs text-bone-faint transition-colors duration-150 hover:bg-ink-high hover:text-vermillion"
                >
                  <Glyph name="trash" className="size-3" />
                  {s.removeEpisode}
                </button>
              ) : null}
            </span>
          </div>
          {children}
        </div>
      ) : null}
    </li>
  )
}

/* ── Adding by hand ───────────────────────────────────────────────────────── */

function AddSeason({
  work,
  numbers,
  onChanged,
  onAdded,
}: {
  work: MediaItem
  numbers: number[]
  onChanged: () => void
  onAdded: (season: number) => void
}) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const s = t.admin.editor.seasonsTab
  const next = numbers.length ? Math.max(...numbers) + 1 : 1
  const [number, setNumber] = useState(String(next))
  const [title, setTitle] = useState('')
  const [airDate, setAirDate] = useState('')
  const [overview, setOverview] = useState('')

  const add = useMutation({
    mutationFn: (body: unknown) => api.post<{ id: string }>(`/items/${work.id}/seasons`, body),
    onSuccess: (_, body) => {
      setTitle('')
      setAirDate('')
      setOverview('')
      setNumber(String(Number(number) + 1))
      onChanged()
      onAdded((body as { seasonNumber: number }).seasonNumber)
    },
  })

  return (
    <AddForm
      label={c.addSeason}
      pending={add.isPending}
      error={add.error}
      className="border-t-0"
      onSubmit={() =>
        add.mutate({
          seasonNumber: Number(number),
          title: title || undefined,
          airDate: airDate || undefined,
          overview: overview || undefined,
        })
      }
    >
      {/* Two abreast on a phone, where the rail is the width of the screen;
          one under the other on a desktop, where the rail is a narrow column. */}
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-1">
        <FormField label={c.seasonNumber} htmlFor="season-number">
          <Input id="season-number" required inputMode="numeric" value={number} onChange={(event) => setNumber(event.target.value.replace(/\D/g, ''))} />
        </FormField>
        <FormField label={c.airDate} htmlFor="season-air-date">
          <Input id="season-air-date" type="date" value={airDate} onChange={(event) => setAirDate(event.target.value)} />
        </FormField>
      </div>
      <FormField label={c.seasonTitle} htmlFor="season-title">
        <Input id="season-title" value={title} onChange={(event) => setTitle(event.target.value)} />
      </FormField>
      <FormField label={s.overview} htmlFor="season-overview">
        <Textarea id="season-overview" rows={3} value={overview} onChange={(event) => setOverview(event.target.value)} />
      </FormField>
    </AddForm>
  )
}

function AddEpisode({
  work,
  season,
  episodes,
  onChanged,
  onAdded,
}: {
  work: MediaItem
  season: number
  episodes: Episode[]
  onChanged: () => void
  onAdded: (episode: number) => void
}) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const s = t.admin.editor.seasonsTab
  const next = episodes.length ? Math.max(...episodes.map((episode) => episode.episodeNumber)) + 1 : 1
  const [number, setNumber] = useState(String(next))
  const [title, setTitle] = useState('')
  const [airDate, setAirDate] = useState('')
  const [runtime, setRuntime] = useState('')
  const [absolute, setAbsolute] = useState('')
  const [image, setImage] = useState('')
  const [overview, setOverview] = useState('')

  // The next free number follows the season shown.
  useEffect(() => {
    setNumber(String(next))
  }, [season, next])

  const add = useMutation({
    mutationFn: (body: unknown) => api.post<{ id: string }>(`/items/${work.id}/episodes`, body),
    onSuccess: (_, body) => {
      setTitle('')
      setAirDate('')
      setRuntime('')
      setAbsolute('')
      setImage('')
      setOverview('')
      onChanged()
      onAdded((body as { episodeNumber: number }).episodeNumber)
    },
  })

  const name = seasonName(work.seasons?.find((x) => x.seasonNumber === season)?.title, season, t.work.season)

  return (
    <AddForm
      label={s.addEpisodeTo(name)}
      hint={<Count>{s.nextCode(episodeCode({ seasonNumber: season, episodeNumber: next }))}</Count>}
      pending={add.isPending}
      error={add.error}
      onSubmit={() =>
        add.mutate({
          seasonNumber: season,
          episodeNumber: Number(number),
          title: title || undefined,
          airDate: airDate || undefined,
          runtime: runtime ? Number(runtime) : undefined,
          absoluteEpisodeNumber: absolute ? Number(absolute) : undefined,
          image: image || undefined,
          overview: overview || undefined,
        })
      }
    >
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <FormField label={c.episodeNumber} htmlFor="episode-number">
          <Input id="episode-number" required inputMode="numeric" value={number} onChange={(event) => setNumber(event.target.value.replace(/\D/g, ''))} />
        </FormField>
        <FormField label={c.airDate} htmlFor="episode-air-date">
          <Input id="episode-air-date" type="date" value={airDate} onChange={(event) => setAirDate(event.target.value)} />
        </FormField>
        <FormField label={s.runtime} htmlFor="episode-runtime">
          <Input id="episode-runtime" inputMode="numeric" value={runtime} onChange={(event) => setRuntime(event.target.value.replace(/\D/g, ''))} />
        </FormField>
        <FormField label={s.absolute} htmlFor="episode-absolute">
          <Input id="episode-absolute" inputMode="numeric" value={absolute} onChange={(event) => setAbsolute(event.target.value.replace(/\D/g, ''))} />
        </FormField>
      </div>
      <FormField label={c.episodeTitle} htmlFor="episode-title">
        <Input id="episode-title" value={title} onChange={(event) => setTitle(event.target.value)} />
      </FormField>
      <FormField label={s.stillUrl} htmlFor="episode-image">
        <Input id="episode-image" type="url" placeholder="https://…" value={image} onChange={(event) => setImage(event.target.value)} />
      </FormField>
      <FormField label={s.overview} htmlFor="episode-overview">
        <Textarea id="episode-overview" rows={3} value={overview} onChange={(event) => setOverview(event.target.value)} />
      </FormField>
    </AddForm>
  )
}
