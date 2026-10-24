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
 *
 * On a phone the tabs are not up there at all: they are the bar along the foot
 * of the screen, where a thumb rests, and what does not fit in its four places
 * is in the sheet its fifth opens.
 */

import { useQuery } from '@tanstack/react-query'
import { useEffect, useId, useRef, useState } from 'react'
import { Link, NavLink, useLocation } from 'react-router'

import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import { useHasLists } from '../lib/hooks'
import { useI18n, type Dict } from '../lib/i18n'
import { genreLabel } from '../lib/labels'
import type { Facets, Me } from '../lib/types'
import { LanguageToggle } from './LanguageToggle'
import { SignOut } from './SignOut'
import { ThemeToggle } from './ThemeToggle'
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
  section,
}: {
  to: string
  children: React.ReactNode
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
    // eslint-disable-next-line jsx-a11y/no-static-element-interactions -- the group only listens to its controls: a mouse over them, Escape, the focus leaving
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
        if (!group.current?.contains(event.relatedTarget)) setOpen(false)
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
                <Row to="/people" glyph="user">
                  {t.people.title}
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

/* ── The bar along the foot of a phone ────────────────────────────────────── */

/**
 * The places that earn a tab. Four, and a fifth that holds the rest: a bar of
 * more is a bar of targets too narrow for a thumb, and the admin side's bar
 * keeps to five for the same reason.
 */
const PLACES: { to: string; glyph: GlyphName; label: (t: Dict) => string; end?: boolean }[] = [
  { to: '/', glyph: 'home', label: (t) => t.nav.home, end: true },
  { to: '/browse', glyph: 'grid', label: (t) => t.nav.browse },
  { to: '/calendar', glyph: 'calendar', label: (t) => t.nav.calendar },
  { to: '/seasons', glyph: 'seasons', label: (t) => t.nav.seasons },
]

/** What the sheet leads to, so that its tab can say the reader is in one of them. */
const SHEET_PATHS = ['/collections', '/people', '/lists', '/stats', '/account']

/**
 * One cell of the bar: tall enough for a thumb, the glyph above its word.
 * The word is 11px on the narrowest phones, where "Programme" is all a fifth
 * of 320px has room for, and 12px from 360 up. It is never cut short: should
 * a language's word be wider than its cell, it bleeds evenly into the gaps on
 * either side, which a clipped "Programm…" would not be forgiven for.
 */
const CELL =
  'relative flex min-h-14 min-w-0 flex-col items-center justify-center gap-1 px-0.5 transition-colors duration-150'
const CAPTION =
  'text-center text-[0.6875rem] leading-tight font-medium tracking-tight whitespace-nowrap min-[360px]:text-xs'

/** The rule along the top of the tab the reader is in: a shape, not a colour alone. */
function Mark({ on }: { on: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        'absolute inset-x-4 top-0 h-0.5 rounded-full bg-vermillion transition-opacity duration-150',
        on ? 'opacity-100' : 'opacity-0',
      )}
    />
  )
}

/**
 * The tabs of a phone: along the foot of the screen, where a thumb rests,
 * instead of behind a button at the top that took two gestures to change
 * section. From `md` up the bar at the top has them and this is not drawn.
 *
 * Accueil, Parcourir, Programme, Saisons — and Plus, which opens a sheet with
 * what the four leave out: the collections, the selections, the figures, the
 * lamp, the language, the account. The same shape as the administration's
 * bar, so that both sides of the site read alike under a thumb.
 */
export function TabBar({ me }: { me?: Me }) {
  const { t } = useI18n()
  const { pathname } = useLocation()
  const [open, setOpen] = useState(false)
  const sheet = useId()
  const inSheet = SHEET_PATHS.some((path) => pathname === path || pathname.startsWith(`${path}/`))

  // A sheet that survives navigation would cover the page it just opened.
  useEffect(() => {
    setOpen(false)
  }, [pathname])

  return (
    <>
      <nav
        aria-label={t.nav.mainNavigation}
        // Named for the view transitions, as the bar at the top is: it holds
        // still while the page beneath it crosses over.
        style={{ viewTransitionName: 'tabbar' }}
        className="fixed inset-x-0 bottom-0 z-40 grid grid-cols-5 border-t border-rule bg-ink-raised pb-[env(safe-area-inset-bottom)] md:hidden"
      >
        {PLACES.map((place) => (
          <NavLink
            key={place.to}
            to={place.to}
            end={place.end}
            className={({ isActive }) => cn(CELL, isActive ? 'text-vermillion' : 'text-bone-faint hover:text-bone')}
          >
            {({ isActive }) => (
              <>
                <Mark on={isActive} />
                <Glyph name={place.glyph} className="size-5" />
                <span className={CAPTION}>{place.label(t)}</span>
              </>
            )}
          </NavLink>
        ))}
        <button
          type="button"
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-controls={sheet}
          // A button is no page to be "the current page": the sheet's places
          // are, and this says one of them is.
          aria-current={inSheet ? 'true' : undefined}
          onClick={() => setOpen(true)}
          className={cn(CELL, 'cursor-pointer', inSheet || open ? 'text-vermillion' : 'text-bone-faint hover:text-bone')}
        >
          <Mark on={inSheet} />
          <Glyph name="more" className="size-5" />
          <span className={CAPTION}>{t.nav.more}</span>
        </button>
      </nav>

      <MoreSheet id={sheet} open={open} onClose={() => setOpen(false)} me={me} />
    </>
  )
}

/**
 * What the four tabs leave out, in a sheet that comes up from the bar.
 *
 * A native `<dialog>`, as the palette is: the browser brings the focus trap,
 * the Escape key and an inert page behind it, and being in the top layer it
 * stands over the bar it was opened from.
 */
function MoreSheet({ id, open, onClose, me }: { id: string; open: boolean; onClose: () => void; me?: Me }) {
  const { t } = useI18n()
  // The selections are offered only while there is one to show.
  const hasLists = useHasLists()
  const ref = useRef<HTMLDialogElement>(null)
  const signedIn = Boolean(me?.canWrite || me?.user)

  useEffect(() => {
    const node = ref.current
    if (!node) return

    if (open && !node.open) node.showModal()
    if (!open && node.open) node.close()
  }, [open])

  // The page behind holds still while the sheet is up: an inert page cannot
  // be pressed, but a finger dragging on the sheet can still move it.
  useEffect(() => {
    if (!open) return
    const overflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      document.body.style.overflow = overflow
    }
  }, [open])

  const places: { to: string; glyph: GlyphName; label: string }[] = [
    { to: '/collections', glyph: 'reel', label: t.collections.label },
    { to: '/people', glyph: 'user', label: t.people.title },
    ...(hasLists ? [{ to: '/lists', glyph: 'list' as const, label: t.nav.lists }] : []),
    { to: '/stats', glyph: 'gauge', label: t.nav.figures },
  ]

  return (
    <dialog
      ref={ref}
      id={id}
      aria-labelledby={`${id}-title`}
      // Escape, which the browser handles, and whatever else closes a dialog.
      onClose={onClose}
      onMouseDown={(event) => {
        // The dark around the sheet: the dialog itself is what a press there lands on.
        if (event.target === event.currentTarget) onClose()
      }}
      className="mx-0 mt-auto mb-0 max-h-[85dvh] w-full max-w-none overflow-hidden rounded-t-plate border border-b-0 border-rule bg-ink-raised p-0 text-bone backdrop:bg-ink/70"
    >
      {open ? (
        <div className="sheet-in flex max-h-[85dvh] flex-col">
          <header className="flex shrink-0 items-center justify-between border-b border-rule py-1 pr-1 pl-5">
            <h2 id={`${id}-title`} className="font-display text-lg font-medium text-bone">
              {t.nav.more}
            </h2>
            <button
              type="button"
              onClick={onClose}
              aria-label={t.nav.close}
              className="grid size-11 cursor-pointer place-items-center rounded-card text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
            >
              <Glyph name="close" className="size-5" />
            </button>
          </header>

          <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain pb-[max(0.75rem,env(safe-area-inset-bottom))]">
            <ul className="py-1">
              {places.map((place) => (
                <li key={place.to}>
                  <NavLink
                    to={place.to}
                    onClick={onClose}
                    className={({ isActive }) =>
                      cn(
                        'flex min-h-12 items-center gap-3 px-5 text-base font-medium transition-colors duration-150',
                        isActive ? 'text-vermillion' : 'text-bone hover:bg-ink-high',
                      )
                    }
                  >
                    <Glyph name={place.glyph} className="size-5 shrink-0 text-bone-faint" />
                    <span className="flex-1">{place.label}</span>
                    <Glyph name="chevronRight" className="size-4 shrink-0 text-bone-faint" />
                  </NavLink>
                </li>
              ))}
            </ul>

            <div className="border-t border-rule px-5 py-2">
              <div className="flex min-h-14 items-center justify-between gap-3">
                <span className="label">{t.nav.appearance}</span>
                <ThemeToggle />
              </div>
              <div className="flex min-h-14 items-center justify-between gap-3">
                <span className="label">{t.nav.language}</span>
                <LanguageToggle />
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2 border-t border-rule px-5 pt-3">
              <AdminLink me={me} labelled />
              {signedIn ? (
                <SignOut className="flex min-h-11 items-center gap-2 rounded-card px-3 text-sm font-medium text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-vermillion" />
              ) : null}
            </div>
          </div>
        </div>
      ) : null}
    </dialog>
  )
}

/* ── Controls the bar at the top and the sheet share ──────────────────────── */

export function AdminLink({ me, labelled = false }: { me?: Me; labelled?: boolean }) {
  const { t } = useI18n()
  const signedIn = Boolean(me?.canWrite)
  // A member is signed in too, with no administration to go to: their account.
  const member = !signedIn && Boolean(me?.user)

  const label = signedIn ? t.nav.admin : member ? t.nav.account : t.nav.signIn

  return (
    <Link
      to={signedIn ? '/admin' : member ? '/account' : '/login'}
      aria-label={label}
      className={cn(
        'flex min-h-11 items-center gap-2 rounded-card px-3 text-sm font-medium',
        'transition-colors duration-200',
        signedIn
          ? 'border border-vermillion-deep text-vermillion hover:bg-vermillion hover:text-ink'
          : 'text-bone-dim hover:bg-ink-high hover:text-bone',
      )}
    >
      {/* The word from `lg` up only: between `md` and `lg` the bar holds
          the tabs, the search, the language and this, and with the word it
          was wider than an 820px tablet — this link cut off past the edge.
          For somebody signed in it waits for `xl`: the bar then holds the way
          out beside it too, and a member's "My account" is the longest word
          it carries — at 1024px it was this, and the way out, past the edge. */}
      <Glyph name={signedIn ? 'settings' : 'user'} className="size-4" />
      <span className={labelled ? undefined : signedIn || member ? 'hidden xl:inline' : 'hidden lg:inline'}>{label}</span>
    </Link>
  )
}
