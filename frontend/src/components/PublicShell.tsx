/**
 * The chrome around the catalogue.
 *
 * A slim bar, a hairline, and the room below it. Nothing here floats or blurs:
 * a header that frosts what scrolls under it costs a compositor pass on every
 * frame, and this interface has to stay cheap on a machine whose job is
 * transcoding.
 *
 * The one thing that does float is the bar along the foot of a phone — fixed,
 * since a thumb reaches the bottom of a screen and not its top — and the page
 * keeps room under its own end for it: nothing, the footer included, is left
 * beneath it. A page whose own actions are what the thumb is there for — a
 * work's: where to watch it, its trailer, its address — stands its own bar
 * there instead (`useFootBar`), and the page keeps no room for the site's.
 */

import { keepPreviousData, useQuery, useQueryClient } from '@tanstack/react-query'
import { createContext, Fragment, use, useEffect, useId, useLayoutEffect, useState } from 'react'
import { Link, Outlet, useLocation, useNavigate, useSearchParams } from 'react-router'

import { useAdultPosters } from '../lib/adultPosters'
import { api, query } from '../lib/api'
import { useSettled } from '../lib/debounce'
import { feeds, webcal } from '../lib/feeds'
import { useHasLists, useMe, useNavigationReset } from '../lib/hooks'
import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { ThemeToggle } from './ThemeToggle'
import { LanguageToggle } from './LanguageToggle'
import { AdminLink, BrowseTab, Tab, TabBar } from './nav'
import { lookup, providerName } from '../lib/labels'
import { poster } from '../lib/media'
import { TEXT_LANGUAGES, setTextLanguage, useTextLanguage } from '../lib/prefs'
import type { ItemPage, Me, Sources } from '../lib/types'
import { CommandPalette, paletteShortcut } from './CommandPalette'
import { KeysHelp, useShortcuts } from './KeysHelp'
import { Artwork, VEILED, useVeiled } from './media'
import { PageBoundary } from './PageBoundary'
import { SignOut } from './SignOut'
import { Glyph, Input, Menu, Select, Toggle } from './ui'

/** Told by a page that it stands its own bar along the foot of a phone. */
const FootBar = createContext<((footed: boolean) => void) | null>(null)

/**
 * For a page that stands a bar of its own along the foot of a phone, in place
 * of the site's: said before the first paint, so the two are never drawn at
 * once, and unsaid as the page goes.
 */
export function useFootBar() {
  const say = use(FootBar)
  useLayoutEffect(() => {
    if (!say) return
    say(true)
    return () => say(false)
  }, [say])
}

export function PublicShell({ me }: { me?: Me }) {
  const { t } = useI18n()
  // The selections are offered only while there is one to show.
  const hasLists = useHasLists()
  // Signed in, whatever the account may do: a member has a way out too.
  const signedIn = Boolean(me?.canWrite || me?.user)
  const [searching, setSearching] = useState(false)
  const [keys, setKeys] = useState(false)
  // Whether the page has a bar of its own along the foot of a phone.
  const [footed, setFooted] = useState(false)
  const location = useLocation()
  useNavigationReset('main')

  // A search that survives navigation would cover the page it just opened.
  useEffect(() => {
    setSearching(false)
  }, [location.pathname, location.search])

  // `/` puts the cursor in the search, as it does on every site with one —
  // the field in the bar where the width has it, the one under the bar where
  // it has not — and the other keys go where they say; see `KeysHelp`.
  useShortcuts({
    search: () => {
      const box = document.getElementById('catalogue-search') as HTMLInputElement | null
      const under = document.getElementById('catalogue-search-bar') as HTMLInputElement | null
      const field = box && box.offsetParent !== null ? box : under
      if (field) {
        field.focus()
        field.select()
      } else {
        setSearching(true)
      }
    },
    help: () => setKeys(true),
  })

  return (
    // `clip` rather than `hidden`: the season scrollers deliberately bleed past
    // the container's right edge so a half-visible card shows there is more, and
    // without containment that bleed makes the whole page scroll sideways.
    // `hidden` would contain it too, but it turns this element into a scroll
    // container and the sticky header stops sticking.
    //
    // Under a phone, room for the bar along its foot: a cell is 3.5rem and a
    // hairline, and a phone with a home indicator adds its own inset. None for
    // a page that stands its own bar there, in the flow of the page.
    <div
      className={cn(
        'grain ambience min-h-dvh overflow-x-clip',
        !footed && 'pb-[calc(3.625rem+env(safe-area-inset-bottom))] md:pb-0',
      )}
    >
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-50 focus:rounded-card focus:bg-vermillion focus:px-4 focus:py-2 focus:text-sm focus:font-medium focus:text-ink"
      >
        {t.nav.skipToContent}
      </a>

      {/* Named for the view transitions: the bar holds still while the page
          beneath it crosses over. */}
      <header className="sticky top-0 z-30 border-b border-rule bg-ink" style={{ viewTransitionName: 'bar' }}>
        <div className="mx-auto flex h-16 max-w-7xl items-center gap-3 px-4 sm:px-6 lg:gap-4">
          <Wordmark />

          <nav aria-label={t.nav.browse} className="hidden items-center gap-0.5 md:flex lg:gap-1">
            {/* The catalogue, with its shortcuts hanging from the tab: the
                series and the films were tabs of their own, three ways to
                the same page across a bar that had no room for them. */}
            <BrowseTab />
            <Tab to="/calendar">{t.nav.calendar}</Tab>
            <Tab to="/seasons" section>
              {t.nav.seasons}
            </Tab>
            {/* A sixth tab is one too many between a tablet's width and a
                laptop's; there the selections are a footer link away. */}
            {hasLists ? (
              <div className="hidden lg:contents">
                <Tab to="/lists" section>
                  {t.nav.lists}
                </Tab>
              </div>
            ) : null}
          </nav>

          <div className="ml-auto flex items-center gap-2">
            {/* The palette's ⌘K is said in the field, beside its `/`: a
                button of its own beside the search was a second way into the
                same thing, and one more control in a full bar. The field
                waits for `xl`: between a laptop's narrowest and `xl`, beside
                five tabs, it pushed the way in past the edge of the screen. */}
            <SearchBox className="hidden w-64 xl:block" />

            {/* Below the width where the field fits, search is still one tap
                away, in the bar at the top with the title: it is the thing
                people come to a catalogue to do. */}
            <button
              type="button"
              onClick={() => setSearching((was) => !was)}
              aria-expanded={searching}
              aria-label={t.nav.search}
              aria-keyshortcuts="/"
              className="grid size-11 place-items-center rounded-card text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-bone xl:hidden"
            >
              <Glyph name={searching ? 'close' : 'search'} className="size-5" />
            </button>

            {/* On a tablet the bar is full: the lamp waits for `lg`. */}
            <ThemeToggle className="hidden lg:flex" />

            {/* The languages, and how the catalogue is shown, at every width:
                a phone's sheet has the interface's language, but not the
                texts' or the posters'. A menu rather than a pair of buttons:
                one control, where there were two halves of one. */}
            <ReaderMenu />

            {/* In the bar from `md` up; below it they are in the sheet the
                bar along the foot of the screen opens. On a phone the logo,
                search, language, account and a menu button were wider than
                the screen together — at 320px the menu button sat past the
                edge, clipped away, and the navigation behind it could not be
                opened at all. The top bar keeps the title and the search. */}
            <div className="hidden md:contents">
              <AdminLink me={me} />
              {signedIn ? (
                <SignOut
                  iconOnly
                  className="grid size-11 shrink-0 place-items-center rounded-card text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-vermillion"
                />
              ) : null}
            </div>
          </div>
        </div>

        {searching ? (
          <div className="border-t border-rule bg-ink-raised px-4 py-3 xl:hidden">
            <SearchBox id="catalogue-search-bar" autoFocus />
          </div>
        ) : null}
      </header>

      <main id="main" tabIndex={-1} className="relative z-10 mx-auto max-w-7xl px-4 pb-24 outline-none sm:px-6">
        {/* A page that throws is said so here, with the bar still where it was. */}
        <PageBoundary>
          <FootBar value={setFooted}>
            <Outlet />
          </FootBar>
        </PageBoundary>
        <CommandPalette canWrite={Boolean(me?.canWrite)} isAdmin={Boolean(me?.isAdmin)} />
        <KeysHelp open={keys} onClose={() => setKeys(false)} />
      </main>

      <Footer onKeys={() => setKeys(true)} />

      {footed ? null : <TabBar me={me} />}
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
 * How the catalogue is read here: the interface's language, the language of
 * its texts, and whether the posters of works for adults are shown — the one
 * of the three kept with the account of whoever is signed in, as their own
 * page has it too (see `adultPosters.ts`).
 *
 * Two languages, because they are two questions. The interface speaks the
 * few languages it is written in; the texts — titles, synopses, taglines —
 * are the providers', in any language they translate into. A reader of the
 * interface in English may want the films' own synopses in German.
 *
 * The site's own menu (`Menu`): a popover tied to its button, Escape and a
 * press elsewhere closing it — and, holding controls rather than actions, a
 * small dialog of them, each named.
 */
function ReaderMenu() {
  const { t, lang, locale } = useI18n()
  const queryClient = useQueryClient()
  const texts = useTextLanguage()
  const [adult, showAdult] = useAdultPosters()
  const id = useId()

  // Each language named in the interface's, and sorted as their names are,
  // not as their codes.
  const names = (() => {
    try {
      return new Intl.DisplayNames([locale], { type: 'language' })
    } catch {
      return undefined
    }
  })()
  const named = (tag: string) => {
    try {
      return names?.of(tag) ?? tag
    } catch {
      return tag
    }
  }
  const collator = new Intl.Collator(locale)
  const languages = TEXT_LANGUAGES.map((tag) => ({ tag, name: named(tag) })).sort((a, b) => collator.compare(a.name, b.name))

  return (
    <Menu
      // Named with the code it shows, so a voice that says "EN" finds it;
      // the language in full is the toggle's, inside.
      label={t.reader.menu(lang.toUpperCase())}
      title={t.reader.title}
      button={
        <>
          <Glyph name="globe" className="size-4" />
          <span className="font-mono text-caption font-medium tracking-wider uppercase">{lang}</span>
          <Glyph name="chevronDown" className="size-3 transition-transform duration-150" />
        </>
      }
      buttonClassName={(open) =>
        cn(
          'flex min-h-11 shrink-0 cursor-pointer items-center gap-1.5 rounded-card border border-rule px-2.5 text-bone-dim',
          'transition-colors duration-150 hover:border-rule-bright hover:text-bone',
          // The chevron turns while the panel is out.
          open && 'border-rule-bright text-bone [&>svg:last-of-type]:rotate-180',
        )
      }
      className="w-[min(20rem,calc(100vw-2rem))] p-4"
    >
      <p className="label">{t.reader.interface}</p>
      <div className="mt-2">
        <LanguageToggle />
      </div>

      <label htmlFor={`${id}-texts`} className="label mt-5 block">
        {t.reader.texts}
      </label>
      <Select
        id={`${id}-texts`}
        value={texts ?? ''}
        aria-describedby={`${id}-texts-hint`}
        onChange={(event) => {
          setTextLanguage(event.target.value || undefined)
          // Everything read so far was read in the other language: read
          // again, the page stays as it is until the new words arrive.
          void queryClient.invalidateQueries()
        }}
        className="mt-2"
      >
        <option value="">{t.reader.asInterface}</option>
        {languages.map(({ tag, name }) => (
          <option key={tag} value={tag}>
            {name}
          </option>
        ))}
      </Select>
      <p id={`${id}-texts-hint`} className="mt-1.5 text-caption leading-relaxed text-bone-faint">
        {t.reader.textsHint}
      </p>

      <div className="mt-5 flex items-center justify-between gap-3">
        <span className="text-sm text-bone">{t.reader.adult}</span>
        <Toggle checked={adult} label={t.reader.adult} onChange={showAdult} />
      </div>
      <p className="mt-1 text-caption leading-relaxed text-bone-faint">{t.reader.adultHint}</p>
    </Menu>
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
        aria-keyshortcuts="/"
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
        // Room on the right for the keys said there, from where they are.
        className="pr-9 pl-9 lg:pr-15"
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
        // The keys that lead here, said where the eye already is: `/` for
        // this field, the palette's for everything else. Said to a screen
        // reader by the field's own `aria-keyshortcuts` and the list `?`
        // opens; drawn only where a keyboard is likely, from `lg` up.
        <span
          aria-hidden
          className="pointer-events-none absolute top-1/2 right-2.5 hidden -translate-y-1/2 items-center gap-1 lg:flex"
        >
          {['/', paletteShortcut()].map((key) => (
            <kbd
              key={key}
              className="rounded-sm border border-rule px-1 font-mono text-caption leading-[1.125rem] text-bone-faint"
            >
              {key}
            </kbd>
          ))}
        </span>
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
              <span className="aspect-2/3 w-8 shrink-0 overflow-hidden rounded-sm bg-ink-high">
                {poster(item) ? <ResultThumb item={item} /> : null}
              </span>
              <span className="min-w-0">
                <span className="block truncate text-sm font-medium text-bone">{item.title}</span>
                <span className="block font-mono text-caption text-bone-faint tabular-nums">
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

/**
 * A found work's poster, the size of a thumbnail: veiled, as every poster of a
 * work for adults is, for a reader who has not asked to see them — too small
 * here to be uncovered, which its page is the place for.
 */
function ResultThumb({ item }: { item: ItemPage['items'][number] }) {
  const veiled = useVeiled(item)

  return (
    <Artwork
      url={poster(item)!}
      role="thumb"
      sizes="32px"
      alt=""
      className={cn('size-full object-cover', veiled && VEILED)}
    />
  )
}

function Footer({ onKeys }: { onKeys: () => void }) {
  const { t, lang } = useI18n()
  const hasLists = useHasLists()
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
          {hasLists ? (
            <Link
              to="/lists"
              className="mt-1 inline-flex min-h-11 items-center text-xs text-bone-dim transition-colors duration-150 hover:text-bone"
            >
              {t.nav.lists}
            </Link>
          ) : null}
          <Link
            to="/stats"
            className={cn(
              'inline-flex min-h-11 items-center text-xs text-bone-dim transition-colors duration-150 hover:text-bone',
              hasLists ? '' : 'mt-1',
            )}
          >
            {t.nav.figures}
          </Link>
          {/* Here rather than in the bar, which has no room between a tablet's
              width and a laptop's: a person is mostly reached from a work's
              cast, and the index of them all is a way in like the figures. */}
          <Link
            to="/people"
            className="inline-flex min-h-11 items-center text-xs text-bone-dim transition-colors duration-150 hover:text-bone"
          >
            {t.people.title}
          </Link>
          {/* The list of the keys, for whoever reads with a keyboard and has
              not met `?` yet: not drawn under a finger, which has none. */}
          <button
            type="button"
            onClick={onKeys}
            aria-keyshortcuts="?"
            className="hidden min-h-11 cursor-pointer items-center self-start text-xs text-bone-dim transition-colors duration-150 hover:text-bone pointer-fine:inline-flex"
          >
            {t.keys.title}
          </button>
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
  tenrai: 'https://tenrai.org',
  imdb: 'https://www.imdb.com',
  fankai: 'https://fankai.fr',
  fankaiwiki: 'https://fan-kai.fandom.com/fr/',
}

/** The licence TVmaze publishes its data under, which its credit names. */
const TVMAZE_LICENCE = 'https://creativecommons.org/licenses/by-sa/4.0/'

/** A source the labels do not name: a mirror, credited beside what it mirrors. */
const NAMED: Record<string, string> = { tenrai: 'Tenrai' }

const CREDIT_LINK = 'text-bone-dim underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-bone'

/**
 * Who the data on these pages comes from.
 *
 * Owed, not decorative: TMDB's terms ask for their notice, TheTVDB's for a
 * credit with a direct link, TVmaze's licence for a link and its name,
 * IMDb's for a line. Only what is switched on is named, so the footer never
 * credits a source that contributed nothing; Tenrai, which answers for
 * MyAnimeList while Jikan's public API is gone, is named beside it.
 */
function Credits({ active }: { active: string[] }) {
  const { t } = useI18n()
  const named = (key: string) => lookup(NAMED, key) ?? providerName(key)
  const link = (key: string, label: string) => (
    <a href={lookup(HOMES, key)} target="_blank" rel="noreferrer" className={CREDIT_LINK}>
      {label}
    </a>
  )
  const linked = active.filter((key) => key !== 'tenrai' && lookup(HOMES, key))

  return (
    <div className="max-w-xl text-xs leading-relaxed text-bone-faint sm:text-right">
      <p>
        {t.credits.lead}{' '}
        {linked.map((key, index) => (
          <Fragment key={key}>
            {/* The dot rides with the name before it, so a line never starts
                with one. */}
            <span className="whitespace-nowrap">
              {link(key, named(key))}
              {key === 'mal' && active.includes('tenrai') ? <> {t.credits.via} {link('tenrai', named('tenrai'))}</> : null}
              {key === 'tvmaze' ? (
                <>
                  {' '}
                  <a href={TVMAZE_LICENCE} target="_blank" rel="noreferrer" className={CREDIT_LINK}>
                    {t.credits.tvmaze}
                  </a>
                </>
              ) : null}
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
