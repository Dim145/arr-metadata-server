/**
 * The chrome around the catalogue.
 *
 * A slim bar, a hairline, and the room below it. Nothing here floats or blurs:
 * a header that frosts what scrolls under it costs a compositor pass on every
 * frame, and this interface has to stay cheap on a machine whose job is
 * transcoding.
 */

import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { Fragment, useEffect, useState } from 'react'
import { Link, Outlet, useLocation, useNavigate, useSearchParams } from 'react-router'

import { api, query } from '../lib/api'
import { useSettled } from '../lib/debounce'
import { feeds, webcal } from '../lib/feeds'
import { useMe, useNavigationReset } from '../lib/hooks'
import { cn } from '../lib/cn'
import { LANGS, LANGUAGES, useI18n } from '../lib/i18n'
import { ThemeToggle } from './ThemeToggle'
import { providerName } from '../lib/labels'
import { poster } from '../lib/media'
import type { ItemPage, Me, Sources } from '../lib/types'
import { CommandPalette, openPalette, paletteShortcut } from './CommandPalette'
import { Artwork } from './media'
import { Glyph, Input } from './ui'

export function PublicShell({ me }: { me?: Me }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [searching, setSearching] = useState(false)
  const location = useLocation()
  useNavigationReset('main')

  // A menu that survives navigation would cover the page it just opened.
  useEffect(() => {
    setOpen(false)
    setSearching(false)
  }, [location.pathname, location.search])

  // `/` puts the cursor in the search, as it does on every site with one,
  // unless it is being typed into a field.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return
      const target = event.target as HTMLElement | null
      if (target?.closest('input, textarea, select, [contenteditable="true"]')) return
      event.preventDefault()
      const box = document.getElementById('catalogue-search') as HTMLInputElement | null
      if (box && box.offsetParent !== null) {
        box.focus()
        box.select()
      } else {
        setSearching(true)
        setOpen(false)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    // `clip` rather than `hidden`: the season scrollers deliberately bleed past
    // the container's right edge so a half-visible card shows there is more, and
    // without containment that bleed makes the whole page scroll sideways.
    // `hidden` would contain it too, but it turns this element into a scroll
    // container and the sticky header stops sticking.
    <div className="grain ambience min-h-dvh overflow-x-clip">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-50 focus:rounded-card focus:bg-vermillion focus:px-4 focus:py-2 focus:text-sm focus:font-medium focus:text-ink"
      >
        {t.nav.skipToContent}
      </a>

      <header className="sticky top-0 z-30 border-b border-rule bg-ink">
        <div className="mx-auto flex h-16 max-w-7xl items-center gap-3 px-4 sm:px-6 lg:gap-4">
          <Wordmark />

          <nav aria-label={t.nav.browse} className="hidden items-center gap-0.5 md:flex lg:gap-1">
            <Tab to="/browse">{t.nav.browse}</Tab>
            <Tab to="/browse?kind=series">{t.nav.series}</Tab>
            <Tab to="/browse?kind=movie">{t.nav.films}</Tab>
            <Tab to="/calendar">{t.nav.calendar}</Tab>
            <Tab to="/seasons" section>
              {t.nav.seasons}
            </Tab>
            {/* A sixth tab is one too many between a tablet's width and a
                laptop's; there the selections are a footer link away. */}
            <div className="hidden lg:contents">
              <Tab to="/lists" section>
                {t.nav.lists}
              </Tab>
            </div>
          </nav>

          <div className="ml-auto flex items-center gap-2">
            <SearchBox className="hidden w-64 lg:block" />
            <button
              type="button"
              onClick={openPalette}
              aria-label={t.palette.open}
              title={t.palette.open}
              className="hidden min-h-11 items-center rounded-full border border-rule px-2.5 font-mono text-[0.6875rem] text-bone-faint transition-colors duration-150 hover:border-rule-bright hover:text-bone lg:inline-flex"
            >
              {paletteShortcut()}
            </button>

            {/* Below the width where the field fits, search is still one tap
                away rather than buried in the menu: it is the thing people
                come to a catalogue to do. */}
            <button
              type="button"
              onClick={() => {
                setSearching((was) => !was)
                setOpen(false)
              }}
              aria-expanded={searching}
              aria-label={t.nav.search}
              className="grid size-11 place-items-center rounded-card text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-bone lg:hidden"
            >
              <Glyph name={searching ? 'close' : 'search'} className="size-5" />
            </button>

            {/* In the bar from `md` up; below it they move into the menu. On a
                phone the logo, search, language, account and menu buttons are
                wider than the screen together — at 320px the menu button sat
                past the edge, clipped away, and the navigation behind it could
                not be opened at all. */}
            <div className="hidden items-center gap-2 md:flex">
              {/* On a tablet the bar is full; the lamp is in the menu there. */}
              <ThemeToggle className="hidden lg:flex" />
              <LanguageToggle />
              <AdminLink me={me} />
            </div>

            <button
              type="button"
              onClick={() => {
                setOpen((was) => !was)
                setSearching(false)
              }}
              aria-expanded={open}
              aria-label={open ? t.nav.close : t.nav.menu}
              className="grid size-11 place-items-center rounded-card text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-bone md:hidden"
            >
              <Glyph name={open ? 'close' : 'menu'} className="size-5" />
            </button>
          </div>
        </div>

        {searching ? (
          <div className="border-t border-rule bg-ink-raised px-4 py-3 lg:hidden">
            <SearchBox id="catalogue-search-bar" autoFocus />
          </div>
        ) : null}

        {open ? (
          <div className="border-t border-rule bg-ink-raised px-4 py-4 md:hidden">
            <nav aria-label={t.nav.browse} className="grid gap-1">
              <Tab to="/browse" block>
                {t.nav.browse}
              </Tab>
              <Tab to="/browse?kind=series" block>
                {t.nav.series}
              </Tab>
              <Tab to="/browse?kind=movie" block>
                {t.nav.films}
              </Tab>
              <Tab to="/calendar" block>
                {t.nav.calendar}
              </Tab>
              <Tab to="/lists" block section>
                {t.nav.lists}
              </Tab>
              <Tab to="/seasons" block section>
                {t.nav.seasons}
              </Tab>
            </nav>

            <div className="mt-3 flex flex-wrap items-center justify-between gap-3 border-t border-rule pt-3">
              <ThemeToggle />
              <LanguageToggle />
              <AdminLink me={me} labelled />
            </div>
          </div>
        ) : null}
      </header>

      <main id="main" tabIndex={-1} className="relative z-10 mx-auto max-w-7xl px-4 pb-24 outline-none sm:px-6">
        <Outlet />
        <CommandPalette admin={Boolean(me?.canWrite)} />
      </main>

      <Footer />
    </div>
  )
}

function Wordmark() {
  const { t } = useI18n()

  return (
    <Link to="/" className="group flex min-h-11 shrink-0 items-center gap-2">
      {/* The mark: a projector aperture, drawn once. */}
      <span className="font-display text-xl leading-none font-medium tracking-tight text-bone transition-colors duration-200 group-hover:text-vermillion">
        {t.brand.name}
      </span>
      <span
        aria-hidden
        className="hidden h-3 w-px bg-rule-bright lg:block"
      />
      <span className="label hidden lg:block">arr</span>
    </Link>
  )
}

/**
 * A section tab.
 *
 * `NavLink` decides what is active from the path alone, and these three tabs
 * differ only by a query parameter — so on `/browse?kind=series` it lit both
 * Series and Films. The comparison has to include the parameter, which means
 * making it here rather than letting the router guess.
 */
function Tab({
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
        // Closer between `md` and `lg`, where five of these share the bar with
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

/**
 * The search, with what it finds as it is typed.
 *
 * A catalogue is opened to find one work, and the first letters of its title
 * are enough: what they match is listed under the box as they are typed — a
 * poster, a title, a year — the arrow keys walk it, Enter opens the one under
 * the cursor, and the whole list is the last line. Pressing Enter to see a
 * page of results, then choosing from it, was a detour on every visit.
 *
 * ARIA's combobox, so a screen reader hears the count and the option under
 * the cursor; the options are links, so the page is the same one a click and
 * a keyboard open.
 */
function SearchBox({
  id = 'catalogue-search',
  className,
  autoFocus,
}: {
  id?: string
  className?: string
  autoFocus?: boolean
}) {
  const { t, lang } = useI18n()
  const navigate = useNavigate()
  const [params] = useSearchParams()
  const [term, setTerm] = useState(params.get('q') ?? '')
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(-1)
  const settled = useSettled(term.trim(), 200)
  const asked = settled.length >= 2

  // Landing on /browse?q=… from elsewhere should fill the box.
  useEffect(() => {
    setTerm(params.get('q') ?? '')
  }, [params])

  const found = useQuery({
    queryKey: ['search', settled, lang],
    queryFn: () => api.get<ItemPage>(`/items${query({ term: settled, limit: 6, language: lang })}`),
    enabled: asked,
    // The last list stays up while the next letter's is fetched: a list that
    // emptied between keystrokes flickered.
    placeholderData: keepPreviousData,
    staleTime: 60_000,
  })
  const items = asked ? (found.data?.items ?? []) : []
  const total = asked ? (found.data?.total ?? 0) : 0
  const listed = open && asked
  // The options, and the way to the whole list after them.
  const count = items.length ? items.length + 1 : 0
  const listing = `${id}-results`
  const optionId = (index: number) => `${id}-option-${index}`

  const browse = (trimmed: string) => (trimmed ? `/browse?q=${encodeURIComponent(trimmed)}` : '/browse')
  const go = (index: number) => {
    const trimmed = term.trim()
    navigate(index >= 0 && index < items.length ? `/work/${items[index]!.id}` : browse(trimmed))
    setOpen(false)
    setActive(-1)
  }

  return (
    <form
      role="search"
      className={cn('relative', className)}
      onSubmit={(event) => {
        event.preventDefault()
        go(active)
      }}
    >
      <label htmlFor={id} className="sr-only">
        {t.nav.search}
      </label>
      <Glyph
        name="search"
        className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
      />
      <Input
        id={id}
        autoFocus={autoFocus}
        type="text"
        role="combobox"
        aria-expanded={listed}
        aria-controls={listing}
        aria-autocomplete="list"
        aria-activedescendant={listed && active >= 0 ? optionId(active) : undefined}
        autoComplete="off"
        enterKeyHint="search"
        value={term}
        onChange={(event) => {
          setTerm(event.target.value)
          setActive(-1)
          setOpen(true)
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            if (!count) return
            event.preventDefault()
            setOpen(true)
            const step = event.key === 'ArrowDown' ? 1 : -1
            // Through none, then each option, and round: the states are one
            // more than the options.
            setActive((was) => (was + 1 + step + count + 1) % (count + 1) - 1)
          } else if (event.key === 'Escape' && listed) {
            event.preventDefault()
            setOpen(false)
            setActive(-1)
          }
        }}
        placeholder={t.nav.searchPlaceholder}
        className="pr-9 pl-9"
      />
      {term ? (
        <button
          type="button"
          onClick={() => {
            setTerm('')
            setActive(-1)
            document.getElementById(id)?.focus()
          }}
          aria-label={t.nav.close}
          className="absolute top-1/2 right-1 grid size-9 -translate-y-1/2 place-items-center rounded-card text-bone-faint transition-colors duration-150 hover:text-bone"
        >
          <Glyph name="close" className="size-3.5" />
        </button>
      ) : (
        <kbd
          aria-hidden
          title={t.nav.searchShortcut}
          className="pointer-events-none absolute top-1/2 right-3 hidden -translate-y-1/2 rounded-[4px] border border-rule px-1.5 font-mono text-[0.625rem] text-bone-faint lg:block"
        >
          /
        </kbd>
      )}

      {listed ? (
        <div
          id={listing}
          role="listbox"
          aria-label={t.nav.results}
          className="fade-in absolute inset-x-0 top-full z-40 mt-2 overflow-hidden rounded-panel border border-rule-bright bg-ink-raised shadow-[var(--shadow-plate)]"
        >
          {items.map((item, index) => (
            <Link
              key={item.id}
              id={optionId(index)}
              role="option"
              aria-selected={index === active}
              to={`/work/${item.id}`}
              // Pressing the mouse would take the focus, and the list with it,
              // before the click could land.
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => setOpen(false)}
              className={cn(
                'flex items-center gap-3 px-3 py-2 transition-colors duration-100',
                index === active ? 'bg-ink-high' : 'hover:bg-ink-high',
              )}
            >
              <span className="aspect-2/3 w-8 shrink-0 overflow-hidden rounded-[4px] bg-ink-high">
                {poster(item) ? (
                  <Artwork url={poster(item)!} role="thumb" sizes="32px" alt="" className="size-full object-cover" />
                ) : null}
              </span>
              <span className="min-w-0">
                <span className="block truncate text-sm font-medium text-bone">{item.title}</span>
                <span className="block font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                  {[item.year, item.kind === 'series' ? t.home.kindSeries : t.home.kindFilm].filter(Boolean).join(' · ')}
                </span>
              </span>
            </Link>
          ))}
          {items.length ? (
            <Link
              id={optionId(items.length)}
              role="option"
              aria-selected={active === items.length}
              to={browse(term.trim())}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => setOpen(false)}
              className={cn(
                'flex min-h-11 items-center justify-between gap-2 border-t border-rule px-3 text-sm transition-colors duration-100',
                active === items.length ? 'bg-ink-high text-bone' : 'text-bone-dim hover:bg-ink-high hover:text-bone',
              )}
            >
              {t.nav.allResults(total)}
              <Glyph name="chevronRight" className="size-3.5" />
            </Link>
          ) : (
            <p className="px-3 py-3 text-sm text-bone-faint">
              {found.isPending ? t.nav.searching : t.nav.noResults(settled)}
            </p>
          )}
        </div>
      ) : null}
      <p className="sr-only" aria-live="polite">
        {listed && found.isSuccess ? t.nav.resultsFound(items.length) : ''}
      </p>
    </form>
  )
}

function LanguageToggle() {
  const { lang, setLang, t } = useI18n()

  return (
    <div
      role="group"
      aria-label={t.nav.language}
      className="flex items-center rounded-card border border-rule"
    >
      {LANGS.map((code) => (
        <button
          key={code}
          type="button"
          onClick={() => setLang(code)}
          aria-pressed={lang === code}
          title={LANGUAGES[code].name}
          lang={code}
          className={cn(
            // Each half is a target in its own right, so each half gets the
            // full 44px rather than the pair sharing one.
            'flex min-h-11 min-w-11 items-center justify-center rounded-card px-2',
            'font-mono text-[0.6875rem] font-medium tracking-wider uppercase',
            'cursor-pointer transition-colors duration-200',
            lang === code ? 'bg-bone text-ink' : 'text-bone-faint hover:text-bone',
          )}
        >
          {code}
        </button>
      ))}
    </div>
  )
}

function AdminLink({ me, labelled = false }: { me?: Me; labelled?: boolean }) {
  const { t } = useI18n()
  const signedIn = Boolean(me?.canWrite)

  const label = signedIn ? t.nav.admin : t.nav.signIn

  return (
    <Link
      to={signedIn ? '/admin' : '/login'}
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
          was wider than an 820px tablet — this link cut off past the edge. */}
      <Glyph name={signedIn ? 'settings' : 'user'} className="size-4" />
      <span className={labelled ? undefined : 'hidden lg:inline'}>{label}</span>
    </Link>
  )
}

function Footer() {
  const { t, lang } = useI18n()
  const me = useMe()

  const sources = useQuery({
    queryKey: ['sources'],
    queryFn: () => api.get<Sources>('/sources'),
    staleTime: 10 * 60_000,
    // The page is whole without it; a failed request is not worth a second.
    retry: false,
  })

  return (
    <footer className="relative z-10 border-t border-rule">
      <div className="mx-auto flex max-w-7xl flex-col gap-6 px-4 py-8 sm:flex-row sm:items-start sm:justify-between sm:px-6">
        <div className="flex flex-col gap-1">
          <span className="font-display text-base text-bone-dim">{t.brand.name}</span>
          <span className="text-xs text-bone-faint">{t.brand.tagline}</span>
          <Link
            to="/lists"
            className="mt-1 inline-flex min-h-11 items-center text-xs text-bone-dim transition-colors duration-150 hover:text-bone"
          >
            {t.nav.lists}
          </Link>
          <Link
            to="/stats"
            className="inline-flex min-h-11 items-center text-xs text-bone-dim transition-colors duration-150 hover:text-bone"
          >
            {t.nav.figures}
          </Link>
        </div>
        {/* What a reader can follow from elsewhere: the schedule in a
            calendar app, the arrivals and the week in a feed reader. Only
            where the catalogue is open, since neither carries a credential. */}
        {me.data?.publicBrowse ? (
        <nav aria-label={t.feeds.label} className="flex flex-wrap items-center gap-x-4 text-xs text-bone-faint">
          <a
            href={webcal(feeds.calendar(lang))}
            title={t.feeds.calendarHint}
            className="inline-flex min-h-11 items-center gap-1.5 transition-colors duration-150 hover:text-bone"
          >
            <Glyph name="calendar" className="size-3.5" />
            {t.feeds.calendar}
          </a>
          <a
            href={feeds.added}
            type="application/atom+xml"
            className="inline-flex min-h-11 items-center gap-1.5 transition-colors duration-150 hover:text-bone"
          >
            <Glyph name="rss" className="size-3.5" />
            {t.feeds.added}
          </a>
          <a
            href={feeds.airing}
            type="application/atom+xml"
            className="inline-flex min-h-11 items-center gap-1.5 transition-colors duration-150 hover:text-bone"
          >
            <Glyph name="rss" className="size-3.5" />
            {t.feeds.airing}
          </a>
        </nav>
        ) : null}
        {sources.data?.sources.length ? <Credits active={sources.data.sources} /> : null}
      </div>
    </footer>
  )
}

/** Where each source lives, for the link its credit carries. */
const HOMES: Record<string, string> = {
  tmdb: 'https://www.themoviedb.org',
  tvdb: 'https://thetvdb.com',
  fanart: 'https://fanart.tv',
  tvmaze: 'https://www.tvmaze.com',
  anilist: 'https://anilist.co',
  mal: 'https://myanimelist.net',
  imdb: 'https://www.imdb.com',
}

/**
 * Who the data on these pages comes from.
 *
 * Owed, not decorative: TMDB's terms ask for their notice, TVmaze's licence
 * for a link, IMDb's for a line. Only what is switched on is named, so the
 * footer never credits a source that contributed nothing.
 */
function Credits({ active }: { active: string[] }) {
  const { t } = useI18n()
  const linked = active.filter((key) => HOMES[key])

  return (
    <div className="max-w-xl text-xs leading-relaxed text-bone-faint sm:text-right">
      <p>
        {t.credits.lead}{' '}
        {linked.map((key, index) => (
          <Fragment key={key}>
            {/* The dot rides with the name before it, so a line never starts
                with one. */}
            <span className="whitespace-nowrap">
              <a
                href={HOMES[key]}
                target="_blank"
                rel="noreferrer"
                className="text-bone-dim underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-bone"
              >
                {providerName(key)}
              </a>
              {index < linked.length - 1 ? <span aria-hidden> ·</span> : null}
            </span>{' '}
          </Fragment>
        ))}
      </p>
      {active.includes('tmdb') ? (
        <p lang="en" className="mt-1">
          {t.credits.tmdb}
        </p>
      ) : null}
      {active.includes('imdb') ? (
        <p lang="en" className="mt-1">
          {t.credits.imdb}
        </p>
      ) : null}
    </div>
  )
}
