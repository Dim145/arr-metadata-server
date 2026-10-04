/**
 * The bar's tabs, and the way into the catalogue.
 *
 * One tab leads to the catalogue, and the shortcuts a reader would otherwise
 * take through it — the series and the films, the commonest genres of each,
 * what is new and what is rated best — hang from it as a panel. It opens when
 * the pointer rests on the tab, or from the small button beside it for a
 * finger or a keyboard, and closes on leaving, on Escape, and on going
 * anywhere. The tab stays a link: a click on the word is a click on the
 * catalogue.
 *
 * Not an ARIA menu, which promises arrow keys and a menuitem contract, but a
 * disclosure: a button that says whether the panel is shown, and links.
 */

import { useQuery } from '@tanstack/react-query'
import { useEffect, useId, useRef, useState } from 'react'
import { Link, useLocation } from 'react-router'

import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import { useHasLists } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { genreLabel } from '../lib/labels'
import type { Facets } from '../lib/types'
import { Glyph, Skeleton, type GlyphName } from './ui'
import { stockOf } from './ui/chips'

/**
 * A section tab.
 *
 * `NavLink` decides what is active from the path alone, and a tab that
 * differs from another only by a query parameter would light both — so the
 * comparison includes the parameter, and is made here rather than left to
 * the router.
 */
export function Tab({
  to,
  children,
  block,
  section,
}: {
  to: string
  children: React.ReactNode
  block?: boolean
  /** Current anywhere beneath it too: every season is the seasons' tab. */
  section?: boolean
}) {
  const location = useLocation()

  const [path, search] = to.split('?')
  const wanted = new URLSearchParams(search).get('kind')
  const current = new URLSearchParams(location.search).get('kind')

  const isActive = section
    ? location.pathname === path || location.pathname.startsWith(`${path}/`)
    : location.pathname === path && (wanted ?? null) === (current ?? null)

  return (
    <Link
      to={to}
      aria-current={isActive ? 'page' : undefined}
      className={cn(
        // Closer between `md` and `lg`, where the tabs share the bar with
        // everything else on a screen not much wider than they are.
        'relative flex min-h-11 items-center rounded-full px-2.5 text-sm font-medium transition-colors duration-200 lg:px-3.5',
        block ? 'w-full' : '',
        isActive ? 'text-bone' : 'text-bone-dim hover:bg-ink-high hover:text-bone',
      )}
    >
      {children}
      {/* The active mark is a rule, not a pill: a catalogue underlines. */}
      <span
        aria-hidden
        className={cn(
          'absolute inset-x-3.5 -bottom-0.5 h-0.5 origin-left rounded-full bg-vermillion transition-transform duration-300 ease-[var(--ease-out-soft)]',
          isActive ? 'scale-x-100' : 'scale-x-0',
        )}
      />
    </Link>
  )
}

/** How long the pointer rests on the tab before the panel opens. */
const HOVER_DELAY = 80
/** How long the panel stays after the pointer leaves it: a hand crossing the gap. */
const LEAVE_GRACE = 220
/** The genres shown for each kind: the commonest, the rest a filter away. */
const GENRES_SHOWN = 6

type Kind = 'series' | 'movie'

/** The commonest genres of one kind, asked for once the reader shows interest. */
function useKindFacets(kind: Kind, enabled: boolean) {
  return useQuery({
    queryKey: ['facets', 'menu', kind],
    queryFn: () => api.get<Facets>(`/facets${query({ kind })}`),
    enabled,
    staleTime: 10 * 60_000,
  })
}

/** The catalogue's tab, with the panel of shortcuts hanging from it. */
export function BrowseTab() {
  const { t } = useI18n()
  const hasLists = useHasLists()
  const location = useLocation()
  const id = useId()
  const [open, setOpen] = useState(false)
  // The genres are fetched at the first sign of interest — a pointer resting,
  // a focus — so they are there by the time the panel is, and never for a
  // reader who never opens it.
  const [armed, setArmed] = useState(false)
  const group = useRef<HTMLDivElement>(null)
  const chevron = useRef<HTMLButtonElement>(null)
  const timer = useRef<number | undefined>(undefined)

  const later = (what: () => void, after: number) => {
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(what, after)
  }
  const show = () => {
    setArmed(true)
    later(() => setOpen(true), HOVER_DELAY)
  }
  const hide = () => later(() => setOpen(false), LEAVE_GRACE)
  const toggle = () => {
    window.clearTimeout(timer.current)
    setArmed(true)
    setOpen((was) => !was)
  }

  // Gone with the page it opened from.
  useEffect(() => {
    setOpen(false)
  }, [location.pathname, location.search])
  useEffect(() => () => window.clearTimeout(timer.current), [])

  // A press anywhere else closes it: a finger has no way to leave.
  useEffect(() => {
    if (!open) return
    const onPointerDown = (event: PointerEvent) => {
      if (!group.current?.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('pointerdown', onPointerDown)
    return () => document.removeEventListener('pointerdown', onPointerDown)
  }, [open])

  const series = useKindFacets('series', armed)
  const movies = useKindFacets('movie', armed)

  return (
    <div
      ref={group}
      className="relative"
      onPointerEnter={(event) => {
        if (event.pointerType === 'mouse') show()
      }}
      onPointerLeave={(event) => {
        if (event.pointerType === 'mouse') hide()
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && open) {
          event.preventDefault()
          setOpen(false)
          chevron.current?.focus()
        }
      }}
      onBlur={(event) => {
        if (!group.current?.contains(event.relatedTarget as Node | null)) setOpen(false)
      }}
    >
      <div className="flex items-center">
        <Tab to="/browse" section>
          {t.nav.browse}
        </Tab>
        <button
          ref={chevron}
          type="button"
          onClick={toggle}
          onFocus={() => setArmed(true)}
          aria-expanded={open}
          aria-controls={id}
          aria-label={t.nav.browseMenu}
          className="-ml-2 grid size-11 shrink-0 place-items-center rounded-full text-bone-faint transition-colors duration-200 hover:text-bone"
        >
          <Glyph
            name="chevronDown"
            className={cn('size-3.5 transition-transform duration-200', open && 'rotate-180')}
          />
        </button>
      </div>

      {open ? (
        <div
          id={id}
          role="group"
          aria-label={t.nav.browse}
          className={cn(
            'absolute top-full left-0 z-40 mt-1 w-[36rem] rounded-panel border border-rule bg-ink-raised p-4 shadow-[var(--shadow-plate)] lg:w-[44rem] lg:p-5',
            'origin-top motion-safe:animate-[rise_220ms_var(--ease-out-soft)_both]',
          )}
        >
          {/* Each as wide as its words and wrapping where the panel is not
              wide enough for all four: cut short, "Les plus po…" says nothing. */}
          <ul className="flex flex-wrap gap-1">
            {(
              [
                ['/browse', 'discover', t.nav.quick.all],
                ['/browse?order=added', 'plus', t.nav.quick.added],
                ['/browse?order=rated', 'star', t.nav.quick.rated],
                ['/browse?order=popular', 'reel', t.nav.quick.popular],
              ] as [string, GlyphName, string][]
            ).map(([to, glyph, label]) => (
              <li key={to}>
                <Link
                  to={to}
                  className="flex min-h-11 items-center gap-2 rounded-card px-2.5 text-sm font-medium text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
                >
                  <Glyph name={glyph} className="size-4 shrink-0 text-bone-faint" />
                  <span className="whitespace-nowrap">{label}</span>
                </Link>
              </li>
            ))}
          </ul>

          <div className="mt-3 grid grid-cols-3 gap-4 border-t border-rule pt-3 lg:gap-6">
            <GenreColumn kind="series" title={t.nav.series} whole={t.nav.allSeries} facets={series.data} />
            <GenreColumn kind="movie" title={t.nav.films} whole={t.nav.allFilms} facets={movies.data} />
            <div>
              <span className="label block min-h-11 leading-[2.75rem]">{t.nav.also}</span>
              <ul className="space-y-0.5">
                <Row to="/collections" glyph="reel">
                  {t.collections.label}
                </Row>
                {hasLists ? (
                  <Row to="/lists" glyph="list">
                    {t.nav.lists}
                  </Row>
                ) : null}
                <Row to="/stats" glyph="gauge">
                  {t.nav.figures}
                </Row>
              </ul>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}

/** One kind's column: its name leads to all of it, its genres to a part. */
function GenreColumn({
  kind,
  title,
  whole,
  facets,
}: {
  kind: Kind
  title: string
  whole: string
  facets?: Facets
}) {
  const { lang, locale } = useI18n()
  const genres = facets?.genres.slice(0, GENRES_SHOWN)

  return (
    <div className="min-w-0">
      <Link
        to={`/browse?kind=${kind}`}
        className="label flex min-h-11 items-center gap-1.5 rounded-card transition-colors duration-150 hover:text-vermillion"
      >
        <Glyph name={kind === 'series' ? 'tv' : 'film'} className="size-3.5" />
        {title}
      </Link>
      <ul className="space-y-0.5">
        {genres
          ? genres.map((genre) => (
              <li key={genre.value}>
                <Link
                  to={`/browse?kind=${kind}&genre=${encodeURIComponent(genre.value)}`}
                  className="flex min-h-11 items-center gap-2 rounded-card px-2 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
                >
                  {/* The genre's own tint, as its chip wears it. */}
                  <span
                    aria-hidden
                    className="size-1.5 shrink-0 rounded-full"
                    style={{ backgroundColor: `var(--color-stock-${stockOf(genre.value)})` }}
                  />
                  <span className="min-w-0 flex-1 truncate">{genreLabel(genre.value, lang)}</span>
                  <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                    {genre.count.toLocaleString(locale)}
                  </span>
                </Link>
              </li>
            ))
          : Array.from({ length: GENRES_SHOWN }, (_, index) => (
              <li key={index} className="flex min-h-11 items-center px-2">
                <Skeleton className="h-3 w-full" />
              </li>
            ))}
      </ul>
      <Link
        to={`/browse?kind=${kind}`}
        className="mt-1 flex min-h-11 items-center gap-1 px-2 text-xs font-medium text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
      >
        {whole}
        <Glyph name="chevronRight" className="size-3" />
      </Link>
    </div>
  )
}

function Row({ to, glyph, children }: { to: string; glyph: GlyphName; children: React.ReactNode }) {
  return (
    <li>
      <Link
        to={to}
        className="flex min-h-11 items-center gap-2 rounded-card px-2 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
      >
        <Glyph name={glyph} className="size-4 shrink-0 text-bone-faint" />
        {children}
      </Link>
    </li>
  )
}
