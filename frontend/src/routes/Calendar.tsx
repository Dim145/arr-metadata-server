/**
 * What airs this week, across every series in the catalogue.
 *
 * Set like the listings page of a newspaper: the day in the display face, the
 * time in the margin, the programme beside it. Days run in the reader's own
 * timezone, since "Monday" is theirs — an episode with a known time falls on
 * the day it is on where they are. One with only a date falls on that date,
 * which is where its network placed it; converting a date with no time would
 * move it a day early for everyone west of Greenwich.
 */

import { useQuery } from '@tanstack/react-query'
import { Link, useSearchParams } from 'react-router'

import { Artwork } from '../components/media'
import { Button, Chip, EmptyState, Glyph, Label, Skeleton } from '../components/ui'
import { api, query } from '../lib/api'
import { useTitle } from '../lib/hooks'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import { airTime, episodeCode, poster } from '../lib/media'
import type { Airing, Calendar as CalendarData, MediaItem } from '../lib/types'

/** Midnight at the start of the Monday of `date`'s week, local time. */
function mondayOf(date: Date): Date {
  const monday = new Date(date.getFullYear(), date.getMonth(), date.getDate())
  const weekday = (monday.getDay() + 6) % 7 // Monday is 0
  monday.setDate(monday.getDate() - weekday)
  return monday
}

/** `2026-09-21`, in local time — what a week is addressed by in the URL. */
const localDate = fmt.localDay

/** The week the URL asks for, or this one. */
function weekFrom(raw: string | null): Date {
  const match = raw && /^(\d{4})-(\d{2})-(\d{2})$/.exec(raw)
  if (match) {
    const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))
    if (!Number.isNaN(date.getTime())) return mondayOf(date)
  }
  return mondayOf(new Date())
}

/** The day an episode falls on, as the listing files it. */
function dayOf(airing: Airing): string | undefined {
  return airTime(airing.episode)?.day
}

export function Calendar() {
  const { t, lang, locale } = useI18n()
  const [params, setParams] = useSearchParams()
  useTitle(t.calendar.label)

  const monday = weekFrom(params.get('week'))
  const days = Array.from({ length: 7 }, (_, index) => {
    const day = new Date(monday)
    day.setDate(monday.getDate() + index)
    return day
  })

  // The reader's week, and the same dates at midnight UTC — where an episode
  // with only a date is stored, which is some hours off the reader's own
  // midnight either way. Both, and no wider: the server answers so many
  // episodes at once, and a padding day spent them on days not shown.
  const nextMonday = new Date(monday)
  nextMonday.setDate(monday.getDate() + 7)
  const utcMidnight = (d: Date) => Date.UTC(d.getFullYear(), d.getMonth(), d.getDate())
  const from = new Date(Math.min(monday.getTime(), utcMidnight(monday))).toISOString()
  const to = new Date(Math.max(nextMonday.getTime(), utcMidnight(nextMonday))).toISOString()

  const listing = useQuery({
    queryKey: ['calendar', localDate(monday), lang],
    queryFn: () => api.get<CalendarData>(`/calendar${query({ from, to, language: lang })}`),
  })

  const works = new Map<string, MediaItem>((listing.data?.works ?? []).map((w) => [w.id, w]))
  const byDay = new Map<string, Airing[]>()
  for (const airing of listing.data?.episodes ?? []) {
    const day = dayOf(airing)
    if (day) byDay.set(day, [...(byDay.get(day) ?? []), airing])
  }

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
        </div>

        <nav aria-label={t.calendar.weeks} className="flex flex-wrap items-center gap-2">
          <Button size="sm" onClick={() => move(-1)}>
            <Glyph name="chevronLeft" className="size-4" />
            {t.calendar.previous}
          </Button>
          <Button size="sm" variant={thisWeek ? 'quiet' : 'ghost'} disabled={thisWeek} onClick={() => setParams({})}>
            {t.calendar.thisWeek}
          </Button>
          <Button size="sm" onClick={() => move(1)}>
            {t.calendar.next}
            <Glyph name="chevronRight" className="size-4" />
          </Button>
        </nav>
      </header>

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
          {days.map((day, index) => {
            const key = localDate(day)
            const entries = byDay.get(key) ?? []
            const isToday = key === today
            const previous = days[index - 1]
            const afterEmpty = previous !== undefined && !byDay.get(localDate(previous))?.length

            // A day with nothing on is one line of the week, not a section of
            // its own: in a quiet week, seven headings over seven "nothing"s
            // pushed the one programme there was to the bottom of the page.
            if (!entries.length) {
              return (
                <section
                  key={key}
                  aria-labelledby={`day-${key}`}
                  className={cn(
                    'flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b border-rule py-2.5',
                    index > 0 && !afterEmpty && 'mt-10',
                  )}
                >
                  <h2 id={`day-${key}`} className="font-display text-base text-bone-faint first-letter:uppercase">
                    {fmt.dayHeading(day, locale)}
                  </h2>
                  {isToday ? <Chip tone="accent">{t.calendar.today}</Chip> : null}
                  <p className="text-sm text-bone-faint italic">{t.calendar.nothing}</p>
                </section>
              )
            }

            return (
              <section key={key} aria-labelledby={`day-${key}`} className={cn(index > 0 && 'mt-10')}>
                <div className="mb-3 flex items-baseline gap-3 border-b border-rule pb-2">
                  <h2 id={`day-${key}`} className="font-display text-xl font-medium text-bone first-letter:uppercase">
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
      <span className={cn('font-mono text-sm tabular-nums', time ? 'text-bone' : 'text-bone-faint')}>
        {time ?? '—'}
        {time ? null : <span className="sr-only">{t.calendar.noTime}</span>}
      </span>

      <div className="aspect-2/3 w-full overflow-hidden rounded-card border border-rule bg-ink-high">
        {art ? (
          <Artwork url={art} role="card" alt="" className="size-full object-cover" />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="tv" className="size-3.5 text-bone-faint" />
          </div>
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
