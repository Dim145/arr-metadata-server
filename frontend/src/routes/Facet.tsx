/**
 * A page of the catalogue: a genre's, a network's or a studio's, a year's or a
 * decade's.
 *
 * Its name over a band in the genre's own colour, how many works it holds,
 * the programmer's note on it in brass, the five rated highest, then every
 * one of them in the catalogue's own list — narrowed, ordered and laid out as
 * that list is, with the page's subject pinned to it.
 *
 * Addressed by a slug the server answers to under any of its names:
 * `/genre/drame` is the drama page, and its address becomes `/genre/drama`
 * once the server has said so; `/decade/1994` becomes `/decade/1990`.
 */

import { useQuery } from '@tanstack/react-query'
import { type CSSProperties, useEffect } from 'react'
import { Link, useLocation, useNavigate, useParams } from 'react-router'

import { PosterCard, PosterShelf } from '../components/media'
import { ProgrammersNote } from '../components/ProgrammersNote'
import { EmptyState, Glyph, Label, SectionTitle, Skeleton, stockOf } from '../components/ui'
import { ApiError, api, query } from '../lib/api'
import { cn } from '../lib/cn'
import { type FacetKind, decadePath, facetApiPath, yearPath } from '../lib/facets'
import { useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import type { FacetPage } from '../lib/types'
import { Browse } from './Browse'

export function Facet({ kind }: { kind: FacetKind }) {
  const { slug = '' } = useParams()
  const { t, lang } = useI18n()
  const navigate = useNavigate()
  const location = useLocation()

  const page = useQuery({
    queryKey: ['facet', kind, slug, lang],
    queryFn: () => api.get<FacetPage>(`${facetApiPath(kind, slug)}${query({ language: lang })}`),
    staleTime: 60_000,
  })

  // The address the server calls it by, once it has said: a genre named
  // another way, a decade by another of its years. The list's own filters
  // in the address stay as they are.
  const canonical = page.data?.slug
  useEffect(() => {
    if (canonical && canonical !== slug) {
      navigate({ pathname: `/${kind}/${canonical}`, search: location.search }, { replace: true })
    }
  }, [canonical, slug, kind, navigate, location.search])

  const name = page.data ? title(kind, page.data, t) : undefined
  useTitle(name)

  if (page.isPending) {
    return <FacetSkeleton />
  }

  if (page.isError || !page.data) {
    const gone = page.error instanceof ApiError && page.error.status === 404
    return (
      <div className="pt-16">
        <EmptyState
          kind={gone ? 'empty' : 'error'}
          title={gone ? t.facet.notFound : t.common.error}
          hint={gone ? t.facet.notFoundHint : t.facet.loadFailed}
          action={
            <div className="mt-2 flex flex-wrap justify-center gap-2">
              {gone ? null : (
                <button
                  type="button"
                  onClick={() => void page.refetch()}
                  className="inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-card border border-rule-bright px-4 text-sm text-bone transition-colors duration-200 hover:bg-ink-high"
                >
                  <Glyph name="refresh" className="size-4" />
                  {t.common.retry}
                </button>
              )}
              <Link
                to="/browse"
                className="inline-flex min-h-11 items-center gap-2 rounded-card px-4 text-sm text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-bone"
              >
                <Glyph name="arrowLeft" className="size-4" />
                {t.facet.toCatalogue}
              </Link>
            </div>
          }
        />
      </div>
    )
  }

  const data = page.data
  return (
    <div className="pt-8">
      <Header kind={kind} page={data} name={name ?? data.name} />

      <ProgrammersNote scope={kind} id={data.slug} note={data.note ?? null} className="mt-8" />

      {data.best.length ? (
        <section aria-label={t.facet.best} className="mt-12">
          <SectionTitle>{t.facet.best}</SectionTitle>
          <PosterShelf label={t.facet.best}>
            {data.best.map((item) => (
              <PosterCard key={item.id} item={item} to={`/work/${item.id}`} kind />
            ))}
          </PosterShelf>
        </section>
      ) : null}

      <section aria-labelledby="facet-all" className="mt-14">
        <Browse
          pinned={data.filter}
          lead={
            <h2 id="facet-all" className="font-display text-2xl font-medium text-bone sm:text-3xl">
              {t.facet.all}
            </h2>
          }
        />
      </section>
    </div>
  )
}

/** What a page is called, as the interface says it: a decade as "the 1990s". */
function title(kind: FacetKind, page: FacetPage, t: ReturnType<typeof useI18n>['t']): string {
  return kind === 'decade' ? t.facet.decadeName(Number(page.name)) : page.name
}

/**
 * The page's heading: what it is, its name, how many works it holds, and for
 * a year or a decade the way to the next and the one before. A genre's band is
 * washed in the genre's own colour, the one its chips wear, from the same
 * name; anything else is plain ink.
 */
function Header({ kind, page, name }: { kind: FacetKind; page: FacetPage; name: string }) {
  const { t } = useI18n()
  const names = page.names ?? {}
  const stock = kind === 'genre' ? stockOf(names.en ?? names.und ?? page.name) : undefined
  const tint: CSSProperties | undefined = stock
    ? {
        borderColor: `color-mix(in srgb, var(--color-stock-${stock}) 40%, transparent)`,
        backgroundImage: `linear-gradient(135deg, color-mix(in srgb, var(--color-stock-${stock}) 20%, transparent), transparent 72%)`,
      }
    : undefined
  const year = Number(page.slug)

  return (
    <header
      style={tint}
      className={cn(
        'rounded-plate border px-5 py-8 sm:px-8 sm:py-10',
        stock ? 'forced-colors:border-[color:CanvasText]' : 'border-rule bg-ink-raised',
      )}
    >
      <div className="flex items-center gap-2">
        {stock ? (
          <span aria-hidden className="size-2 rounded-full" style={{ backgroundColor: `var(--color-stock-${stock})` }} />
        ) : null}
        <Label>{t.facet.kinds[kind]}</Label>
      </div>
      <h1 className="mt-3 font-display text-4xl leading-[1.05] font-medium text-bone sm:text-5xl lg:text-6xl">{name}</h1>
      <p className="mt-4 font-mono text-sm text-bone-dim tabular-nums">
        {page.count ? t.facet.works(page.count) : t.facet.none}
      </p>

      {kind === 'year' || kind === 'decade' ? (
        <nav aria-label={kind === 'year' ? t.facet.around : t.facet.aroundDecades} className="mt-6 flex flex-wrap items-center gap-2">
          <Step to={kind === 'year' ? yearPath(year - 1) : decadePath(year - 10)} back>
            {kind === 'year' ? String(year - 1) : t.facet.decadeName(year - 10)}
          </Step>
          <Step to={kind === 'year' ? yearPath(year + 1) : decadePath(year + 10)}>
            {kind === 'year' ? String(year + 1) : t.facet.decadeName(year + 10)}
          </Step>
          {kind === 'year' ? (
            <Link
              to={decadePath(year)}
              className="inline-flex min-h-9 items-center rounded-full px-3 text-body-sm text-bone-dim underline decoration-rule-bright underline-offset-4 transition-colors duration-150 hover:text-bone"
            >
              {t.facet.decadeName(year - (((year % 10) + 10) % 10))}
            </Link>
          ) : null}
        </nav>
      ) : null}
    </header>
  )
}

/** The year or the decade before, or after. */
function Step({ to, back = false, children }: { to: string; back?: boolean; children: string }) {
  return (
    <Link
      to={to}
      className="inline-flex min-h-9 items-center gap-1.5 rounded-full border border-rule-bright px-3 font-mono text-body-sm text-bone tabular-nums transition-colors duration-150 hover:border-bone-faint hover:bg-ink-high"
    >
      {back ? <Glyph name="chevronLeft" className="size-3.5" /> : null}
      {children}
      {back ? null : <Glyph name="chevronRight" className="size-3.5" />}
    </Link>
  )
}

function FacetSkeleton() {
  return (
    <div aria-hidden className="pt-8">
      <div className="rounded-plate border border-rule px-5 py-8 sm:px-8 sm:py-10">
        <Skeleton className="h-3 w-16" />
        <Skeleton className="mt-4 h-12 w-2/3 max-w-md" />
        <Skeleton className="mt-5 h-4 w-40" />
      </div>
      <div className="mt-12 grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5">
        {Array.from({ length: 5 }, (_, index) => (
          <Skeleton key={index} className="aspect-2/3 w-full" />
        ))}
      </div>
    </div>
  )
}

/** A genre's page, its route's own. */
export function GenrePage() {
  return <Facet kind="genre" />
}

/** A network's or a studio's page. */
export function NetworkPage() {
  return <Facet kind="network" />
}

/** A year's page. */
export function YearPage() {
  return <Facet kind="year" />
}

/** A decade's page. */
export function DecadePage() {
  return <Facet kind="decade" />
}
