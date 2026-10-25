/**
 * What airs and what comes out, across every series and film in the
 * catalogue: the week, as the listings page of a newspaper, or the month at a
 * glance.
 *
 * The week is set like a programme: the day in the display face, the time in
 * the margin, the programme beside it. The month is a table of days, each with
 * what airs on it, for the other question — which days have something, when
 * a series is back — and every day of it a step down to its week; or, for
 * whoever does not read a grid, the same month as a list, day after day.
 *
 * Days run in the reader's own timezone, since "Monday" is theirs — an episode
 * with a known time falls on the day it is on where they are. One with only a
 * date falls on that date, which is where its network placed it; converting a
 * date with no time would move it a day early for everyone west of Greenwich.
 * A film comes out on a date too — in cinemas, digitally, on disc — and is
 * listed on it after the day's episodes, having no hour to be placed at.
 */

import { useQuery } from '@tanstack/react-query'
import { useEffect, useId, useRef, useState, type MouseEvent } from 'react'
import { Link, useLocation, useSearchParams } from 'react-router'

import { Artwork, Placeholder, VEILED, Veil, useVeiled } from '../components/media'
import { Button, Chip, EmptyState, Glyph, Label, POPOVER, Skeleton } from '../components/ui'
import { api, query } from '../lib/api'
import { feeds, webcal } from '../lib/feeds'
import { useMe, useTitle } from '../lib/hooks'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import { airTime, backdrop, episodeCode, poster } from '../lib/media'
import { airsWhenOn, releaseDay, releasedNotAired } from '../lib/platforms'
import { mondayOf, seasonOf, seasonPath, type SeasonRef } from '../lib/seasons'
import { plainClick } from '../lib/transitions'
import type { Airing, Calendar as CalendarData, CalendarEntry, Me, MediaItem, Release } from '../lib/types'

type View = 'week' | 'month'

/** How the month is laid out: a grid of its days, or a list of those with something on. */
type Layout = 'grid' | 'list'

type Episodic = Extract<CalendarEntry, { kind: 'episode' }>
type Releasing = Extract<CalendarEntry, { kind: 'release' }>

/** What a day lists: its episodes in the order they air, then the films out on it. */
type Day = { episodes: Episodic[]; releases: Releasing[] }

/** How many of a day's entries the month grid lists before "+ n more". */
const SHOWN_A_DAY = 3

/**
 * Midnight at the start of the Monday of `date`'s week, local time: the
 * reader's own day, its Monday found by the one rule the season charts use
 * too. A date and a time with no offset is read as local time.
 */
function weekStart(date: Date): Date {
  return new Date(`${mondayOf(fmt.localDay(date))}T00:00`)
}

/** `2026-09-21`, in local time — what a week is addressed by in the URL. */
const localDate = fmt.localDay

/** `2026-10` — what a month is addressed by. */
function monthKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`
}

/** The week the URL asks for, or this one. */
function weekFrom(raw: string | null): Date {
  const match = raw && /^(\d{4})-(\d{2})-(\d{2})$/.exec(raw)
  if (match) {
    const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))
    if (!Number.isNaN(date.getTime())) return weekStart(date)
  }
  return weekStart(new Date())
}

/** The first day of the month the URL asks for, or of this one. */
function monthFrom(raw: string | null): Date {
  const match = raw && /^(\d{4})-(\d{2})$/.exec(raw)
  if (match) {
    const date = new Date(Number(match[1]), Number(match[2]) - 1, 1)
    if (!Number.isNaN(date.getTime()) && date.getMonth() === Number(match[2]) - 1) return date
  }
  const now = new Date()
  return new Date(now.getFullYear(), now.getMonth(), 1)
}

/**
 * The window to ask the server for: the reader's days, and the same dates at
 * midnight UTC — where an episode with only a date is stored, which is some
 * hours off the reader's own midnight either way. Both, and no wider: the
 * server answers so many episodes at once, and a padding day spent them on
 * days not shown.
 */
function windowOf(start: Date, end: Date): { from: string; to: string } {
  const utcMidnight = (d: Date) => Date.UTC(d.getFullYear(), d.getMonth(), d.getDate())
  return {
    from: new Date(Math.min(start.getTime(), utcMidnight(start))).toISOString(),
    to: new Date(Math.max(end.getTime(), utcMidnight(end))).toISOString(),
  }
}

/** The day an entry falls on, as the listing files it: a film's is its date. */
function dayOf(entry: CalendarEntry): string | undefined {
  return entry.kind === 'release' ? entry.date : airTime(entry.episode)?.day
}

/** How many entries a day lists, of either kind. */
function sizeOf(day: Day | undefined): number {
  return (day?.episodes.length ?? 0) + (day?.releases.length ?? 0)
}

/** A day's entries, as its counter says them: "3 episodes · 1 release". */
function counted(day: Day | undefined, count: (episodes: number, releases?: number) => string): string {
  return count(day?.episodes.length ?? 0, day?.releases.length ?? 0)
}

/** The episodes and the releases of some days, counted apart. */
function tally(byDay: Map<string, Day>, days: Date[]): { episodes: number; releases: number } {
  let episodes = 0
  let releases = 0
  for (const day of days) {
    const listed = byDay.get(localDate(day))
    episodes += listed?.episodes.length ?? 0
    releases += listed?.releases.length ?? 0
  }
  return { episodes, releases }
}

/** Every day from `start` up to, not including, `end`. */
function daysBetween(start: Date, end: Date): Date[] {
  const days: Date[] = []
  for (let day = new Date(start); day < end; day.setDate(day.getDate() + 1)) {
    days.push(new Date(day))
  }
  return days
}

/**
 * The entries of a listing, filed by the day each falls on: the episodes in
 * the order the server gave them, which is the order they air in, and the
 * films after them.
 */
function filed(listing: CalendarData | undefined): { works: Map<string, MediaItem>; byDay: Map<string, Day> } {
  const works = new Map<string, MediaItem>((listing?.works ?? []).map((w) => [w.id, w]))
  const byDay = new Map<string, Day>()
  for (const entry of listing?.entries ?? []) {
    const day = dayOf(entry)
    if (!day) continue
    const listed = byDay.get(day) ?? { episodes: [], releases: [] }
    if (entry.kind === 'release') listed.releases.push(entry)
    else listed.episodes.push(entry)
    byDay.set(day, listed)
  }
  return { works, byDay }
}

/** The week view's address for a day: its week, opened at the day. */
function weekAddress(day: Date): string {
  const monday = weekStart(day)
  const thisWeek = localDate(monday) === localDate(weekStart(new Date()))
  return `/calendar${thisWeek ? '' : `?week=${localDate(monday)}`}#day-${localDate(day)}`
}

export function Calendar() {
  const { t } = useI18n()
  const [params] = useSearchParams()
  useTitle(t.calendar.label)

  const view: View = params.get('view') === 'month' ? 'month' : 'week'
  return view === 'month' ? <Month /> : <Week />
}

/* ── The week ─────────────────────────────────────────────────────────────── */

function Week() {
  const { t, lang, locale } = useI18n()
  const me = useMe()
  const [params, setParams] = useSearchParams()
  const { hash } = useLocation()

  const monday = weekFrom(params.get('week'))
  const nextMonday = new Date(monday)
  nextMonday.setDate(monday.getDate() + 7)
  const days = daysBetween(monday, nextMonday)
  const { from, to } = windowOf(monday, nextMonday)

  const listing = useQuery({
    queryKey: ['calendar', localDate(monday), lang],
    queryFn: () => api.get<CalendarData>(`/calendar${query({ from, to, language: lang })}`),
  })

  // Opened at a day — from a day of the month, or by `t` for today — the
  // week is read from that day's line, once the listing has drawn it. A
  // task later: a page newly opened is first put at its top by the shell,
  // whose effects run after this one's.
  useEffect(() => {
    if (!listing.isSuccess || !hash.startsWith('#day-')) return
    const later = window.setTimeout(() => document.getElementById(hash.slice(1))?.scrollIntoView({ block: 'start' }))
    return () => window.clearTimeout(later)
  }, [hash, listing.isSuccess])

  const { works, byDay } = filed(listing.data)

  const today = localDate(new Date())
  const thisWeek = localDate(weekStart(new Date())) === localDate(monday)
  const move = (weeks: number) => {
    const next = new Date(monday)
    next.setDate(monday.getDate() + weeks * 7)
    setParams(localDate(next) === localDate(weekStart(new Date())) ? {} : { week: localDate(next) })
  }

  const range = new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'long', year: 'numeric' }).formatRange(
    days[0] ?? monday,
    days[6] ?? monday,
  )
  const counts = tally(byDay, days)
  const total = counts.episodes + counts.releases

  // The week as it is listed: each day with something on, and the quiet days
  // between them run together — except today, which is always its own.
  type Slot = { day: Date; key: string; listed: Day | undefined }
  type Group = { kind: 'day'; slot: Slot } | { kind: 'gap'; from: Slot; to: Slot }
  const grouped: Group[] = []
  for (const day of days) {
    const key = localDate(day)
    const slot: Slot = { day, key, listed: byDay.get(key) }
    const last = grouped.at(-1)
    if (!sizeOf(slot.listed)) {
      // Today never runs together with the days around it: marked, on its
      // own line, so a reader sees where the week stands.
      if (key !== today && last?.kind === 'gap' && last.to.key !== today) last.to = slot
      else grouped.push({ kind: 'gap', from: slot, to: slot })
    } else {
      grouped.push({ kind: 'day', slot })
    }
  }
  // The season most of the week is in: a week across the turn of a quarter
  // belongs where its Thursday does.
  const season = seasonOf(days[3] ?? monday)
  // The same days, as a month: this one's, unless the week is this week's.
  const monthOf = days[3] ?? monday
  const monthTo = `/calendar?view=month${monthKey(monthOf) === monthKey(new Date()) ? '' : `&month=${monthKey(monthOf)}`}`

  return (
    <div className="pt-10 pb-12">
      <header className="rise mb-8 flex flex-wrap items-end justify-between gap-6">
        <div>
          <Label>{t.calendar.label}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
            {thisWeek ? t.calendar.title : monday.getTime() < Date.now() ? t.calendar.titlePast : t.calendar.titleFuture}
          </h1>
          <p className="mt-2 font-mono text-sm text-bone-faint tabular-nums">
            {range}
            {listing.isSuccess ? ` · ${t.calendar.count(counts.episodes, counts.releases)}` : ''}
          </p>
          <Extras season={season} me={me.data} />
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <ViewToggle view="week" weekTo="/calendar" monthTo={monthTo} />
          {/* One row, whatever the width: the arrows either side of the week
              they step from. Three labelled buttons wrapped onto two lines on a
              phone, the last one alone. */}
          <nav aria-label={t.calendar.weeks} className="flex items-center gap-1 rounded-full border border-rule p-1">
            <Button size="sm" onClick={() => move(-1)} aria-label={t.calendar.previous} className="size-11 px-0">
              <Glyph name="chevronLeft" className="size-4" />
            </Button>
            <Button
              size="sm"
              variant={thisWeek ? 'quiet' : 'ghost'}
              disabled={thisWeek}
              onClick={() => setParams({})}
              aria-keyshortcuts="t"
            >
              {t.calendar.thisWeek}
            </Button>
            <Button size="sm" onClick={() => move(1)} aria-label={t.calendar.next} className="size-11 px-0">
              <Glyph name="chevronRight" className="size-4" />
            </Button>
          </nav>
        </div>
      </header>

      {/* The week at a glance: a cell a day, a mark on the days that have
          something on, and each of those a step down to its listing. */}
      {listing.isSuccess && total > 0 ? (
        <ol aria-label={t.calendar.days} className="rise mb-8 grid grid-cols-7 gap-1 rounded-panel border border-rule bg-ink-raised p-1">
          {days.map((day) => {
            const key = localDate(day)
            const listed = byDay.get(key)
            const n = sizeOf(listed)
            const isToday = key === today
            const label = `${fmt.dayHeading(day, locale)} · ${n ? counted(listed, t.calendar.count) : t.calendar.nothing}`
            const cell = cn(
              'flex min-h-14 flex-col items-center justify-center gap-0.5 rounded-card font-mono text-xs tabular-nums transition-colors duration-150',
              isToday ? 'bg-vermillion/12 text-bone' : n ? 'text-bone' : 'text-bone-faint',
              n && 'hover:bg-ink-high',
            )
            const inner = (
              <>
                <span className="text-caption tracking-wider uppercase">
                  {new Intl.DateTimeFormat(locale, { weekday: 'short' }).format(day).replace('.', '')}
                </span>
                <span className="font-display text-base leading-none">{day.getDate()}</span>
                <span aria-hidden className={cn('mt-0.5 size-1.5 rounded-full', n ? 'bg-vermillion' : 'bg-transparent')} />
              </>
            )
            return (
              <li key={key} className="flex">
                {n ? (
                  // Named by what it shows, the day and its date, then the
                  // month and what is on: whoever speaks to the page says
                  // what they see (WCAG 2.5.3).
                  <a href={`#day-${key}`} aria-current={isToday ? 'date' : undefined} className={cn(cell, 'flex-1')}>
                    {inner}
                    <span className="sr-only">
                      {` ${new Intl.DateTimeFormat(locale, { month: 'long' }).format(day)} · ${counted(listed, t.calendar.count)}`}
                    </span>
                  </a>
                ) : (
                  <span role="img" aria-label={label} aria-current={isToday ? 'date' : undefined} className={cn(cell, 'flex-1')}>
                    {inner}
                  </span>
                )}
              </li>
            )
          })}
        </ol>
      ) : null}

      {listing.isPending ? (
        <div className="space-y-8">
          {Array.from({ length: 3 }, (_, index) => (
            <div key={index} className="space-y-3">
              <Skeleton className="h-7 w-56" />
              <Skeleton className="h-16 w-full" />
              <Skeleton className="h-16 w-full" />
            </div>
          ))}
        </div>
      ) : listing.isError ? (
        <EmptyState
          kind="error"
          title={t.common.error}
          hint={t.calendar.loadFailed}
          action={
            <Button className="mt-2" onClick={() => void listing.refetch()}>
              <Glyph name="refresh" className="size-4" />
              {t.common.retry}
            </Button>
          }
        />
      ) : total === 0 ? (
        <EmptyState title={thisWeek ? t.calendar.empty : t.calendar.emptyOther} hint={t.calendar.emptyHint} />
      ) : (
        <div className="stagger">
          {grouped.map((group) => {
            // Days with nothing on run together as one line, however many:
            // in a quiet week, six lines of "nothing" pushed the one programme
            // there was to the bottom of the page. Today keeps its own line,
            // marked, so a reader can see where the week stands.
            if (group.kind === 'gap') {
              const { from, to } = group
              const heading =
                from.key === to.key
                  ? fmt.dayHeading(from.day, locale)
                  : new Intl.DateTimeFormat(locale, { weekday: 'long', day: 'numeric', month: 'long' }).formatRange(from.day, to.day)
              const isToday = from.key === today
              return (
                <section
                  key={from.key}
                  id={`day-${from.key}`}
                  aria-labelledby={`day-heading-${from.key}`}
                  className="flex scroll-mt-24 flex-wrap items-baseline gap-x-3 gap-y-1 border-b border-rule py-2.5"
                >
                  <h2 id={`day-heading-${from.key}`} className="font-display text-base text-bone-faint first-letter:uppercase">
                    {heading}
                  </h2>
                  {isToday ? <Chip tone="accent">{t.calendar.today}</Chip> : null}
                  <p className="text-sm text-bone-faint italic">
                    {from.key === to.key ? t.calendar.nothing : t.calendar.nothingBetween}
                  </p>
                </section>
              )
            }

            const { day, key, listed } = group.slot
            const isToday = key === today
            return (
              <section
                key={key}
                id={`day-${key}`}
                aria-labelledby={`day-heading-${key}`}
                className="scroll-mt-24 pt-10 first:pt-0"
              >
                <div className="mb-3 flex items-baseline gap-3 border-b border-rule pb-2">
                  <h2 id={`day-heading-${key}`} className="font-display text-xl font-medium text-bone first-letter:uppercase">
                    {fmt.dayHeading(day, locale)}
                  </h2>
                  {isToday ? <Chip tone="accent">{t.calendar.today}</Chip> : null}
                </div>

                <DayListing listed={listed} works={works} />
              </section>
            )
          })}
        </div>
      )}

      {listing.data?.truncated ? (
        <p role="note" className="mt-8 rounded-panel border border-brass-deep px-4 py-3 text-sm text-bone-dim">
          {t.calendar.truncated}
        </p>
      ) : null}

      <p className="mt-10 text-xs leading-relaxed text-bone-faint">{t.calendar.note}</p>
    </div>
  )
}

/** A day's programme: its episodes in the order they air, then its films. */
function DayListing({ listed, works }: { listed: Day | undefined; works: Map<string, MediaItem> }) {
  return (
    <ul className="divide-y divide-rule">
      {listed?.episodes.map((airing) => (
        <Listing key={`${airing.workId}-${airing.episode.id}`} airing={airing} work={works.get(airing.workId)} />
      ))}
      {listed?.releases.map((release) => (
        <ReleaseListing key={`${release.workId}-${release.date}`} release={release} work={works.get(release.workId)} />
      ))}
    </ul>
  )
}

/** A work's poster, small, in a listing: veiled as every poster of a work for adults is. */
function ListingPoster({ work, kind }: { work?: MediaItem; kind: MediaItem['kind'] }) {
  const art = work ? poster(work) : undefined
  const veiled = useVeiled(work ?? { id: '' })
  const standIn = <Placeholder title={work?.title ?? ''} kind={work?.kind ?? kind} className="size-full" />

  return (
    <div className="aspect-2/3 w-full overflow-hidden rounded-card border border-rule bg-ink-high">
      {art ? (
        <Artwork url={art} role="card" alt="" className={cn('size-full object-cover', veiled && VEILED)} fallback={standIn} />
      ) : (
        standIn
      )}
    </div>
  )
}

/** One programme in the listing: the time in the margin, the rest beside it. */
function Listing({ airing, work }: { airing: Airing; work?: MediaItem }) {
  const { t, locale } = useI18n()
  const { episode } = airing
  const moment = airTime(episode)?.moment
  const time = moment ? fmt.clock(moment.toISOString(), locale) : undefined

  return (
    <li className="grid grid-cols-[3.75rem_2.75rem_minmax(0,1fr)] items-center gap-4 py-3">
      {/* A time nobody knows is said only to a screen reader: under the day's
          heading, a dash where the hour would be read as one. */}
      <span className="font-mono text-sm text-bone tabular-nums">
        {moment && time ? <time dateTime={moment.toISOString()}>{time}</time> : <span className="sr-only">{t.calendar.noTime}</span>}
      </span>

      <ListingPoster work={work} kind="series" />

      <div className="min-w-0">
        <p className="flex flex-wrap items-baseline gap-x-3">
          <Link
            to={`/work/${airing.workId}`}
            className="truncate font-display text-lg leading-snug text-bone transition-colors duration-150 hover:text-vermillion"
          >
            {work?.title ?? '—'}
          </Link>
          {work?.network ? <span className="text-xs text-bone-faint">{work.network}</span> : null}
        </p>
        <Link
          to={`/work/${airing.workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
          className="mt-0.5 flex min-h-6 items-baseline gap-2 text-sm text-bone-dim transition-colors duration-150 hover:text-bone"
        >
          <span className="font-mono text-xs text-bone-faint tabular-nums">{episodeCode(episode)}</span>
          <span className="truncate">{episode.title || t.episode.untitled(episode.episodeNumber)}</span>
          {episode.finaleType ? <Chip tone="accent">{t.calendar.finale}</Chip> : null}
        </Link>
      </div>
    </li>
  )
}

/**
 * A film coming out, in the listing: its poster beside its title as a
 * series' is, the film's mark in the margin where a time would be — a
 * release has a day, not an hour — and what comes out, a chip each.
 */
function ReleaseListing({ release, work }: { release: Release; work?: MediaItem }) {
  const { t } = useI18n()

  return (
    <li className="grid grid-cols-[3.75rem_2.75rem_minmax(0,1fr)] items-center gap-4 py-3">
      <Glyph name="film" className="text-bone-faint" />

      <ListingPoster work={work} kind="movie" />

      <div className="min-w-0">
        <p className="flex flex-wrap items-baseline gap-x-3">
          <Link
            to={`/work/${release.workId}`}
            className="truncate font-display text-lg leading-snug text-bone transition-colors duration-150 hover:text-vermillion"
          >
            {work?.title ?? '—'}
          </Link>
          {work?.studio ? <span className="text-xs text-bone-faint">{work.studio}</span> : null}
        </p>
        <p className="mt-1 flex flex-wrap items-center gap-1.5">
          {release.releaseTypes.map((type) => (
            <Chip key={type}>{t.calendar.releaseType[type]}</Chip>
          ))}
        </p>
      </div>
    </li>
  )
}

/* ── The month ────────────────────────────────────────────────────────────── */

/**
 * The month at a glance: a table of its days, Monday first, a week a row, the
 * days before and after it that fill the first and last weeks dimmed but
 * drawn — a month that begins on a Thursday does not hide the Monday before.
 * A table, which a screen reader walks by column and row, its name the month
 * and each column the day's name in full; a grid of list items announced the
 * days with no week and no weekday.
 *
 * Each day lists what airs on it and then what comes out, a few at most, a
 * poster beside each where the day is wide enough to show one, and leads to
 * its week for the rest. An episode opens a look at itself over the month —
 * what it is, when, and the way to its page — rather than taking the reader
 * away from the month to see it.
 *
 * On a phone a grid of text is unreadable at this width; the days carry a
 * mark an entry each, and a tap opens the week at that day. And for anyone
 * who reads a month better as a list, the same days in order, as the week
 * lists them.
 */
function Month() {
  const { t, lang, locale } = useI18n()
  const me = useMe()
  const [params, setParams] = useSearchParams()
  const id = useId()

  const first = monthFrom(params.get('month'))
  const layout: Layout = params.get('as') === 'list' ? 'list' : 'grid'
  const last = new Date(first.getFullYear(), first.getMonth() + 1, 0)
  const start = weekStart(first)
  const end = weekStart(last)
  end.setDate(end.getDate() + 7)
  const days = daysBetween(start, end)
  const { from, to } = windowOf(start, end)

  const listing = useQuery({
    queryKey: ['calendar', 'month', localDate(start), lang],
    queryFn: () => api.get<CalendarData>(`/calendar${query({ from, to, language: lang })}`),
  })

  // The episode looked at, and the entry it was opened from, which it is
  // drawn beside and which the focus goes back to.
  const [looking, setLooking] = useState<{ airing: Airing; key: string; keyboard: boolean } | null>(null)
  const panel = useRef<HTMLDivElement>(null)
  const opener = useRef<HTMLElement | null>(null)

  useEffect(() => {
    const node = panel.current
    if (!looking || !node) return
    if (!node.matches(':popover-open')) node.showPopover()
    // From the keyboard, the look is read from its top, where it opens, and
    // Tab walks on through it; a pointer leaves the focus where it was.
    if (looking.keyboard) node.focus()
  }, [looking])

  // Where the engine has no popovers, an episode is the way to its page, as
  // it always was; with a key held it is one too, for a tab of its own.
  const look = (airing: Airing, key: string) => (event: MouseEvent<HTMLAnchorElement>) => {
    if (!POPOVER || !plainClick(event)) return
    event.preventDefault()
    opener.current = event.currentTarget
    // A click the keyboard made carries no count of presses.
    setLooking({ airing, key, keyboard: event.detail === 0 })
  }

  const { works, byDay } = filed(listing.data)
  const today = localDate(new Date())
  const now = new Date()
  const thisMonth = monthKey(first) === monthKey(now)
  const inMonth = (day: Date) => day.getMonth() === first.getMonth()
  const counts = tally(byDay, days.filter(inMonth))
  const total = counts.episodes + counts.releases

  /** The month's address, its layout kept unless another is asked for. */
  const address = (month: Date, as: Layout = layout) => {
    const next: Record<string, string> = { view: 'month' }
    if (monthKey(month) !== monthKey(now)) next.month = monthKey(month)
    if (as === 'list') next.as = 'list'
    return next
  }
  const move = (months: number) => setParams(address(new Date(first.getFullYear(), first.getMonth() + months, 1)))

  const title = new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric' }).format(first)
  const weekdays = daysBetween(start, new Date(start.getFullYear(), start.getMonth(), start.getDate() + 7))
  const weeks = Array.from({ length: days.length / 7 }, (_, row) => days.slice(row * 7, row * 7 + 7))
  // The season the month is in — the middle of it, where a month always is.
  const season = seasonOf(new Date(first.getFullYear(), first.getMonth(), 15))
  const weekTo = thisMonth ? '/calendar' : `/calendar?week=${localDate(weekStart(first))}`
  const layoutTo = (as: Layout) => `/calendar?${new URLSearchParams(address(first, as)).toString()}`

  return (
    <div className="pt-10 pb-12">
      <header className="rise mb-8 flex flex-wrap items-end justify-between gap-6">
        <div>
          <Label>{t.calendar.label}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone first-letter:uppercase sm:text-4xl">{title}</h1>
          <p className="mt-2 font-mono text-sm text-bone-faint tabular-nums">
            {listing.isSuccess ? t.calendar.count(counts.episodes, counts.releases) : ' '}
          </p>
          <Extras season={season} me={me.data} />
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <ViewToggle view="month" weekTo={weekTo} monthTo="/calendar?view=month" />
          <LayoutToggle layout={layout} gridTo={layoutTo('grid')} listTo={layoutTo('list')} />
          <nav aria-label={t.calendar.months} className="flex items-center gap-1 rounded-full border border-rule p-1">
            <Button size="sm" onClick={() => move(-1)} aria-label={t.calendar.previousMonth} className="size-11 px-0">
              <Glyph name="chevronLeft" className="size-4" />
            </Button>
            <Button
              size="sm"
              variant={thisMonth ? 'quiet' : 'ghost'}
              disabled={thisMonth}
              onClick={() => setParams(address(now))}
              aria-keyshortcuts="t"
            >
              {t.calendar.thisMonth}
            </Button>
            <Button size="sm" onClick={() => move(1)} aria-label={t.calendar.nextMonth} className="size-11 px-0">
              <Glyph name="chevronRight" className="size-4" />
            </Button>
          </nav>
        </div>
      </header>

      {listing.isPending ? (
        <Skeleton className="h-[32rem] w-full" />
      ) : listing.isError ? (
        <EmptyState
          kind="error"
          title={t.common.error}
          hint={t.calendar.loadFailed}
          action={
            <Button className="mt-2" onClick={() => void listing.refetch()}>
              <Glyph name="refresh" className="size-4" />
              {t.common.retry}
            </Button>
          }
        />
      ) : layout === 'list' ? (
        <MonthList days={days.filter(inMonth)} byDay={byDay} works={works} />
      ) : (
        <>
          <div className="rise overflow-hidden rounded-panel border border-rule bg-ink-raised">
            <table className="w-full table-fixed border-collapse">
              <caption className="sr-only">{title}</caption>
              <thead>
                <tr className="border-b border-rule">
                  {weekdays.map((day) => (
                    <th key={day.getDay()} scope="col" className="label px-1 py-2.5 text-center font-medium md:px-3 md:text-left">
                      {/* The abbreviation for the eye, a letter on a phone;
                          the name in full for the ear. */}
                      <span aria-hidden className="md:hidden">
                        {new Intl.DateTimeFormat(locale, { weekday: 'narrow' }).format(day)}
                      </span>
                      <span aria-hidden className="hidden md:inline">
                        {new Intl.DateTimeFormat(locale, { weekday: 'short' }).format(day)}
                      </span>
                      <span className="sr-only">{new Intl.DateTimeFormat(locale, { weekday: 'long' }).format(day)}</span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {weeks.map((week, row) => (
                  <tr key={localDate(week[0]!)} className={cn(row > 0 && 'border-t border-rule')}>
                    {week.map((day, column) => {
                      const key = localDate(day)
                      return (
                        <MonthDay
                          key={key}
                          day={day}
                          listed={byDay.get(key)}
                          works={works}
                          isToday={key === today}
                          outside={!inMonth(day)}
                          first={column === 0}
                          looking={looking?.key}
                          onLook={look}
                        />
                      )
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {total === 0 ? (
            <p className="mt-6 text-sm text-bone-faint">{thisMonth ? t.calendar.monthEmpty : t.calendar.monthEmptyOther}</p>
          ) : null}
        </>
      )}

      {/* The look at an episode: one for the month, filled with the one
          opened, and drawn beside it where the browser ties a popover to an
          anchor — over the middle of the screen where it does not. */}
      {POPOVER ? (
        <div
          ref={panel}
          id={`${id}-look`}
          popover="auto"
          role="dialog"
          aria-labelledby={`${id}-look-title`}
          tabIndex={-1}
          onToggle={(event) => {
            if (event.newState === 'open') return
            // Closed with the focus inside it: back to the entry it opened from.
            if (panel.current?.contains(document.activeElement)) opener.current?.focus()
            setLooking(null)
          }}
          // Below the entry, or above it, whichever side has the more room,
          // and no taller than that room, its margins counted — a box that
          // overflows its room by a margin's width is refused there. What
          // does not fit scrolls.
          className={cn(
            'popover-in max-h-[calc(100%-0.75rem)] w-[min(20rem,calc(100vw-2rem))] overflow-y-auto overscroll-contain rounded-panel border border-rule-bright bg-(--surface-overlay) p-0 text-bone shadow-[var(--shadow-plate)]',
            'supports-[anchor-name:--a]:[position-anchor:--episode-look] supports-[anchor-name:--a]:[inset:auto] supports-[anchor-name:--a]:m-0 supports-[anchor-name:--a]:my-1.5',
            'supports-[anchor-name:--a]:[position-area:block-end_span-inline-end] supports-[anchor-name:--a]:[position-try-fallbacks:flip-block,flip-inline,flip-block_flip-inline]',
            'supports-[anchor-name:--a]:[position-try-order:most-height]',
          )}
        >
          {looking ? (
            <EpisodeLook
              airing={looking.airing}
              work={works.get(looking.airing.workId)}
              titleId={`${id}-look-title`}
              onClose={() => panel.current?.hidePopover()}
            />
          ) : null}
        </div>
      ) : null}

      {listing.data?.truncated ? (
        <p role="note" className="mt-8 rounded-panel border border-brass-deep px-4 py-3 text-sm text-bone-dim">
          {t.calendar.truncated}
        </p>
      ) : null}

      <p className="mt-10 text-xs leading-relaxed text-bone-faint">{t.calendar.note}</p>
    </div>
  )
}

/**
 * One day of the month's table. Its number is the way to its week — on a
 * phone the whole cell, with a mark an entry; wider, at the corner, above
 * what airs and comes out that day. The days outside the month are told by
 * their cell's ground alone: text dimmed past that fails to read.
 */
function MonthDay({
  day,
  listed,
  works,
  isToday,
  outside,
  first,
  looking,
  onLook,
}: {
  day: Date
  listed: Day | undefined
  works: Map<string, MediaItem>
  isToday: boolean
  outside: boolean
  /** The first of its row, which has no rule on its left. */
  first: boolean
  /** The entry an episode is being looked at from, if any. */
  looking: string | undefined
  onLook: (airing: Airing, key: string) => (event: MouseEvent<HTMLAnchorElement>) => void
}) {
  const { t, locale } = useI18n()
  const key = localDate(day)
  const entries: CalendarEntry[] = [...(listed?.episodes ?? []), ...(listed?.releases ?? [])]
  const n = entries.length
  const label = `${fmt.dayHeading(day, locale)} · ${n ? counted(listed, t.calendar.count) : t.calendar.nothing}`
  const number = (
    <>
      <time
        dateTime={key}
        aria-hidden
        className={cn(
          'font-display text-sm leading-none md:font-mono md:text-xs md:tabular-nums',
          isToday && 'md:rounded-full md:bg-vermillion md:px-1.5 md:py-0.5 md:text-ink',
        )}
      >
        {day.getDate()}
      </time>
      <span aria-hidden className="flex h-1.5 gap-0.5 md:hidden">
        {Array.from({ length: Math.min(n, 3) }, (_, dot) => (
          <span key={dot} className="size-1.5 rounded-full bg-vermillion" />
        ))}
      </span>
    </>
  )
  const corner = cn(
    'flex min-h-11 flex-col items-center justify-center gap-1 rounded-card transition-colors duration-150',
    'md:min-h-0 md:items-end md:self-end',
    isToday ? 'bg-vermillion/12 text-bone md:bg-transparent' : n && !outside ? 'text-bone' : 'text-bone-faint',
  )

  return (
    <td
      data-day={key}
      aria-current={isToday ? 'date' : undefined}
      className={cn(
        'h-14 border-rule p-1 align-top md:h-28 md:p-2',
        !first && 'border-l',
        outside && 'bg-ink/40',
        isToday && 'md:shadow-[inset_0_2px_0_var(--color-vermillion)]',
      )}
    >
      <div className="@container flex h-full min-w-0 flex-col gap-1">
        {n ? (
          <Link to={weekAddress(day)} aria-label={label} className={cn(corner, 'hover:bg-ink-high md:hover:bg-transparent md:hover:text-vermillion')}>
            {number}
          </Link>
        ) : (
          <span className={corner}>
            <span className="sr-only">{label}</span>
            {number}
          </span>
        )}
        {n ? (
          <ul className="hidden min-w-0 flex-col gap-1.5 md:flex">
            {entries.slice(0, SHOWN_A_DAY).map((entry) =>
              entry.kind === 'release' ? (
                <MonthRelease key={`${entry.workId}-${entry.date}`} release={entry} work={works.get(entry.workId)} />
              ) : (
                <MonthEntry
                  key={`${entry.workId}-${entry.episode.id}`}
                  airing={entry}
                  work={works.get(entry.workId)}
                  anchored={looking === `${entry.workId}-${entry.episode.id}`}
                  onLook={onLook(entry, `${entry.workId}-${entry.episode.id}`)}
                />
              ),
            )}
          </ul>
        ) : null}
        {n > SHOWN_A_DAY ? (
          <Link
            to={weekAddress(day)}
            title={t.calendar.openWeek(fmt.dayHeading(day, locale))}
            className="mt-auto hidden text-xs font-medium text-vermillion transition-colors duration-150 hover:text-vermillion-bright md:block"
          >
            {t.calendar.more(n - SHOWN_A_DAY)}
          </Link>
        ) : null}
      </div>
    </td>
  )
}

/**
 * A work's poster in a day's cell: a sliver where the day is narrow, a small
 * poster where it is wide enough to show one — as a reading diary sets them
 * in its month. A picture that fails leaves the box plain rather than the
 * browser's broken-image mark: a sliver is too small for a stand-in.
 */
function CellPoster({ work }: { work?: MediaItem }) {
  const art = work ? poster(work) : undefined
  const veiled = useVeiled(work ?? { id: '' })

  return (
    <span className="aspect-2/3 w-1 shrink-0 overflow-hidden rounded-xs border border-rule bg-ink-high @min-[7rem]:w-4 @min-[9rem]:w-7">
      {art ? (
        <Artwork
          url={art}
          role="thumb"
          alt=""
          className={cn('size-full object-cover', veiled && VEILED)}
          fallback={<span className="block size-full" />}
        />
      ) : null}
    </span>
  )
}

/**
 * One episode in a day's cell: its poster, its hour and code, its series.
 * A press opens a look at it over the month (see `Month`); a press with a
 * key held, or a browser without popovers, follows it to its page.
 */
function MonthEntry({
  airing,
  work,
  anchored,
  onLook,
}: {
  airing: Airing
  work?: MediaItem
  /** Whether the look is drawn beside this one. */
  anchored: boolean
  onLook: (event: MouseEvent<HTMLAnchorElement>) => void
}) {
  const { t, locale } = useI18n()
  const { episode } = airing
  const moment = airTime(episode)?.moment

  return (
    <li className="min-w-0">
      <Link
        to={`/work/${airing.workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
        title={`${work?.title ?? ''} · ${episodeCode(episode)}${episode.title ? ` · ${episode.title}` : ''}`}
        aria-haspopup={POPOVER ? 'dialog' : undefined}
        onClick={onLook}
        // The name the look is tied to, worn by one entry at a time.
        style={anchored ? { anchorName: '--episode-look' } : undefined}
        className="flex min-w-0 items-start gap-1.5 rounded-card text-xs leading-tight text-bone-dim transition-colors duration-150 hover:text-bone"
      >
        <CellPoster work={work} />
        <span className="min-w-0">
          <span className="flex items-baseline gap-1.5 font-mono text-caption text-bone-faint tabular-nums">
            {moment ? (
              <time dateTime={moment.toISOString()} className="hidden @min-[9rem]:inline">
                {fmt.clock(moment.toISOString(), locale)}
              </time>
            ) : (
              <time dateTime={airTime(episode)?.day} className="sr-only">
                {t.calendar.noTime}
              </time>
            )}
            <span>{episodeCode(episode)}</span>
          </span>
          <span className="block truncate text-bone">{work?.title ?? '—'}</span>
        </span>
      </Link>
    </li>
  )
}

/**
 * A film coming out, in a day's cell: its poster, the film's mark where an
 * episode's code is, its title; what comes out is said in full to whoever
 * hovers or listens.
 */
function MonthRelease({ release, work }: { release: Release; work?: MediaItem }) {
  const { t } = useI18n()
  const kinds = release.releaseTypes.map((type) => t.calendar.releaseType[type]).join(' · ')

  return (
    <li className="min-w-0">
      <Link
        to={`/work/${release.workId}`}
        title={`${work?.title ?? ''} · ${kinds}`}
        className="flex min-w-0 items-start gap-1.5 rounded-card text-xs leading-tight text-bone-dim transition-colors duration-150 hover:text-bone"
      >
        <CellPoster work={work} />
        <span className="min-w-0">
          <Glyph name="film" className="size-3 text-bone-faint" />
          <span className="block truncate text-bone">{work?.title ?? '—'}</span>
          <span className="sr-only"> · {kinds}</span>
        </span>
      </Link>
    </li>
  )
}

/**
 * The look at one episode, over the month: its picture, its series, its code
 * and title, when it airs and where, a few lines of what it is about, and the
 * way to its page.
 */
function EpisodeLook({
  airing,
  work,
  titleId,
  onClose,
}: {
  airing: Airing
  work?: MediaItem
  titleId: string
  onClose: () => void
}) {
  const { t, locale } = useI18n()
  const { episode } = airing
  const when = airTime(episode)
  const image = episode.image ?? (work ? backdrop(work) : undefined)
  const veiled = useVeiled(work ?? { id: '' })
  // A platform's midnight is the day it puts the episode up, not an hour it
  // was on: said as a day, as its own page says it.
  const released = when !== undefined && releasedNotAired(work?.network, when)
  const shownDay = when ? (released ? releaseDay(when) : when.day) : undefined
  const day = shownDay ? fmt.dayHeading(new Date(`${shownDay}T00:00`), locale) : undefined
  const time = !released && when?.moment ? fmt.clock(when.moment.toISOString(), locale) : undefined
  const moment = when && !released && when.moment ? when.moment.toISOString() : shownDay

  return (
    <div>
      <div className="relative aspect-video overflow-hidden bg-ink-high">
        {image ? (
          <Artwork url={image} role="still" alt="" className={cn('size-full object-cover', veiled && VEILED)} />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="tv" className="size-6 text-bone-faint" />
          </div>
        )}
        {veiled && image ? <Veil id={airing.workId} shows={false} /> : null}
        <button
          type="button"
          onClick={onClose}
          aria-label={t.nav.close}
          className="absolute top-1.5 right-1.5 grid size-11 cursor-pointer place-items-center rounded-full bg-ink/80 text-bone transition-colors duration-150 hover:bg-ink"
        >
          <Glyph name="close" className="size-4" />
        </button>
      </div>

      <div className="space-y-2 p-4">
        <Link
          to={`/work/${airing.workId}`}
          className="label inline-flex min-h-6 items-center transition-colors duration-150 hover:text-vermillion"
        >
          {work?.title ?? t.calendar.openSeries}
        </Link>
        <h2 id={titleId} className="text-base leading-snug font-medium text-bone">
          <span className="font-mono text-xs text-bone-faint tabular-nums">{episodeCode(episode)}</span>{' '}
          {episode.title || t.episode.untitled(episode.episodeNumber)}
        </h2>
        <p className="text-xs text-bone-dim first-letter:uppercase">
          {moment ? <time dateTime={moment}>{[day, time].filter(Boolean).join(' · ')}</time> : null}
          {[when ? airsWhenOn(work?.network, when, locale) : undefined, work?.network]
            .filter(Boolean)
            .map((part) => ` · ${part}`)
            .join('')}
        </p>
        {episode.overview ? (
          <p className="line-clamp-3 text-sm leading-relaxed text-bone-dim">{episode.overview}</p>
        ) : null}
        <Link
          to={`/work/${airing.workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
          className="-ml-3 inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm font-medium text-vermillion transition-colors duration-150 hover:bg-vermillion/10"
        >
          {t.calendar.openEpisode}
          <Glyph name="chevronRight" className="size-3.5" />
        </Link>
      </div>
    </div>
  )
}

/**
 * The month as a list, for whoever reads a list better than a grid: its days
 * with something on, in order, each a heading and its programme as the week
 * lists it. A screen reader hears it as one ordered list of days.
 */
function MonthList({ days, byDay, works }: { days: Date[]; byDay: Map<string, Day>; works: Map<string, MediaItem> }) {
  const { t, locale } = useI18n()
  const today = localDate(new Date())
  const listed = days.filter((day) => sizeOf(byDay.get(localDate(day))))

  if (!listed.length) return null

  return (
    <ol aria-label={t.calendar.listView} className="stagger space-y-10">
      {listed.map((day) => {
        const key = localDate(day)
        return (
          <li key={key} id={`day-${key}`} className="scroll-mt-24">
            <div className="mb-3 flex items-baseline gap-3 border-b border-rule pb-2">
              <h2 className="font-display text-xl font-medium text-bone first-letter:uppercase">
                <time dateTime={key}>{fmt.dayHeading(day, locale)}</time>
              </h2>
              {key === today ? <Chip tone="accent">{t.calendar.today}</Chip> : null}
            </div>
            <DayListing listed={byDay.get(key)} works={works} />
          </li>
        )
      })}
    </ol>
  )
}

/* ── Shared ───────────────────────────────────────────────────────────────── */

/** The classes of a choice in a pair of linked choices, the current one marked. */
function choice(current: boolean) {
  return cn(
    'inline-flex min-h-9 items-center rounded-full px-3 text-sm transition-colors duration-150',
    current ? 'bg-ink-top text-bone' : 'text-bone-dim hover:text-bone',
  )
}

/**
 * Week or month: two links, the current one marked. A group rather than a
 * navigation landmark, and marked `true` rather than `page`: the two are
 * views of the one page, whose tab in the bar stays the only current page.
 */
function ViewToggle({ view, weekTo, monthTo }: { view: View; weekTo: string; monthTo: string }) {
  const { t } = useI18n()
  const choices: [View, string][] = [
    ['week', weekTo],
    ['month', monthTo],
  ]

  return (
    <div role="group" aria-label={t.calendar.views} className="flex items-center gap-1 rounded-full border border-rule p-1">
      {choices.map(([value, to]) => (
        <Link key={value} to={to} aria-current={view === value ? 'true' : undefined} className={choice(view === value)}>
          {t.calendar[value]}
        </Link>
      ))}
    </div>
  )
}

/** The month as a grid or as a list: two links, as the views are. */
function LayoutToggle({ layout, gridTo, listTo }: { layout: Layout; gridTo: string; listTo: string }) {
  const { t } = useI18n()
  const choices: [Layout, string, 'grid' | 'list'][] = [
    ['grid', gridTo, 'grid'],
    ['list', listTo, 'list'],
  ]

  return (
    <div role="group" aria-label={t.calendar.layouts} className="flex items-center gap-1 rounded-full border border-rule p-1">
      {choices.map(([value, to, glyph]) => (
        <Link key={value} to={to} aria-current={layout === value ? 'true' : undefined} className={cn(choice(layout === value), 'gap-1.5')}>
          <Glyph name={glyph} className="size-3.5" />
          {t.calendar[value]}
        </Link>
      ))}
    </div>
  )
}

/** The season chart the dates fall in, and the schedule as a subscription. */
function Extras({ season, me }: { season: SeasonRef; me: Me | undefined }) {
  const { t, lang } = useI18n()

  return (
    <div className="-ml-3 mt-2 flex flex-wrap items-center gap-x-1">
      <Link
        to={seasonPath(season)}
        className="inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-vermillion transition-colors duration-150 hover:bg-vermillion/10"
      >
        <Glyph name="calendar" className="size-4" />
        {t.calendar.seasonChart(t.seasons.title(t.seasons.names[season.season], season.year))}
        <Glyph name="chevronRight" className="size-3.5" />
      </Link>
      {/* The same schedule in a calendar app, kept current there: a
          `webcal:` address is opened as a subscription, not saved. Only
          where the catalogue is open: the app carries no credential. */}
      {me?.publicBrowse ? (
        <a
          href={webcal(feeds.calendar(lang))}
          title={t.feeds.calendarHint}
          className="inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
        >
          <Glyph name="rss" className="size-4" />
          {t.feeds.subscribe}
        </a>
      ) : null}
    </div>
  )
}
