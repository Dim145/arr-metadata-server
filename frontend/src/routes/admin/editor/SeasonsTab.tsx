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
import { Button, Chip, FormField, Glyph, Input, Panel, PanelHead, Provenance, Segmented, Textarea } from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import * as fmt from '../../../lib/format'
import { useOrders } from '../../../lib/hooks'
import { useI18n } from '../../../lib/i18n'
import { providerName } from '../../../lib/labels'
import { episodeLinks } from '../../../lib/links'
import { airTime, airValue, episodeCode, episodesOf, seasonName, seasonNumbers, seasonPoster } from '../../../lib/media'
import type { Episode, FieldRegistry, MediaItem, Override, ProvenanceReport } from '../../../lib/types'

import { FieldRow, hasValue } from './FieldRow'
import { AddForm, Count, Empty, Facts, LockBadge, useRemove } from './shared'

type Filter = 'all' | 'locked' | 'yours'

const scopeOf = (episode: Pick<Episode, 'seasonNumber' | 'episodeNumber'>) => `episode:${episode.seasonNumber}x${episode.episodeNumber}`

/** Where a special belongs: a question only a special is asked, unless an answer is already held. */
const PLACEMENT = new Set(['airedAfterSeasonNumber', 'airedBeforeSeasonNumber', 'airedBeforeEpisodeNumber'])

export function SeasonsTab({
  work,
  registry,
  overrides,
  report,
  onChanged,
}: {
  work: MediaItem
  registry: FieldRegistry
  overrides: Override[]
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
  const orders = useOrders(work.id)
  const { ask, dialog } = useRemove(work, onChanged)

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
        {
          value: 'locked',
          label: (
            <span className="inline-flex items-center gap-1.5">
              {e.filter.locked}
              <Count>{counts.locked}</Count>
            </span>
          ),
        },
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
      {/* ── The seasons ────────────────────────────────────────────── */}
      <nav aria-label={c.seasons} id="seasons" className="scroll-mt-28 lg:sticky lg:top-16 lg:scroll-mt-16">
        {numbers.length ? (
        <ol
          className={cn(
            '-mx-4 flex gap-2 overflow-x-auto px-4 pb-1 [scrollbar-width:none] sm:-mx-6 sm:px-6',
            'lg:mx-0 lg:block lg:space-y-0.5 lg:overflow-visible lg:rounded-panel lg:border lg:border-rule lg:p-2 lg:px-4 lg:film-strip-y',
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
            <div className="aspect-2/3 overflow-hidden rounded-card border border-rule-bright bg-ink-high shadow-[var(--shadow-lift)]">
              {sheet ? (
                <Artwork url={sheet} role="thumb" alt="" className="size-full object-cover" />
              ) : (
                <div className="grid size-full place-items-center">
                  <Glyph name="tv" className="size-5 text-bone-faint" />
                </div>
              )}
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
              {registry.season.map((def) => (
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
                  {s.otherOrders(fmt.list(otherOrders.map((order) => t.season.order[order.kind] ?? order.kind), locale))}{' '}
                  <Link to={`/work/${work.id}/season/${season}?order=${encodeURIComponent(otherOrders[0]!.kind)}`} className="text-vermillion underline-offset-4 hover:underline">
                    {s.seeOrders}
                  </Link>
                </>
              ) : null}{' '}
              {c.onlyManual}
            </span>
          </p>
        </Panel>
          </>
        )}
      </div>

      {dialog}
    </div>
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
