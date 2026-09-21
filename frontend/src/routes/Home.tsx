/**
 * The way in.
 *
 * A catalogue's front page should show the catalogue, not explain it: one work
 * given the room a poster deserves, then rows of everything else. The featured
 * work is whichever the providers rank highest, which is as close to an opinion
 * as this server is willing to have.
 */

import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'

import { PosterCard, PosterGrid, Score } from '../components/media'
import { EmptyState, Genre, Glyph, Label, SectionTitle, Skeleton } from '../components/ui'
import { api, query } from '../lib/api'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import { backdrop, headlineRating, poster } from '../lib/media'
import type { ItemPage, MediaItem } from '../lib/types'

export function Home() {
  const { t, lang } = useI18n()

  const series = useQuery({
    queryKey: ['home', 'series', lang],
    queryFn: () => api.get<ItemPage>(`/items${query({ kind: 'series', limit: 12, language: lang })}`),
  })

  const films = useQuery({
    queryKey: ['home', 'movie', lang],
    queryFn: () => api.get<ItemPage>(`/items${query({ kind: 'movie', limit: 12, language: lang })}`),
  })

  const loading = series.isPending || films.isPending
  const everything = [...(series.data?.items ?? []), ...(films.data?.items ?? [])]
  const featured = everything.find((item) => backdrop(item)) ?? everything[0]

  if (loading) {
    return <HomeSkeleton />
  }

  if (!everything.length) {
    return (
      <div className="pt-16">
        <EmptyState title={t.home.empty} hint={t.home.emptyHint} />
      </div>
    )
  }

  return (
    <div className="pb-8">
      {featured ? <Featured item={featured} /> : null}

      <div className="mt-16 space-y-16">
        {series.data?.items.length ? (
          <Row
            title={t.home.allSeries}
            to="/browse?kind=series"
            items={series.data.items}
            total={series.data.total}
          />
        ) : null}

        {films.data?.items.length ? (
          <Row
            title={t.home.allFilms}
            to="/browse?kind=movie"
            items={films.data.items}
            total={films.data.total}
          />
        ) : null}
      </div>
    </div>
  )
}

/**
 * The work at the top.
 *
 * The backdrop is laid edge to edge and cut with a hard gradient rather than a
 * blur, so the text sits on ink instead of on a smeared image — cheaper to
 * paint and far easier to read.
 */
function Featured({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const art = backdrop(item)
  const sheet = poster(item)
  const rating = headlineRating(item.ratings)

  return (
    <section className="relative mx-[calc(50%-50vw)] w-screen">
      <div className="relative h-[19rem] overflow-hidden sm:h-[24rem] lg:h-[28rem]">
        {art ? (
          <img
            src={art}
            alt=""
            aria-hidden
            fetchPriority="high"
            className="fade-in size-full object-cover object-top"
          />
        ) : (
          <div className="size-full bg-ink-raised" />
        )}

        {/* Solid where the text sits, then a flat wash the whole way across —
            not a fade to clear. A gradient that lifts near the right edge puts
            body copy on whatever the photograph happens to be doing there, and
            on a bright frame that is unreadable. The backdrop is texture. */}
        <div className="absolute inset-0 bg-[linear-gradient(to_right,var(--color-ink)_0,var(--color-ink)_22%,color-mix(in_srgb,var(--color-ink)_86%,transparent)_52%,color-mix(in_srgb,var(--color-ink)_86%,transparent)_100%)]" />
        <div className="absolute inset-x-0 bottom-0 h-32 bg-gradient-to-t from-ink to-transparent" />
      </div>

      <div className="absolute inset-0">
        <div className="mx-auto flex h-full max-w-7xl items-end px-4 pb-8 sm:px-6">
          <div className="flex items-end gap-5">
            {sheet ? (
              <img
                src={sheet}
                alt={t.a11y.poster(item.title)}
                className="strike hidden w-28 rounded-plate border border-rule-bright shadow-[var(--shadow-plate)] sm:block lg:w-36"
              />
            ) : null}

            <div className="strike max-w-2xl" style={{ animationDelay: '80ms' }}>
              <Label>{item.kind === 'series' ? t.nav.series : t.nav.films}</Label>

              <h1 className="mt-2 font-display text-3xl leading-[1.05] font-medium text-bone sm:text-4xl lg:text-5xl">
                {item.title}
              </h1>

              <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2">
                {item.year ? (
                  <span className="font-mono text-sm text-bone-dim tabular-nums">{item.year}</span>
                ) : null}
                {fmt.runtime(item.runtime, locale) ? (
                  <span className="text-sm text-bone-dim">{fmt.runtime(item.runtime, locale)}</span>
                ) : null}
                {item.genres.slice(0, 3).map((genre) => (
                  <Genre key={genre} name={genre} />
                ))}
                {rating?.value ? <Score value={rating.value} size="sm" /> : null}
              </div>

              {item.overview ? (
                <p className="mt-4 line-clamp-2 max-w-xl text-sm leading-relaxed text-bone-dim sm:line-clamp-3">
                  {item.overview}
                </p>
              ) : null}

              <Link
                to={`/work/${item.id}`}
                className="mt-5 inline-flex min-h-11 items-center gap-2 rounded-full bg-[linear-gradient(135deg,var(--color-vermillion),color-mix(in_srgb,var(--color-vermillion)_70%,var(--color-brass)))] px-5 text-sm font-medium text-ink shadow-[var(--shadow-lift)] transition-[filter] duration-200 hover:brightness-110"
              >
                {t.work.details}
                <Glyph name="chevronRight" className="size-4" />
              </Link>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}

function Row({
  title,
  to,
  items,
  total,
}: {
  title: string
  to: string
  items: MediaItem[]
  total: number
}) {
  const { t, locale } = useI18n()

  return (
    <section>
      <SectionTitle
        action={
          <Link
            to={to}
            className="flex min-h-11 items-center gap-1.5 text-sm text-bone-dim transition-colors duration-200 hover:text-vermillion"
          >
            {t.home.seeAll}
            <span className="font-mono text-xs text-bone-faint tabular-nums">
              {fmt.count(total, locale)}
            </span>
            <Glyph name="chevronRight" className="size-3.5" />
          </Link>
        }
      >
        {title}
      </SectionTitle>

      <PosterGrid>
        {items.map((item) => (
          <PosterCard key={item.id} item={item} to={`/work/${item.id}`} />
        ))}
      </PosterGrid>
    </section>
  )
}

function HomeSkeleton() {
  return (
    <div className="pb-8">
      <div className="mx-[calc(50%-50vw)] h-[19rem] w-screen sm:h-[24rem] lg:h-[28rem]">
        <Skeleton className="size-full rounded-none" />
      </div>
      <div className="mt-16 space-y-4">
        <Skeleton className="h-7 w-40" />
        <PosterGrid>
          {Array.from({ length: 6 }, (_, index) => (
            <div key={index} className="space-y-2.5">
              <Skeleton className="aspect-2/3 w-full" />
              <Skeleton className="h-4 w-3/4" />
            </div>
          ))}
        </PosterGrid>
      </div>
    </div>
  )
}
