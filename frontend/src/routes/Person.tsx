/**
 * Somebody's work, as this catalogue holds it — and who they are, as TMDB
 * has it.
 *
 * Laid out like the filmography at the back of a monograph: the year in the
 * margin, the title set in the display face, the part or the job in italics.
 * It is only ever the part of a career this server happens to hold — the
 * page says so, and links to TMDB for the rest. Above it, when TMDB answers,
 * the life in a few lines: when and where they were born, what they are known
 * for, a biography folded to its first lines, and a few portraits.
 *
 * The works are read a job at a time, as Letterboxd and TMDB read a career:
 * a tab for each of acting, directing, writing and producing that they did,
 * saying how many works it holds, the one with the most open first. Within
 * a tab the newest come first, or the titles in order, or the best rated;
 * films and series together, or one kind alone where the tab holds both. All
 * of it is in the address — `?role=directing&sort=title&kind=movie` — so a
 * view is a link, and the view the page opens on says nothing there.
 *
 * A guest of a series, or the director of a few of its episodes, is credited
 * once for each episode: those credits are gathered under the series, counted
 * and dated — "3 episodes · 2019–2022" — and listed on a press.
 *
 * One page for two addresses: `/people/{id}`, by the person's record, and
 * `/person/{tmdbId}`, as the pages were addressed before there were records.
 * A record merged into another answers as that one, whose address the page
 * then takes, quietly.
 */

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Fragment, useEffect, useState, type ReactNode } from 'react'
import { Link, useLocation, useNavigate, useParams, useSearchParams } from 'react-router'

import { ExternalLink } from '../components/elsewhere'
import { Artwork } from '../components/media'
import { Button, EmptyState, Glyph, Label, Segmented, Shelf, Skeleton } from '../components/ui'
import { ApiError, api, query, segment } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useMe, useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { jobLabel } from '../lib/labels'
import { personLink } from '../lib/links'
import { episodeCode, headlineRating, poster } from '../lib/media'
import type { MediaItem, MediaKind, Person as PersonData, PersonDetails, PersonJob, Role } from '../lib/types'
import { NotFound } from './NotFound'

/** The jobs a career is read by, in the order their tabs are drawn. */
export const JOBS: readonly PersonJob[] = ['acting', 'directing', 'writing', 'producing']

/** The job each kind of credit is: a guest star is cast too, not a job behind the camera. */
const JOB_OF: Record<Role['creditType'], PersonJob> = {
  actor: 'acting',
  guest: 'acting',
  director: 'directing',
  writer: 'writing',
  producer: 'producing',
}

/** The orders a tab can be read in: the newest first, the titles from A, the best rated first. */
export const ORDERS = ['year', 'title', 'rating'] as const

export type Order = (typeof ORDERS)[number]

export function Person() {
  // `/people/{id}` takes a record's id or a TMDB id; `/person/{tmdbId}`, the
  // older address, a TMDB id alone.
  const { id, tmdbId } = useParams()
  const address = id ?? tmdbId ?? ''
  const valid = id === undefined ? /^\d+$/.test(address) : address !== ''
  const { t, lang } = useI18n()
  const client = useQueryClient()
  const navigate = useNavigate()
  const { search } = useLocation()

  const person = useQuery({
    queryKey: ['person', address, lang],
    queryFn: () => api.get<PersonData>(`/people/${segment(address)}${query({ language: lang })}`),
    enabled: valid,
  })

  // A record merged into another answers as the one it went into: the
  // address becomes that one's, with the view it asked for, and the answer
  // goes along so the page is not asked for twice. A TMDB id stays as it is:
  // it names the same person on every server, where a record's id is this
  // server's own.
  const answered = person.data
  useEffect(() => {
    if (id === undefined || !answered || answered.id === id || /^\d+$/.test(id)) return
    client.setQueryData(['person', answered.id, lang], answered)
    navigate({ pathname: `/people/${encodeURIComponent(answered.id)}`, search }, { replace: true })
  }, [id, answered, client, lang, navigate, search])

  if (!valid) return <NotFound />
  if (person.isPending) return <PersonSkeleton />
  if (person.error instanceof ApiError && person.error.status === 404) return <NotFound />
  if (person.isError || !person.data) {
    return (
      <div className="pt-16">
        <EmptyState
          kind="error"
          title={t.common.error}
          hint={t.person.loadFailed}
          action={
            <Button size="sm" onClick={() => void person.refetch()}>
              <Glyph name="refresh" className="size-3.5" />
              {t.common.retry}
            </Button>
          }
        />
      </div>
    )
  }

  return <Filmography person={person.data} />
}

function Filmography({ person }: { person: PersonData }) {
  const { t, locale } = useI18n()
  const me = useMe()
  const { pathname } = useLocation()
  const [params, setParams] = useSearchParams()
  useTitle(person.name)

  const jobs = jobsOf(person.roles)
  const opening = mostWorked(jobs)
  const asked = params.get('role')
  const job = jobs.find((entry) => entry.job === asked)?.job ?? opening
  const order: Order = ORDERS.find((one) => one === params.get('sort')) ?? 'year'
  const lines = job ? linesOf(person, job) : []
  // Films or series is a choice only where the tab holds both.
  const both = new Set(lines.map((line) => line.work.kind)).size > 1
  const askedKind = params.get('kind')
  const kind: MediaKind | '' = both && (askedKind === 'movie' || askedKind === 'series') ? askedKind : ''
  const shown = ordered(kind ? lines.filter((line) => line.work.kind === kind) : lines, order, locale)

  /** The address's query with these changed and the rest kept; what is empty is left out. */
  const searchWith = (changes: Record<string, string>, from: URLSearchParams = params) => {
    const next = new URLSearchParams(from)
    for (const [name, value] of Object.entries(changes)) {
      if (value) next.set(name, value)
      else next.delete(name)
    }
    return next
  }
  // How the tab is read: in place of the view, not one more step back. From
  // the address as it is now, as the catalogue's filters are changed.
  const change = (changes: Record<string, string>) =>
    setParams(searchWith(changes, new URLSearchParams(window.location.search)), { replace: true })

  const details = person.details
  const elsewhere = [
    person.tmdbId ? (
      <ExternalLink key="tmdb" href={personLink(person.tmdbId)}>
        {t.person.onTmdb}
      </ExternalLink>
    ) : null,
    details?.imdbId ? (
      <ExternalLink key="imdb" href={`https://www.imdb.com/name/${encodeURIComponent(details.imdbId)}/`}>
        {t.person.onImdb}
      </ExternalLink>
    ) : null,
    details?.homepage ? (
      <ExternalLink key="home" href={details.homepage}>
        {t.person.website}
      </ExternalLink>
    ) : null,
  ].filter((link) => link !== null)

  return (
    <article className="pt-10 pb-12">
      <header className="rise flex flex-col gap-6 sm:flex-row sm:items-end">
        <div className="w-32 shrink-0 sm:w-44">
          {person.image ? (
            <Artwork
              url={person.image}
              role="portrait"
              eager
              alt={t.a11y.headshot(person.name)}
              className="aspect-2/3 w-full rounded-plate border border-rule-bright object-cover shadow-[var(--shadow-plate)]"
            />
          ) : (
            <div className="grid aspect-2/3 w-full place-items-center rounded-plate border border-rule bg-ink-high">
              <Glyph name="user" className="size-8 text-bone-faint" />
            </div>
          )}
        </div>

        <div className="min-w-0">
          <Label>{t.person.label}</Label>
          <h1 className="mt-1 font-display text-4xl leading-tight font-medium text-bone sm:text-5xl">{person.name}</h1>
          {/* In their own script, said in their own language — where it is
              known — so that a voice reads 宮崎駿 as Japanese. */}
          {person.originalName && person.originalName !== person.name ? (
            <p lang={person.originalNameLang} className="mt-1 font-display text-xl text-bone-dim">
              {person.originalName}
            </p>
          ) : null}
          <p className="mt-3 font-mono text-xs text-bone-faint tabular-nums">{t.person.works(person.works.length)}</p>
          {details ? <Vitals details={details} /> : null}
          <p className="mt-4 max-w-prose text-sm leading-relaxed text-bone-dim">
            {t.person.partial}
            {elsewhere.map((link, index) => (
              <Fragment key={index}>
                {index === 0 ? ' ' : ' · '}
                {link}
              </Fragment>
            ))}
          </p>
          {me.data?.canWrite ? (
            <Link
              to={`/admin/people?id=${encodeURIComponent(person.id)}`}
              className="mt-4 inline-flex min-h-11 items-center gap-1.5 rounded-full border border-brass-deep px-3.5 text-body-sm font-medium text-brass transition-colors duration-150 hover:bg-brass/10"
            >
              <Glyph name="pencil" className="size-3.5" />
              {t.person.edit}
            </Link>
          ) : null}
        </div>
      </header>

      {details?.biography ? <Biography text={details.biography} /> : null}
      {details?.alsoKnownAs?.length ? (
        <p className="mt-6 text-sm text-bone-dim">
          <span className="label mr-2">{t.person.alsoKnownAs(details.gender)}</span>
          <span className="font-mono text-xs text-bone-faint">{details.alsoKnownAs.join(' · ')}</span>
        </p>
      ) : null}
      {details?.photos && details.photos.length > 1 ? <Portraits name={person.name} photos={details.photos} /> : null}

      {job ? (
        <>
          {/* Links rather than ARIA tabs: each job is a view with an address
              of its own, followed like any other, and the one on screen says
              so as a page does. */}
          <nav aria-label={t.person.byJob} className="mt-10 border-b border-rule">
            <ul className="flex gap-1 overflow-x-auto [scrollbar-width:none]">
              {jobs.map((entry) => {
                const current = entry.job === job
                // The job the page opens on is the bare address: one view, one
                // address. The order holds from one job to the next; films or
                // series is a choice within one, and starts again.
                const there = searchWith({ role: entry.job === opening ? '' : entry.job, kind: '' }).toString()
                return (
                  <li key={entry.job} className="shrink-0">
                    <Link
                      to={{ pathname, search: there ? `?${there}` : '' }}
                      aria-current={current ? 'page' : undefined}
                      className={cn(
                        'relative flex min-h-11 items-center gap-2 px-3 text-sm font-medium whitespace-nowrap transition-colors duration-150',
                        current ? 'text-bone' : 'text-bone-dim hover:text-bone',
                      )}
                    >
                      {t.person.job[entry.job]}{' '}
                      {/* The figure for the eye, the words for the ear. */}
                      <span aria-hidden className="font-mono text-caption text-bone-faint tabular-nums">
                        {fmt.count(entry.works, locale)}
                      </span>
                      <span className="sr-only">{t.person.jobWorks(entry.works)}</span>
                      {/* A rule under the one on screen, as the bar's tabs
                          have: a shape, not a colour alone. */}
                      <span
                        aria-hidden
                        className={cn(
                          'absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-vermillion transition-opacity duration-150',
                          current ? 'opacity-100' : 'opacity-0',
                        )}
                      />
                    </Link>
                  </li>
                )
              })}
            </ul>
          </nav>

          <div className="mt-4 flex flex-wrap items-center gap-3">
            <Segmented<Order>
              label={t.person.order}
              value={order}
              onChange={(next) => change({ sort: next === 'year' ? '' : next })}
              options={ORDERS.map((one) => ({ value: one, label: t.person.orders[one] }))}
            />
            {both ? (
              <Segmented<MediaKind | ''>
                label={t.person.kinds}
                value={kind}
                onChange={(next) => change({ kind: next })}
                options={[
                  { value: '', label: t.browse.all },
                  { value: 'movie', label: t.browse.films },
                  { value: 'series', label: t.browse.series },
                ]}
              />
            ) : null}
          </div>

          <ol className="stagger mt-4 divide-y divide-rule border-y border-rule">
            {shown.map((line) => (
              <Entry key={line.work.id} line={line} rated={order === 'rating'} />
            ))}
          </ol>
        </>
      ) : (
        // Only those who keep the catalogue open a record nothing visible credits.
        <div className="mt-10 border-y border-rule">
          <EmptyState title={t.person.nothing} hint={t.person.nothingHint} />
        </div>
      )}
    </article>
  )
}

/** When and where they were born, when they died, what they are known for. */
function Vitals({ details }: { details: PersonDetails }) {
  const { t, locale } = useI18n()
  const born = details.birthday ? fmt.longDate(details.birthday, locale) : undefined
  const died = details.deathday ? fmt.longDate(details.deathday, locale) : undefined
  const age = details.birthday ? ageBetween(details.birthday, details.deathday) : undefined

  // Agreed with the gender TMDB gives, where the language agrees anything.
  const gender = details.gender
  const lines = [
    born
      ? [
          t.person.born(born, gender),
          details.placeOfBirth ? t.person.in(details.placeOfBirth) : undefined,
          age !== undefined && !details.deathday ? `(${t.person.aged(age)})` : undefined,
        ]
          .filter(Boolean)
          .join(' ')
      : undefined,
    died
      ? [t.person.died(died, gender), age !== undefined ? t.person.at(age) : undefined].filter(Boolean).join(' ')
      : undefined,
    details.knownFor
      ? t.person.knownFor(t.person.department[details.knownFor] ?? details.knownFor, gender)
      : undefined,
  ].filter(Boolean) as string[]
  if (!lines.length) return null

  return (
    <ul className="mt-3 space-y-0.5 text-sm text-bone-dim">
      {lines.map((line) => (
        <li key={line}>{line}</li>
      ))}
    </ul>
  )
}

/** Whole years from one day to another — today, when the second is not given. */
export function ageBetween(from: string, until?: string): number | undefined {
  const start = new Date(`${from}T00:00:00Z`)
  const end = until ? new Date(`${until}T00:00:00Z`) : new Date()
  if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime()) || end < start) return undefined
  let years = end.getUTCFullYear() - start.getUTCFullYear()
  const beforeBirthday =
    end.getUTCMonth() < start.getUTCMonth() ||
    (end.getUTCMonth() === start.getUTCMonth() && end.getUTCDate() < start.getUTCDate())
  if (beforeBirthday) years -= 1
  return years
}

/**
 * The biography, folded to its first lines until asked for whole.
 *
 * TMDB's biographies are plain text with a little markdown left in them: a
 * title between asterisks, a paragraph to a line. The emphasis is set as
 * emphasis and each line as a paragraph; nothing else is read as markup, so
 * a stray asterisk — "5 * 3" — stays a stray asterisk.
 */
function Biography({ text }: { text: string }) {
  const { t } = useI18n()
  const [whole, setWhole] = useState(false)
  // Short enough to show whole: a fold that hides one line is a tease.
  const long = text.length > 700

  return (
    <section className="rise mt-8 max-w-prose" style={{ animationDelay: '80ms' }}>
      <Label>{t.person.biography}</Label>
      {/* Folded by height rather than by line, which a run of paragraphs
          does not count the same way in every engine; the fade says where
          the fold is. */}
      <div
        className={cn(
          'mt-2 space-y-3 text-sm leading-relaxed text-bone-dim',
          long && !whole && 'max-h-40 overflow-hidden [mask-image:linear-gradient(to_bottom,black_55%,transparent)]',
        )}
      >
        {paragraphs(text).map((paragraph, index) => (
          <p key={index}>{emphasised(paragraph)}</p>
        ))}
      </div>
      {long ? (
        <button
          type="button"
          onClick={() => setWhole((v) => !v)}
          aria-expanded={whole}
          className="hit mt-2 min-h-8 cursor-pointer text-xs text-vermillion underline-offset-2 hover:underline"
        >
          {whole ? t.person.readLess : t.person.readMore}
        </button>
      ) : null}
    </section>
  )
}

/**
 * The paragraphs of a text, one to a line, however the lines were ended —
 * a blank line between two is not a paragraph of its own.
 */
function paragraphs(text: string): string[] {
  return text
    .split(/\r\n?|\n/)
    .map((paragraph) => paragraph.trim())
    .filter(Boolean)
}

/**
 * Emphasis as markdown writes it: words between asterisks that hug them —
 * `*Breaking Bad*` — and not a pair of asterisks with spaces inside, which
 * is arithmetic or a footnote.
 */
const EMPHASIS = /\*([^\s*](?:[^*\n]*[^\s*])?)\*/g

/** A paragraph with its `*emphasis*` set as such, and nothing else read. */
function emphasised(text: string): ReactNode[] {
  const parts: ReactNode[] = []
  let last = 0
  for (const match of text.matchAll(EMPHASIS)) {
    const at = match.index ?? 0
    if (at > last) parts.push(text.slice(last, at))
    parts.push(<em key={at}>{match[1]}</em>)
    last = at + match[0].length
  }
  if (last < text.length) parts.push(text.slice(last))
  return parts
}

/** A few portraits, in a row that scrolls sideways. */
function Portraits({ name, photos }: { name: string; photos: string[] }) {
  const { t } = useI18n()
  return (
    <section className="mt-8">
      <Label>{t.person.portraits}</Label>
      {/* Focusable: a row that scrolls must be reachable from the keyboard,
          and nothing in it is a control but the arrows. */}
      <Shelf
        tabIndex={0}
        label={t.person.portraits}
        className="mt-3 -mx-2"
        listClassName="gap-3 rounded-card px-2 py-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-vermillion"
      >
        {photos.map((url, index) => (
          <li key={url} className="w-20 shrink-0 snap-start sm:w-24">
            <Artwork
              url={url}
              role="thumb"
              alt={t.person.portrait(name, index + 1)}
              className="aspect-2/3 w-full rounded-card border border-rule object-cover"
            />
          </li>
        ))}
      </Shelf>
    </section>
  )
}

/* ── The filmography, a job at a time ─────────────────────────────────────── */

/** One line of the filmography: a work, what they did on it, and the episodes of it they are credited on. */
export interface Line {
  work: MediaItem
  /** The credits on the work as a whole. */
  roles: Role[]
  /** The credits on single episodes of it, in the order of the series. */
  episodes: Role[]
  /** The year it is set against: the last its episodes aired in, where any is dated, else the work's own. */
  year?: number
}

/** Whether a credit is on one episode of a series rather than on the series. */
function onEpisode(role: Pick<Role, 'seasonNumber' | 'episodeNumber'>): boolean {
  return typeof role.seasonNumber === 'number' && typeof role.episodeNumber === 'number'
}

/** The year a `YYYY-MM-DD` day is in. */
function yearOf(day: string | undefined): number | undefined {
  const found = /^(\d{4})-/.exec(day ?? '')
  return found ? Number(found[1]) : undefined
}

/** The jobs somebody did, each with how many works they did it on, in the order the tabs are drawn. */
export function jobsOf(roles: Pick<Role, 'workId' | 'creditType'>[]): { job: PersonJob; works: number }[] {
  const works = new Map<PersonJob, Set<string>>()
  for (const role of roles) {
    const job = JOB_OF[role.creditType]
    const seen = works.get(job) ?? new Set<string>()
    seen.add(role.workId)
    works.set(job, seen)
  }
  return JOBS.flatMap((job) => {
    const seen = works.get(job)
    return seen ? [{ job, works: seen.size }] : []
  })
}

/** The job a page opens on: the one with the most works, the first in the tabs' order of those with as many. */
export function mostWorked(jobs: { job: PersonJob; works: number }[]): PersonJob | undefined {
  let most: { job: PersonJob; works: number } | undefined
  for (const entry of jobs) {
    if (!most || entry.works > most.works) most = entry
  }
  return most?.job
}

/**
 * The lines of one job's tab: each work they did it on, once, in the order
 * the works came — the newest first — with the credits on the work apart
 * from those on single episodes of it.
 */
export function linesOf(person: Pick<PersonData, 'roles' | 'works'>, job: PersonJob): Line[] {
  const credited = new Map<string, Pick<Line, 'roles' | 'episodes'>>()
  for (const role of person.roles) {
    if (JOB_OF[role.creditType] !== job) continue
    const on = credited.get(role.workId) ?? { roles: [], episodes: [] }
    if (onEpisode(role)) on.episodes.push(role)
    else on.roles.push(role)
    credited.set(role.workId, on)
  }
  return person.works.flatMap((work) => {
    const on = credited.get(work.id)
    if (!on) return []
    const episodes = [...on.episodes].sort(
      (a, b) => (a.seasonNumber ?? 0) - (b.seasonNumber ?? 0) || (a.episodeNumber ?? 0) - (b.episodeNumber ?? 0),
    )
    const aired = episodes.map((role) => yearOf(role.airDate)).filter((year) => year !== undefined)
    return [{ work, roles: on.roles, episodes, year: aired.length ? Math.max(...aired) : work.year }]
  })
}

/**
 * The first and the last year a run of episodes aired in — `2019–2022` — or
 * the one they all did; nothing where none is dated.
 */
export function yearSpan(episodes: Pick<Role, 'airDate'>[]): string | undefined {
  const years = episodes.map((role) => yearOf(role.airDate)).filter((year) => year !== undefined)
  if (!years.length) return undefined
  const first = Math.min(...years)
  const last = Math.max(...years)
  return first === last ? String(first) : `${first}–${last}`
}

/**
 * A tab's lines in one of its orders. The newest first is the order they
 * came in, made sure of by the year each is set against; a title or a
 * rating that two share leaves the newer of them first, and a line with no
 * year or no rating goes after those with one.
 */
export function ordered(lines: Line[], order: Order, locale: string): Line[] {
  const newest = (a: Line, b: Line) => (b.year ?? 0) - (a.year ?? 0)
  const collator = new Intl.Collator(locale, { sensitivity: 'base', numeric: true })
  const score = (line: Line) => headlineRating(line.work.ratings)?.value ?? 0
  const by: Record<Order, (a: Line, b: Line) => number> = {
    year: newest,
    title: (a, b) => collator.compare(a.work.title, b.work.title) || newest(a, b),
    rating: (a, b) => score(b) - score(a) || newest(a, b),
  }
  return [...lines].sort(by[order])
}

/** One line of the filmography, and the episodes it gathers. */
function Entry({ line, rated }: { line: Line; rated: boolean }) {
  const { t, lang, locale } = useI18n()
  const { work } = line
  const art = poster(work)

  // A job in words: TMDB files a director with no job title of its own, and
  // "director" is the name of a type, not how either language says it.
  const jobs = t.admin.editor.children as unknown as Record<string, unknown>
  const job = (type: string) => (typeof jobs[type] === 'string' ? jobs[type] : type)
  const acting = (role: Role) => role.creditType === 'actor' || role.creditType === 'guest'

  // What they did on the work as a whole, then what they did on its
  // episodes: each part, or each job, said once.
  const parts = [
    ...line.roles.map((role) =>
      acting(role)
        ? role.character
          ? t.person.as(role.character)
          : t.person.cast
        : role.character
          ? jobLabel(role.character, lang)
          : job(role.creditType),
    ),
    // A guest with no character named says nothing here: the episodes under
    // the line say what they were.
    ...line.episodes.flatMap((role) =>
      acting(role)
        ? role.character
          ? [t.person.as(role.character)]
          : []
        : [role.character ? jobLabel(role.character, lang) : job(role.creditType)],
    ),
  ].filter((text, index, all) => all.indexOf(text) === index)

  const rating = rated ? fmt.score(headlineRating(work.ratings)?.value, locale) : undefined
  const tail = [work.kind === 'series' ? t.home.kindSeries : t.home.kindFilm, rating].filter(Boolean).join(' · ')
  // The margins of the line and of its episodes, one under the other.
  const columns = 'grid grid-cols-[3.5rem_2.5rem_minmax(0,1fr)] gap-x-4 sm:grid-cols-[4.5rem_3rem_minmax(0,1fr)]'

  return (
    <li>
      <Link
        to={`/work/${work.id}`}
        className={cn(columns, 'group items-center py-3 transition-colors duration-150 hover:bg-ink-raised')}
      >
        <span className="font-mono text-sm text-bone-faint tabular-nums">{line.year ?? '—'}</span>
        <div className="aspect-2/3 w-full overflow-hidden rounded-card border border-rule bg-ink-high">
          {art ? (
            <Artwork url={art} role="card" alt="" className="size-full object-cover" />
          ) : (
            <div className="grid size-full place-items-center">
              <Glyph name={work.kind === 'series' ? 'tv' : 'film'} className="size-3.5 text-bone-faint" />
            </div>
          )}
        </div>
        <div className="min-w-0">
          <p className="truncate font-display text-lg leading-snug text-bone transition-colors duration-150 group-hover:text-vermillion">
            {work.title}
          </p>
          <p className="truncate text-sm text-bone-faint">
            {parts.length ? (
              <>
                <span className="italic">{parts.join(' · ')}</span>
                {' · '}
              </>
            ) : null}
            {tail}
          </p>
        </div>
      </Link>

      {line.episodes.length ? (
        <div className={cn(columns, 'pb-3')}>
          {/* A disclosure the browser keeps: opened from the keyboard, and
              by the browser's own search for an episode's title. */}
          <details className="group col-start-3 min-w-0">
            <summary className="hit inline-flex min-h-8 cursor-pointer list-none items-center gap-1.5 text-xs font-medium text-bone-dim transition-colors duration-150 hover:text-bone [&::-webkit-details-marker]:hidden">
              <Glyph name="chevronDown" className="size-3 transition-transform duration-200 group-open:rotate-180" />
              {t.person.episodes(line.episodes.length, yearSpan(line.episodes))}
              <span className="sr-only">{t.person.episodesIn(work.title)}</span>
            </summary>
            <ol className="mt-1.5">
              {line.episodes.map((role) => (
                <li key={role.creditId}>
                  <Link
                    to={`/work/${work.id}/season/${role.seasonNumber}/episode/${role.episodeNumber}`}
                    className="flex min-h-9 items-baseline gap-2 py-1 text-sm text-bone-dim transition-colors duration-150 hover:text-vermillion"
                  >
                    <span className="shrink-0 font-mono text-xs text-bone-faint tabular-nums">
                      {episodeCode({ seasonNumber: role.seasonNumber ?? 0, episodeNumber: role.episodeNumber ?? 0 })}
                    </span>
                    <span className="min-w-0 truncate">
                      {role.episodeTitle ?? '—'}
                      {acting(role) && role.character ? (
                        <span className="text-bone-faint italic"> · {t.person.as(role.character)}</span>
                      ) : null}
                    </span>
                    {role.airDate ? (
                      <span className="ml-auto shrink-0 font-mono text-xs text-bone-faint tabular-nums">{fmt.year(role.airDate)}</span>
                    ) : null}
                  </Link>
                </li>
              ))}
            </ol>
          </details>
        </div>
      ) : null}
    </li>
  )
}

function PersonSkeleton() {
  return (
    <div className="space-y-6 pt-10">
      <div className="flex gap-6">
        <Skeleton className="aspect-2/3 w-32 sm:w-44" />
        <div className="flex-1 space-y-3 pt-20">
          <Skeleton className="h-10 w-1/2" />
          <Skeleton className="h-4 w-1/3" />
        </div>
      </div>
      <Skeleton className="h-11 w-72" />
      {Array.from({ length: 5 }, (_, index) => (
        <Skeleton key={index} className="h-16 w-full" />
      ))}
    </div>
  )
}
