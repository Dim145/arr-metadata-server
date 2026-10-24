/**
 * Everybody the catalogue credits, as an index.
 *
 * The back of the book: the names by their first letter — A to Z, and # for
 * those that begin with anything else — a letter nobody is filed under said
 * so rather than followed to an empty page; a job, for those who directed,
 * wrote or produced as well as those who acted; the most credited first, or
 * the names in their order. A name typed looks for them across the catalogue
 * instead, by any name they go by and a misspelling forgiven, as the palette
 * does.
 *
 * Every choice is in the address, as the catalogue's filters are, so a view
 * is a link and the back button returns to it. More of the index is asked
 * for at its foot, a page at a time from where the last ended, however long
 * it runs; the address keeps how much was shown, as far as one page can
 * bring back.
 */

import { keepPreviousData, useInfiniteQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Link, useLocation, useSearchParams } from 'react-router'

import { Artwork } from '../components/media'
import { Button, EmptyState, Glyph, Input, Label, Segmented, Select, Skeleton } from '../components/ui'
import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import { useSettled } from '../lib/debounce'
import * as fmt from '../lib/format'
import { useSearch, useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import type { PeoplePage, PersonCard, PersonJob } from '../lib/types'
import { JOBS } from './Person'

/** A page of the index: a few screens of cards. */
const PAGE = 60

/** The most one page of the index may hold, and so the most an address can bring back at once. */
const MOST = 200

/** The most a search finds of one kind. */
const FOUND = 20

/** The letters the index files names under, and the sign for every other beginning. */
const LETTERS = [...'ABCDEFGHIJKLMNOPQRSTUVWXYZ', '#']

/** The width a card's portrait is drawn at, column for column. */
const CARD_SIZES = '(min-width: 1280px) 140px, (min-width: 1024px) 15vw, (min-width: 768px) 18vw, (min-width: 640px) 23vw, 30vw'

type Sort = 'works' | 'name'

/** The index the address asks for, read once: anything it does not know is what saying nothing means. */
function read(params: URLSearchParams) {
  const letter = (params.get('letter') ?? '').trim().toUpperCase()
  const job: PersonJob | '' = JOBS.find((one) => one === params.get('job')) ?? ''
  const sort: Sort = params.get('sort') === 'name' ? 'name' : 'works'
  const shown = Math.trunc(Number(params.get('shown') ?? PAGE))
  return {
    letter: LETTERS.includes(letter) ? letter : '',
    job,
    sort,
    // Not `q`, which the bar's own search box reads as a title to look for.
    term: (params.get('term') ?? '').trim(),
    // Straight off the address, so it can be anything: kept to what one page holds.
    shown: Number.isFinite(shown) ? Math.min(Math.max(shown, PAGE), MOST) : PAGE,
  }
}

export function People() {
  const { t, locale } = useI18n()
  const [params, setParams] = useSearchParams()
  const { pathname } = useLocation()
  const f = read(params)
  const searching = f.term.length >= 2
  useTitle(searching ? t.browse.quoted(f.term) : t.people.title)

  const index = useInfiniteQuery({
    queryKey: ['people', 'index', f.letter, f.job, f.sort],
    queryFn: ({ pageParam }) =>
      api.get<PeoplePage>(
        `/people${query({ letter: f.letter, job: f.job, sort: f.sort === 'name' ? 'name' : '', limit: pageParam.limit, offset: pageParam.offset })}`,
      ),
    // As many as the address says were shown, the first time; then a page
    // at a time from where the last ended, counted in what is loaded.
    initialPageParam: { offset: 0, limit: f.shown },
    getNextPageParam: (last, pages) => {
      const loaded = pages.reduce((sum, page) => sum + page.people.length, 0)
      return loaded < last.total ? { offset: loaded, limit: PAGE } : undefined
    },
    placeholderData: keepPreviousData,
    enabled: !searching,
  })
  const found = useSearch(f.term, { kinds: ['person'], limit: FOUND, enabled: searching })

  /** The address with these changed and the rest kept; what is empty is left out. */
  const addressWith = (changes: Record<string, string>, from: URLSearchParams = params) => {
    const next = new URLSearchParams(from)
    for (const [name, value] of Object.entries(changes)) {
      if (value) next.set(name, value)
      else next.delete(name)
    }
    const text = next.toString()
    return text ? `?${text}` : ''
  }

  /** Write these into the address in place of the view, from the address as it is now rather than as this render saw it. */
  const write = (changes: Record<string, string>) =>
    setParams(new URLSearchParams(addressWith(changes, new URLSearchParams(window.location.search))), { replace: true })

  /** Change the view: a narrower or reordered index starts again from its head. */
  const change = (changes: Record<string, string>) => write({ shown: '', ...changes })

  // What is typed, settled into the address: a name a letter at a time asked
  // nobody anything until the typing paused. The address moving under the
  // field — back, a link — puts its own words in it.
  const [typed, setTyped] = useState(f.term)
  const [seen, setSeen] = useState(f.term)
  if (seen !== f.term) {
    setSeen(f.term)
    if (typed.trim() !== f.term) setTyped(f.term)
  }
  const settled = useSettled(typed.trim(), 250)
  useEffect(() => {
    if (settled !== (new URLSearchParams(window.location.search).get('term') ?? '').trim()) write({ term: settled })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `write` reads the address as it is when it runs: never stale
  }, [settled])

  const pages = index.data?.pages ?? []
  const people = pages.flatMap((page) => page.people)
  const total = pages[0]?.total ?? 0
  // Unknown until the first page is in: every letter is offered meanwhile.
  const letters = pages[0]?.letters
  const narrowed = Boolean(f.letter || f.job)
  const hits = found.data?.people ?? []

  const more = () => {
    void index.fetchNextPage()
    write({ shown: String(Math.min(people.length + PAGE, MOST)) })
  }

  // The way back from a search that found nobody goes with the search: the
  // field it came from takes the focus, for the next name.
  const clearSearch = () => {
    setTyped('')
    document.getElementById('people-search')?.focus()
  }

  return (
    <div className="pt-10 pb-12">
      <header className="mb-8">
        <Label>{t.people.label}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.people.title}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.people.lead}</p>
      </header>

      <div className="mb-6 space-y-5">
        <div className="max-w-md">
          <label htmlFor="people-search" className="label mb-1.5 block">
            {t.people.search}
          </label>
          <div className="relative">
            <Glyph
              name="search"
              className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
            />
            <Input
              id="people-search"
              type="search"
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
              aria-describedby="people-search-hint"
              autoComplete="off"
              autoCorrect="off"
              spellCheck={false}
              enterKeyHint="search"
              className="pl-9"
            />
          </div>
          <p id="people-search-hint" className="mt-1 text-xs text-bone-faint">
            {t.people.searchHint}
          </p>
        </div>

        {/* A search looks across everybody: the letter, the job and the
            order wait, as they were, for the index to come back. */}
        {searching ? null : (
          <>
            <nav aria-label={t.people.letters}>
              <ul className="flex flex-wrap gap-x-1 gap-y-2">
                <li>
                  <Link
                    to={{ pathname, search: addressWith({ letter: '', shown: '' }) }}
                    aria-current={f.letter ? undefined : 'page'}
                    className={cn(LETTER, 'px-3 font-sans', f.letter ? AVAILABLE : CURRENT)}
                  >
                    {t.people.everyLetter}
                  </Link>
                </li>
                {LETTERS.map((letter) => {
                  const current = f.letter === letter
                  const named =
                    letter === '#' ? (
                      <>
                        #<span className="sr-only"> {t.people.otherSigns}</span>
                      </>
                    ) : (
                      letter
                    )
                  return (
                    <li key={letter}>
                      {!letters || letters.includes(letter) || current ? (
                        <Link
                          to={{ pathname, search: addressWith({ letter, shown: '' }) }}
                          aria-current={current ? 'page' : undefined}
                          className={cn(LETTER, current ? CURRENT : AVAILABLE)}
                        >
                          {named}
                        </Link>
                      ) : (
                        // Nobody is filed under it. Said where the keyboard
                        // passes, rather than left out of the row or taken
                        // out of its order: a letter missing is information.
                        <span role="link" aria-disabled="true" tabIndex={0} className={cn(LETTER, UNAVAILABLE)}>
                          {named}
                        </span>
                      )}
                    </li>
                  )
                })}
              </ul>
            </nav>

            <div className="flex flex-wrap items-center gap-3">
              <Filter<PersonJob | ''>
                label={t.people.job}
                value={f.job}
                onChange={(job) => change({ job })}
                options={[
                  { value: '', label: t.people.anyJob },
                  ...JOBS.map((job) => ({ value: job, label: t.person.job[job] })),
                ]}
              />
              <Segmented<Sort>
                label={t.people.order}
                value={f.sort}
                onChange={(sort) => change({ sort: sort === 'name' ? 'name' : '' })}
                options={[
                  { value: 'works', label: t.people.orders.works },
                  { value: 'name', label: t.people.orders.name },
                ]}
              />
            </div>
          </>
        )}
      </div>

      {/* Laid out before the count is known, so that its arrival moves nothing. */}
      <p
        aria-live="polite"
        className={cn(
          'mb-5 font-mono text-sm text-bone-faint tabular-nums',
          !(searching ? found.isSuccess : index.isSuccess) && 'invisible',
        )}
      >
        {searching
          ? found.isSuccess
            ? t.people.found(hits.length)
            : ' '
          : index.isSuccess
            ? t.people.count(total)
            : ' '}
      </p>

      {searching ? (
        found.isError ? (
          <EmptyState
            kind="error"
            title={t.common.error}
            hint={t.people.loadFailed}
            action={<Retry onRetry={() => void found.refetch()} />}
          />
        ) : found.isPending ? (
          <Waiting />
        ) : hits.length === 0 ? (
          <EmptyState
            kind="filtered"
            title={t.people.nobody}
            hint={t.people.nobodyHint}
            action={
              <Button size="sm" variant="quiet" onClick={clearSearch}>
                {t.people.clearSearch}
              </Button>
            }
          />
        ) : (
          <>
            {/* Laid out as the index is, so that somebody looks the same found as listed. */}
            <ul
              aria-busy={(found.isFetching && found.isPlaceholderData) || undefined}
              className={cn('transition-opacity duration-150', GRID, found.isPlaceholderData && 'opacity-60')}
            >
              {hits.map((hit) => (
                <Card key={hit.id} person={hit} matched={hit.matched} />
              ))}
            </ul>
            {hits.length >= FOUND ? <p className="mt-6 text-sm text-bone-faint">{t.people.closest(hits.length)}</p> : null}
          </>
        )
      ) : index.isError ? (
        <EmptyState
          kind="error"
          title={t.common.error}
          hint={t.people.loadFailed}
          action={<Retry onRetry={() => void index.refetch()} />}
        />
      ) : index.isPending ? (
        <Waiting />
      ) : people.length === 0 ? (
        narrowed ? (
          <EmptyState
            kind="filtered"
            title={t.people.emptyFiltered}
            hint={t.people.emptyFilteredHint}
            action={
              <Button size="sm" onClick={() => change({ letter: '', job: '' })}>
                <Glyph name="close" className="size-3.5" />
                {t.people.clear}
              </Button>
            }
          />
        ) : (
          <EmptyState title={t.people.empty} hint={t.people.emptyHint} />
        )
      ) : (
        <>
          {/* The last view's cards stay, dimmed, while the next one's are
              on their way: an index that emptied at every letter flickered. */}
          <ul
            aria-busy={(index.isFetching && index.isPlaceholderData) || undefined}
            className={cn('stagger transition-opacity duration-150', GRID, index.isPlaceholderData && 'opacity-60')}
          >
            {people.map((person) => (
              <Card key={person.id} person={person} />
            ))}
          </ul>

          {index.hasNextPage ? (
            <div className="mt-12 flex flex-col items-center gap-2">
              <Button onClick={more} disabled={index.isFetchingNextPage}>
                {index.isFetchingNextPage ? t.browse.loadingMore : t.browse.loadMore}
              </Button>
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {fmt.count(people.length, locale)} {t.common.of} {fmt.count(total, locale)}
              </span>
            </div>
          ) : null}
        </>
      )}
    </div>
  )
}

/** The grid the cards stand in: three to a phone's width, eight to a wide screen's. */
const GRID = 'grid grid-cols-3 gap-x-4 gap-y-7 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8'

/** One letter of the bar: the size of a key, a finger's reach around it. */
const LETTER =
  'hit grid h-9 min-w-9 place-items-center rounded-card px-2 font-mono text-sm tabular-nums transition-colors duration-150'

/** The letter on screen wears the chosen surface, as a segmented choice does; forced colours, the system's selection. */
const CURRENT =
  'bg-ink-top text-bone shadow-[inset_0_0_0_1px_var(--color-rule-bright)] forced-colors:bg-[color:Highlight] forced-colors:text-[color:HighlightText]'

const AVAILABLE = 'text-bone hover:bg-ink-high hover:text-vermillion'

/**
 * A letter nobody is filed under: the faint ink beside the others' full one,
 * still legible — a missing letter is read too — and the system's grey under
 * forced colours.
 */
const UNAVAILABLE = 'cursor-default text-bone-faint forced-colors:text-[color:GrayText]'

/** The cards on their way: the grid's shape, until they come. */
function Waiting() {
  return (
    <ul aria-busy className={GRID}>
      {Array.from({ length: 12 }, (_, at) => (
        <li key={at} aria-hidden className="space-y-2.5">
          <Skeleton className="aspect-2/3 w-full" />
          <Skeleton className="h-4 w-3/4" />
          <Skeleton className="h-3 w-10" />
        </li>
      ))}
    </ul>
  )
}

/**
 * Somebody, as the index lists them or a search finds them: their portrait,
 * their name — and the other name they were found by, where it was not
 * theirs — how much of their work is here and in what jobs.
 */
function Card({
  person,
  matched,
}: {
  person: Pick<PersonCard, 'id' | 'name' | 'image' | 'works' | 'jobs'>
  matched?: string
}) {
  const { t } = useI18n()
  const faceless = (
    <div className="grid size-full place-items-center">
      <Glyph name="user" className="size-6 text-bone-faint" />
    </div>
  )

  return (
    <li>
      <Link to={`/people/${encodeURIComponent(person.id)}`} className="group block focus-visible:outline-offset-4">
        {/* Colour only on hover, as a poster's card: nothing grows. The
            portrait is the name's, which follows it; it says nothing more. */}
        <div
          className={cn(
            'aspect-2/3 overflow-hidden rounded-panel border border-rule bg-ink-high shadow-[var(--shadow-lift)]',
            'transition-colors duration-150 group-hover:border-vermillion/45 forced-colors:border-[color:CanvasText]',
          )}
        >
          {person.image ? (
            <Artwork
              url={person.image}
              role="portrait"
              sizes={CARD_SIZES}
              alt=""
              className="size-full object-cover"
              fallback={faceless}
            />
          ) : (
            faceless
          )}
        </div>
        <div className="mt-2.5 space-y-0.5">
          <h2 className="line-clamp-2 text-sm leading-snug font-medium text-bone transition-colors duration-200 group-hover:text-vermillion">
            {person.name}
          </h2>
          {matched ? <p className="line-clamp-2 text-xs text-bone-faint">{t.palette.matched(matched)}</p> : null}
          <p className="font-mono text-xs text-bone-faint tabular-nums">{t.people.works(person.works)}</p>
          {person.jobs.length ? (
            <p className="truncate text-xs text-bone-faint">{person.jobs.map((job) => t.person.job[job]).join(' · ')}</p>
          ) : null}
        </div>
      </Link>
    </li>
  )
}

/**
 * One filter: a row of choices where there is room for it, a menu where
 * there is not — five choices in a pill wrap into a shape nobody reads on a
 * phone.
 */
function Filter<T extends string>({
  label,
  value,
  onChange,
  options,
}: {
  label: string
  value: T
  onChange: (next: T) => void
  options: { value: T; label: string }[]
}) {
  return (
    <>
      <Segmented<T> label={label} value={value} onChange={onChange} options={options} className="hidden md:inline-flex" />
      <Select
        aria-label={label}
        value={value}
        onChange={(event) => onChange(event.target.value as T)}
        className="w-auto md:hidden"
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </Select>
    </>
  )
}

function Retry({ onRetry }: { onRetry: () => void }) {
  const { t } = useI18n()
  return (
    <Button size="sm" onClick={onRetry}>
      <Glyph name="refresh" className="size-3.5" />
      {t.common.retry}
    </Button>
  )
}
