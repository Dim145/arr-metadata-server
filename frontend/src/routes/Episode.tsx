/**
 * One episode, as its index card.
 *
 * The still at the size it deserves, the code a release name carries, and the
 * moment it aired — in the reader's own timezone, since that is when it was on
 * for them, with UTC beside it for anyone comparing against a release log.
 * Everything else a provider said about the episode follows, then the way to
 * the ones either side of it.
 */

import { Link, useParams } from 'react-router'

import { Elsewhere, ExternalLink, Trail } from '../components/elsewhere'
import { Placement, finaleLabel } from '../components/episodes'
import { Artwork, Score } from '../components/media'
import { Chip, Field, Glyph, Panel, PanelHead, SectionTitle, Skeleton } from '../components/ui'
import { ApiError } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useMe, useTitle, useWork } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { providerName } from '../lib/labels'
import { episodeLinks } from '../lib/links'
import { adjacentEpisodes, airTime, airValue, airsWhen, episodeCode, hasAired, seasonName } from '../lib/media'
import type { Episode as EpisodeData, MediaItem } from '../lib/types'
import { NotFound, Unavailable } from './NotFound'

export function Episode() {
  const { id = '', season = '', episode = '' } = useParams()
  const work = useWork(id)

  // Kept on screen through a refetch that failed; see the work's page.
  if (!work.data) {
    if (work.isPending) return <EpisodeSkeleton />
    if (work.error instanceof ApiError && work.error.status === 404) return <NotFound work />
    return <Unavailable onRetry={() => void work.refetch()} />
  }

  const item = work.data
  const found = item.episodes?.find(
    (e) => e.seasonNumber === Number(season) && e.episodeNumber === Number(episode),
  )

  if (!found) return <NotFound />

  // Keyed by the episode: "next" reuses this page, and its images kept the
  // last episode's record of which had already fallen back to the original.
  return <Card key={found.id} item={item} episode={found} />
}

function Card({ item, episode }: { item: MediaItem; episode: EpisodeData }) {
  const { t, locale } = useI18n()
  const me = useMe()

  const seasonMeta = item.seasons?.find((s) => s.seasonNumber === episode.seasonNumber)
  const season = seasonName(seasonMeta?.title, episode.seasonNumber, t.work.season)
  const code = episodeCode(episode)
  // The day is the reader's own where the moment is known: a US evening
  // episode is on the next morning in Paris, and dating it the evening before
  // beside a Paris clock time put it on the wrong day.
  const time = airTime(episode)
  const shown = airValue(time)
  const upcoming = time !== undefined && !hasAired(time)
  const local = time?.moment ? fmt.clock(time.moment.toISOString(), locale) : undefined
  const utc = time?.moment
    ? new Intl.DateTimeFormat(locale, { hour: '2-digit', minute: '2-digit', timeZone: 'UTC' }).format(time.moment)
    : undefined
  const { before, after } = adjacentEpisodes(item, episode)
  const title = episode.title || t.episode.untitled(episode.episodeNumber)
  const links = episodeLinks(item, episode)
  useTitle(`${code} ${title}`, item.title)

  return (
    <article className="pt-8 pb-12">
      <Trail
        steps={[
          { label: t.nav.series, to: '/browse?kind=series' },
          { label: item.title, to: `/work/${item.id}` },
          { label: season, to: `/work/${item.id}/season/${episode.seasonNumber}` },
          { label: code },
        ]}
      />

      <header className="mt-4 grid gap-8 lg:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)] lg:items-end">
        <div className="rise relative aspect-video overflow-hidden rounded-plate border border-rule-bright bg-ink-high shadow-[var(--shadow-plate)]">
          {episode.image ? (
            <Artwork
              url={episode.image}
              role="frame"
              eager
              fetchPriority="high"
              alt={t.a11y.still(title)}
              className="fade-in size-full object-cover"
            />
          ) : (
            <div className="grid size-full place-items-center">
              <Glyph name="film" className="size-8 text-bone-faint" />
            </div>
          )}
          <span className="absolute bottom-3 left-3 rounded-card bg-ink/85 px-2 py-1 font-mono text-xs font-medium tracking-wider text-bone tabular-nums">
            {code}
          </span>
        </div>

        <div className="rise min-w-0" style={{ animationDelay: '70ms' }}>
          <p className="flex flex-wrap items-center gap-2">
            <Link
              to={`/work/${item.id}`}
              className="label inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
            >
              {item.title}
            </Link>
            <span className="label text-bone-faint" aria-hidden>
              ·
            </span>
            <span className="label">{season}</span>
          </p>

          <h1 className="font-display text-3xl leading-[1.1] font-medium text-bone sm:text-4xl">{title}</h1>

          <div className="mt-3 flex flex-wrap items-center gap-2">
            {episode.finaleType ? <Chip tone="accent">{finaleLabel(episode.finaleType, t)}</Chip> : null}
            {upcoming && time ? (
              <Chip tone="provider">
                <Glyph name="clock" className="size-3" />
                {airsWhen(time, locale)}
              </Chip>
            ) : null}
            {episode.isManual ? (
              <Chip tone="manual">
                <Glyph name="lock" className="size-3" />
                {t.work.manualEntry}
              </Chip>
            ) : null}
          </div>

          <p className="mt-4 text-sm text-bone-dim">
            {fmt.weekdayDate(shown, locale) ?? t.episode.noDate}
            {local ? (
              <>
                {' · '}
                <span className="text-bone">{local}</span>
                <span className="text-bone-faint"> {t.episode.yourTime}</span>
              </>
            ) : null}
          </p>

          <div className="mt-5 flex flex-wrap items-center gap-x-5 gap-y-3">
            {episode.rating?.value ? (
              <div className="flex items-center gap-3">
                <Score value={episode.rating.value} votes={episode.rating.votes} size="sm" />
                {episode.rating.votes ? (
                  <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                    {t.work.votes(fmt.count(episode.rating.votes, locale), episode.rating.votes)}
                  </span>
                ) : null}
              </div>
            ) : null}
            {fmt.runtime(episode.runtime, locale) ? (
              <span className="text-sm text-bone-dim">{fmt.runtime(episode.runtime, locale)}</span>
            ) : null}
            {episode.absoluteEpisodeNumber && episode.absoluteEpisodeNumber !== episode.episodeNumber ? (
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {t.episode.absolute(episode.absoluteEpisodeNumber)}
              </span>
            ) : null}
          </div>

          <Placement episode={episode} className="mt-3" />

          <div className="mt-5 flex flex-wrap items-center gap-3">
            <Elsewhere links={links} />
            {me.data?.canWrite ? (
              <Link
                to={`/admin/catalogue/${item.id}?season=${episode.seasonNumber}&episode=${episode.episodeNumber}`}
                className="inline-flex min-h-11 items-center gap-1.5 rounded-full border border-brass-deep px-3.5 text-[0.8125rem] font-medium text-brass transition-colors duration-150 hover:bg-brass/10"
              >
                <Glyph name="pencil" className="size-3.5" />
                {t.episode.edit}
              </Link>
            ) : null}
          </div>
        </div>
      </header>

      <div className="mt-12 grid gap-12 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <section className="min-w-0">
          <SectionTitle>{t.work.overview}</SectionTitle>
          {episode.overview ? (
            <p className="max-w-[68ch] text-[0.9375rem] leading-relaxed text-bone-dim">{episode.overview}</p>
          ) : (
            <p className="text-sm text-bone-faint italic">{t.episode.noOverview}</p>
          )}
        </section>

        <aside>
          <Panel>
            <PanelHead title={t.work.details} />
            <dl className="divide-y divide-rule">
              <Field label={t.episode.aired}>{fmt.longDate(shown, locale) ?? '—'}</Field>
              {local ? (
                <Field label={t.episode.time}>
                  <span className="tabular-nums">
                    {local}
                    <span className="text-bone-faint"> · {utc} UTC</span>
                  </span>
                </Field>
              ) : null}
              {item.network ? <Field label={t.work.network}>{item.network}</Field> : null}
              {episode.runtime ? <Field label={t.work.runtime}>{fmt.runtime(episode.runtime, locale)}</Field> : null}
              {links.map((link) => (
                <Field key={link.source} label={providerName(link.source)}>
                  <ExternalLink href={link.href} className="font-mono text-[0.8125rem] tabular-nums">
                    {link.source === 'tvdb' ? episode.tvdbId : episode.tmdbId}
                  </ExternalLink>
                </Field>
              ))}
            </dl>
          </Panel>
        </aside>
      </div>

      <nav aria-label={t.episode.around} className="mt-12 grid gap-4 border-t border-rule pt-8 sm:grid-cols-2">
        <Neighbour item={item} episode={before} direction="before" />
        <Neighbour item={item} episode={after} direction="after" />
      </nav>
    </article>
  )
}

function Neighbour({
  item,
  episode,
  direction,
}: {
  item: MediaItem
  episode?: EpisodeData
  direction: 'before' | 'after'
}) {
  const { t } = useI18n()

  if (!episode) return <span />

  return (
    <Link
      to={`/work/${item.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
      rel={direction === 'before' ? 'prev' : 'next'}
      className={cn(
        'group flex items-center gap-4 rounded-panel border border-rule bg-ink-raised p-3',
        'transition-colors duration-150 hover:border-rule-bright hover:bg-ink-high',
        direction === 'after' && 'sm:flex-row-reverse sm:text-right',
      )}
    >
      <div className="relative aspect-video w-28 shrink-0 overflow-hidden rounded-card bg-ink-high">
        {episode.image ? (
          <Artwork url={episode.image} role="still" alt="" className="size-full object-cover" />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="film" className="size-4 text-bone-faint" />
          </div>
        )}
      </div>
      <div className="min-w-0">
        <p className={cn('label flex items-center gap-1.5', direction === 'after' && 'sm:justify-end')}>
          {direction === 'before' ? <Glyph name="chevronLeft" className="size-3" /> : null}
          {direction === 'before' ? t.episode.previous : t.episode.next}
          {direction === 'after' ? <Glyph name="chevronRight" className="size-3" /> : null}
        </p>
        <p className="mt-1 font-mono text-[0.6875rem] text-bone-faint tabular-nums">{episodeCode(episode)}</p>
        <p className="truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
          {episode.title || t.episode.untitled(episode.episodeNumber)}
        </p>
      </div>
    </Link>
  )
}

function EpisodeSkeleton() {
  return (
    <div className="space-y-6 pt-8">
      <Skeleton className="h-4 w-72" />
      <div className="grid gap-8 lg:grid-cols-[1.35fr_1fr]">
        <Skeleton className="aspect-video w-full" />
        <div className="space-y-3 pt-10">
          <Skeleton className="h-10 w-3/4" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      </div>
    </div>
  )
}
