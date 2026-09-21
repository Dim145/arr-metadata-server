/**
 * The chrome around the catalogue.
 *
 * A slim bar, a hairline, and the room below it. Nothing here floats or blurs:
 * a header that frosts what scrolls under it costs a compositor pass on every
 * frame, and this interface has to stay cheap on a machine whose job is
 * transcoding.
 */

import { useEffect, useState } from 'react'
import { Link, Outlet, useLocation, useNavigate, useSearchParams } from 'react-router'

import { cn } from '../lib/cn'
import { useI18n, type Lang } from '../lib/i18n'
import type { Me } from '../lib/types'
import { Glyph, Input } from './ui'

export function PublicShell({ me }: { me?: Me }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [searching, setSearching] = useState(false)
  const location = useLocation()

  // A menu that survives navigation would cover the page it just opened.
  useEffect(() => {
    setOpen(false)
    setSearching(false)
  }, [location.pathname, location.search])

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
        <div className="mx-auto flex h-16 max-w-7xl items-center gap-4 px-4 sm:px-6">
          <Wordmark />

          <nav aria-label={t.nav.browse} className="hidden items-center gap-1 md:flex">
            <Tab to="/browse">{t.nav.browse}</Tab>
            <Tab to="/browse?kind=series">{t.nav.series}</Tab>
            <Tab to="/browse?kind=movie">{t.nav.films}</Tab>
          </nav>

          <div className="ml-auto flex items-center gap-2">
            <SearchBox className="hidden w-56 lg:block" />

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

            <LanguageToggle />
            <AdminLink me={me} />

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
            <SearchBox autoFocus />
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
            </nav>
          </div>
        ) : null}
      </header>

      <main id="main" className="relative z-10 mx-auto max-w-7xl px-4 pb-24 sm:px-6">
        <Outlet />
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
        className="hidden h-3 w-px bg-rule-bright sm:block"
      />
      <span className="label hidden sm:block">arr</span>
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
}: {
  to: string
  children: React.ReactNode
  block?: boolean
}) {
  const location = useLocation()

  const [path, search] = to.split('?')
  const wanted = new URLSearchParams(search).get('kind')
  const current = new URLSearchParams(location.search).get('kind')

  const isActive =
    location.pathname === path && (wanted ?? null) === (current ?? null)

  return (
    <Link
      to={to}
      aria-current={isActive ? 'page' : undefined}
      className={cn(
        'relative flex min-h-11 items-center rounded-full px-3.5 text-sm font-medium transition-colors duration-200',
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

function SearchBox({ className, autoFocus }: { className?: string; autoFocus?: boolean }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const [params] = useSearchParams()
  const [term, setTerm] = useState(params.get('q') ?? '')

  // Landing on /browse?q=… from elsewhere should fill the box.
  useEffect(() => {
    setTerm(params.get('q') ?? '')
  }, [params])

  return (
    <form
      role="search"
      className={cn('relative', className)}
      onSubmit={(event) => {
        event.preventDefault()
        const trimmed = term.trim()
        navigate(trimmed ? `/browse?q=${encodeURIComponent(trimmed)}` : '/browse')
      }}
    >
      <label htmlFor="catalogue-search" className="sr-only">
        {t.nav.search}
      </label>
      <Glyph
        name="search"
        className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
      />
      <Input
        id="catalogue-search"
        autoFocus={autoFocus}
        type="search"
        value={term}
        onChange={(event) => setTerm(event.target.value)}
        placeholder={t.nav.searchPlaceholder}
        className="pl-9"
      />
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
      {(['en', 'fr'] as Lang[]).map((code) => (
        <button
          key={code}
          type="button"
          onClick={() => setLang(code)}
          aria-pressed={lang === code}
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

function AdminLink({ me }: { me?: Me }) {
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
      <Glyph name={signedIn ? 'settings' : 'user'} className="size-4" />
      <span className="hidden sm:inline">{label}</span>
    </Link>
  )
}

function Footer() {
  const { t } = useI18n()

  return (
    <footer className="relative z-10 border-t border-rule">
      <div className="mx-auto flex max-w-7xl flex-col gap-1 px-4 py-8 sm:px-6">
        <span className="font-display text-base text-bone-dim">{t.brand.name}</span>
        <span className="text-xs text-bone-faint">{t.brand.tagline}</span>
      </div>
    </footer>
  )
}
