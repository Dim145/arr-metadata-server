/**
 * What airs, across every series in the catalogue: the week, as the listings
 * page of a newspaper, or the month at a glance.
 *
 * The week is set like a programme: the day in the display face, the time in
 * the margin, the programme beside it. The month is a grid of days, each with
 * what airs on it, for the other question — which days have something, when
 * a series is back — and every day of it a step down to its week.
 *
 * Days run in the reader's own timezone, since "Monday" is theirs — an episode
 * with a known time falls on the day it is on where they are. One with only a
 * date falls on that date, which is where its network placed it; converting a
 * date with no time would move it a day early for everyone west of Greenwich.
 */

import { useQuery } from '@tanstack/react-query'
import { Link, useSearchParams } from 'react-router'

import { Artwork, Placeholder } from '../components/media'
import { Button, Chip, EmptyState, Glyph, Label, Skeleton } from '../components/ui'
import { api, query } from '../lib/api'
import { feeds, webcal } from '../lib/feeds'
import { useMe, useTitle } from '../lib/hooks'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import { airTime, episodeCode, poster } from '../lib/media'
import { seasonOf, seasonPath, type SeasonRef } from '../lib/seasons'
import type { Airing, Calendar as CalendarData, Me, MediaItem } from '../lib/types'

type View = 'week' | 'month'

/** How many of a day's episodes the month grid lists before "+ n more". */
const SHOWN_A_DAY = 3

/** Midnight at the start of the Monday of `date`'s week, local time. */
function mondayOf(date: Date): Date {
  const monday = new Date(date.getFullYear(), date.getMonth(), date.getDate())
  const weekday = (monday.getDay() + 6) % 7 // Monday is 0
  monday.setDate(monday.getDate() - weekday)
  return monday
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
    if (!Number.isNaN(date.getTime())) return mondayOf(date)
  }
  return mondayOf(new Date())
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

/** The day an episode falls on, as the listing files it. */
function dayOf(airing: Airing): string | undefined {
  return airTime(airing.episode)?.day
}

/** Every day from `start` up to, not including, `end`. */
function daysBetween(start: Date, end: Date): Date[] {
  const days: Date[] = []
  for (let day = new Date(start); day < end; day.setDate(day.getDate() + 1)) {
    days.push(new Date(day))
  }
  return days
}

/** The episodes of a listing, filed by the day each airs. */
function filed(listing: CalendarData | undefined): { works: Map<string, MediaItem>; byDay: Map<string, Airing[]> } {
  const works = new Map<string, MediaItem>((listing?.works ?? []).map((w) => [w.id, w]))
  const byDay = new Map<string, Airing[]>()
  for (const airing of listing?.episodes ?? []) {
    const day = dayOf(airing)
    if (day) byDay.set(day, [...(byDay.get(day) ?? []), airing])
  }
  return { works, byDay }
}

/** The week view's address for a day: its week, opened at the day. */
function weekAddress(day: Date): string {
  const monday = mondayOf(day)
  const thisWeek = localDate(monday) === localDate(mondayOf(new Date()))
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

  const monday = weekFrom(params.get('week'))
  const nextMonday = new Date(monday)
  nextMonday.setDate(monday.getDate() + 7)
  const days = daysBetween(monday, nextMonday)
  const { from, to } = windowOf(monday, nextMonday)

  const listing = useQuery({
    queryKey: ['calendar', localDate(monday), lang],
    queryFn: () => api.get<CalendarData>(`/calendar${query({ from, to, language: lang })}`),
  })

  const { works, byDay } = filed(listing.data)

  const today = localDate(new Date())
  const thisWeek = localDate(mondayOf(new Date())) === localDate(monday)
  const move = (weeks: number) => {
    const next = new Date(monday)
    next.setDate(monday.getDate() + weeks * 7)
    setParams(localDate(next) === localDate(mondayOf(new Date())) ? {} : { week: localDate(next) })
  }

  const range = new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'long', year: 'numeric' }).formatRange(
    days[0] ?? monday,
    days[6] ?? monday,
  )
  const total = days.reduce((sum, day) => sum + (byDay.get(localDate(day))?.length ?? 0), 0)

  // The week as it is listed: each day with something on, and the quiet days
  // between them run together — except today, which is always its own.
  type Slot = { day: Date; key: string; entries: Airing[] }
  type Group = { kind: 'day'; slot: Slot } | { kind: 'gap'; from: Slot; to: Slot }
  const grouped: Group[] = []
  for (const day of days) {
    const key = localDate(day)
    const slot: Slot = { day, key, entries: byDay.get(key) ?? [] }
    const last = grouped.at(-1)
    if (!slot.entries.length) {
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
            {listing.isSuccess ? ` · ${t.calendar.count(total)}` : ''}
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
            <Button size="sm" variant={thisWeek ? 'quiet' : 'ghost'} disabled={thisWeek} onClick={() => setParams({})}>
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
            const n = byDay.get(key)?.length ?? 0
            const isToday = key === today
            const label = `${fmt.dayHeading(day, locale)} · ${n ? t.calendar.count(n) : t.calendar.nothing}`
            const cell = cn(
              'flex min-h-14 flex-col items-center justify-center gap-0.5 rounded-card font-mono text-xs tabular-nums transition-colors duration-150',
              isToday ? 'bg-vermillion/12 text-bone' : n ? 'text-bone' : 'text-bone-faint',
              n && 'hover:bg-ink-high',
            )
            const inner = (
              <>
                <span aria-hidden className="text-[0.625rem] tracking-wider uppercase">
                  {new Intl.DateTimeFormat(locale, { weekday: 'short' }).format(day).replace('.', '')}
                </span>
                <span aria-hidden className="font-display text-base leading-none">
                  {day.getDate()}
                </span>
                <span aria-hidden className={cn('mt-0.5 size-1.5 rounded-full', n ? 'bg-vermillion' : 'bg-transparent')} />
              </>
            )
            return (
              <li key={key} className="flex">
                {n ? (
                  <a href={`#day-${key}`} aria-label={label} aria-current={isToday ? 'date' : undefined} className={cn(cell, 'flex-1')}>
                    {inner}
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
                  className="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b border-rule py-2.5"
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

            const { day, key, entries } = group.slot
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

                <ul className="divide-y divide-rule">
                  {entries.map((airing) => (
                    <Listing
                      key={`${airing.workId}-${airing.episode.id}`}
                      airing={airing}
                      work={works.get(airing.workId)}
                    />
                  ))}
                </ul>
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

/** One programme in the listing: the time in the margin, the rest beside it. */
function Listing({ airing, work }: { airing: Airing; work?: MediaItem }) {
  const { t, locale } = useI18n()
  const { episode } = airing
  const art = work ? poster(work) : undefined
  const moment = airTime(episode)?.moment
  const time = moment ? fmt.clock(moment.toISOString(), locale) : undefined

  return (
    <li className="grid grid-cols-[3.75rem_2.75rem_minmax(0,1fr)] items-center gap-4 py-3">
      {/* A time nobody knows is said only to a screen reader: under the day's
          heading, a dash where the hour would be read as one. */}
      <span className="font-mono text-sm text-bone tabular-nums">
        {time ?? <span className="sr-only">{t.calendar.noTime}</span>}
      </span>

      <div className="aspect-2/3 w-full overflow-hidden rounded-card border border-rule bg-ink-high">
        {art ? (
          <Artwork
            url={art}
            role="card"
            alt=""
            className="size-full object-cover"
            fallback={<Placeholder title={work?.title ?? ''} kind={work?.kind ?? 'series'} className="size-full" />}
          />
        ) : (
          <Placeholder title={work?.title ?? ''} kind={work?.kind ?? 'series'} className="size-full" />
        )}
      </div>

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

/* ── The month ────────────────────────────────────────────────────────────── */

/**
 * The month at a glance: a grid of its days, Monday first, the days before
 * and after it that fill the first and last weeks dimmed but drawn — a month
 * that begins on a Thursday does not hide the Monday before. Each day lists
 * what airs on it, a few at most, and leads to its week for the rest.
 *
 * On a phone a grid of text is unreadable at this width; the days carry a
 * mark an episode each, and a tap opens the week at that day.
 */
function Month() {
  const { t, lang, locale } = useI18n()
  const me = useMe()
  const [params, setParams] = useSearchParams()

  const first = monthFrom(params.get('month'))
  const last = new Date(first.getFullYear(), first.getMonth() + 1, 0)
  const start = mondayOf(first)
  const end = mondayOf(last)
  end.setDate(end.getDate() + 7)
  const days = daysBetween(start, end)
  const { from, to } = windowOf(start, end)

  const listing = useQuery({
    queryKey: ['calendar', 'month', localDate(start), lang],
    queryFn: () => api.get<CalendarData>(`/calendar${query({ from, to, language: lang })}`),
  })

  const { works, byDay } = filed(listing.data)
  const today = localDate(new Date())
  const now = new Date()
  const thisMonth = monthKey(first) === monthKey(now)
  const inMonth = (day: Date) => day.getMonth() === first.getMonth()
  const total = days.filter(inMonth).reduce((sum, day) => sum + (byDay.get(localDate(day))?.length ?? 0), 0)

  const move = (months: number) => {
    const next = new Date(first.getFullYear(), first.getMonth() + months, 1)
    setParams(monthKey(next) === monthKey(now) ? { view: 'month' } : { view: 'month', month: monthKey(next) })
  }

  const title = new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric' }).format(first)
  const weekdays = daysBetween(start, new Date(start.getFullYear(), start.getMonth(), start.getDate() + 7))
  // The season the month is in — the middle of it, where a month always is.
  const season = seasonOf(new Date(first.getFullYear(), first.getMonth(), 15))
  const weekTo = thisMonth ? '/calendar' : `/calendar?week=${localDate(mondayOf(first))}`

  return (
    <div className="pt-10 pb-12">
      <header className="rise mb-8 flex flex-wrap items-end justify-between gap-6">
        <div>
          <Label>{t.calendar.label}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone first-letter:uppercase sm:text-4xl">{title}</h1>
          <p className="mt-2 font-mono text-sm text-bone-faint tabular-nums">
            {listing.isSuccess ? t.calendar.count(total) : ' '}
          </p>
          <Extras season={season} me={me.data} />
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <ViewToggle view="month" weekTo={weekTo} monthTo="/calendar?view=month" />
          <nav aria-label={t.calendar.months} className="flex items-center gap-1 rounded-full border border-rule p-1">
            <Button size="sm" onClick={() => move(-1)} aria-label={t.calendar.previousMonth} className="size-11 px-0">
              <Glyph name="chevronLeft" className="size-4" />
            </Button>
            <Button
              size="sm"
              variant={thisMonth ? 'quiet' : 'ghost'}
              disabled={thisMonth}
              onClick={() => setParams({ view: 'month' })}
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
          title={t.common.error}
          hint={t.calendar.loadFailed}
          action={
            <Button className="mt-2" onClick={() => void listing.refetch()}>
              <Glyph name="refresh" className="size-4" />
              {t.common.retry}
            </Button>
          }
        />
      ) : (
        <>
          {/* Wide: the grid, each day with what airs on it. */}
          <div className="rise hidden overflow-hidden rounded-panel border border-rule bg-ink-raised md:block">
            <div aria-hidden className="grid grid-cols-7 border-b border-rule">
              {weekdays.map((day) => (
                <span key={day.getDay()} className="label px-3 py-2.5">
                  {new Intl.DateTimeFormat(locale, { weekday: 'short' }).format(day)}
                </span>
              ))}
            </div>
            <ol aria-label={t.calendar.monthDays} className="grid grid-cols-7">
              {days.map((day, index) => {
                const key = localDate(day)
                const entries = byDay.get(key) ?? []
                const isToday = key === today
                const outside = !inMonth(day)
                return (
                  <li
                    key={key}
                    data-day={key}
                    aria-current={isToday ? 'date' : undefined}
                    className={cn(
                      'flex min-h-28 min-w-0 flex-col gap-1 border-rule p-2',
                      index >= 7 && 'border-t',
                      index % 7 !== 0 && 'border-l',
                      outside && 'bg-ink/40',
                      isToday && 'shadow-[inset_0_2px_0_var(--color-vermillion)]',
                    )}
                  >
                    {/* The days outside the month are told by their cell's
                        ground alone: text dimmed past that fails to read. */}
                    <span
                      className={cn(
                        'self-end font-mono text-xs tabular-nums',
                        isToday ? 'rounded-full bg-vermillion px-1.5 text-ink' : 'text-bone-faint',
                      )}
                    >
                      <span className="sr-only">
                        {fmt.dayHeading(day, locale)} · {entries.length ? t.calendar.count(entries.length) : t.calendar.nothing}
                      </span>
                      <span aria-hidden>{day.getDate()}</span>
                    </span>
                    {entries.slice(0, SHOWN_A_DAY).map((airing) => (
                      <MonthEntry key={`${airing.workId}-${airing.episode.id}`} airing={airing} work={works.get(airing.workId)} />
                    ))}
                    {entries.length > SHOWN_A_DAY ? (
                      <Link
                        to={weekAddress(day)}
                        title={t.calendar.openWeek(fmt.dayHeading(day, locale))}
                        className="mt-auto text-xs font-medium text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
                      >
                        {t.calendar.more(entries.length - SHOWN_A_DAY)}
                      </Link>
                    ) : null}
                  </li>
                )
              })}
            </ol>
          </div>

          {/* Narrow: a mark an episode, and the week a tap away. */}
          <div className="rise rounded-panel border border-rule bg-ink-raised p-2 md:hidden">
            <div aria-hidden className="grid grid-cols-7">
              {weekdays.map((day) => (
                <span key={day.getDay()} className="label py-1 text-center">
                  {new Intl.DateTimeFormat(locale, { weekday: 'narrow' }).format(day)}
                </span>
              ))}
            </div>
            <ol aria-label={t.calendar.monthDays} className="grid grid-cols-7 gap-y-1">
              {days.map((day) => {
                const key = localDate(day)
                const n = byDay.get(key)?.length ?? 0
                const isToday = key === today
                const outside = !inMonth(day)
                const label = `${fmt.dayHeading(day, locale)} · ${n ? t.calendar.count(n) : t.calendar.nothing}`
                const cell = cn(
                  'flex min-h-11 flex-1 flex-col items-center justify-center gap-1 rounded-card font-mono text-xs tabular-nums transition-colors duration-150',
                  isToday ? 'bg-vermillion/12 text-bone' : n && !outside ? 'text-bone' : 'text-bone-faint',
                  n && 'hover:bg-ink-high',
                )
                const inner = (
                  <>
                    <span aria-hidden className="font-display text-sm leading-none">
                      {day.getDate()}
                    </span>
                    <span aria-hidden className="flex h-1.5 gap-0.5">
                      {Array.from({ length: Math.min(n, 3) }, (_, dot) => (
                        <span key={dot} className="size-1.5 rounded-full bg-vermillion" />
                      ))}
                    </span>
                  </>
                )
                return (
                  <li key={key} data-day={key} className="flex">
                    {n ? (
                      <Link to={weekAddress(day)} aria-label={label} aria-current={isToday ? 'date' : undefined} className={cell}>
                        {inner}
                      </Link>
                    ) : (
                      <span role="img" aria-label={label} aria-current={isToday ? 'date' : undefined} className={cell}>
                        {inner}
                      </span>
                    )}
                  </li>
                )
              })}
            </ol>
          </div>

          {total === 0 ? (
            <p className="mt-6 text-sm text-bone-faint">{thisMonth ? t.calendar.monthEmpty : t.calendar.monthEmptyOther}</p>
          ) : null}
        </>
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

/** One episode in a day's cell: a sliver of its poster, its code, its series. */
function MonthEntry({ airing, work }: { airing: Airing; work?: MediaItem }) {
  const { episode } = airing
  const art = work ? poster(work) : undefined

  return (
    <Link
      to={`/work/${airing.workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
      title={`${work?.title ?? ''} · ${episodeCode(episode)}${episode.title ? ` · ${episode.title}` : ''}`}
      className="flex min-w-0 items-center gap-1.5 rounded-card text-xs leading-tight text-bone-dim transition-colors duration-150 hover:text-bone"
    >
      {/* A sliver too small for a stand-in: a picture that fails leaves the
          box plain rather than the browser's broken-image mark. */}
      <span className="aspect-2/3 w-4 shrink-0 overflow-hidden rounded-xs border border-rule bg-ink-high">
        {art ? (
          <Artwork url={art} role="thumb" alt="" className="size-full object-cover" fallback={<span className="block size-full" />} />
        ) : null}
      </span>
      <span className="truncate">
        <span className="font-mono text-[0.625rem] text-bone-faint tabular-nums">{episodeCode(episode)}</span>{' '}
        <span className="text-bone">{work?.title ?? '—'}</span>
      </span>
    </Link>
  )
}

/* ── Shared ───────────────────────────────────────────────────────────────── */

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
      {choices.map(([choice, to]) => (
        <Link
          key={choice}
          to={to}
          aria-current={view === choice ? 'true' : undefined}
          className={cn(
            'inline-flex min-h-9 items-center rounded-full px-3 text-sm transition-colors duration-150',
            view === choice ? 'bg-ink-top text-bone' : 'text-bone-dim hover:text-bone',
          )}
        >
          {t.calendar[choice]}
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
