/**
 * Seasons as season charts count them: the calendar's quarters, winter from
 * January, spring from April, summer from July, autumn from October.
 *
 * Days are `YYYY-MM-DD` strings throughout and worked out in UTC, where a day
 * is a day: the chart files an episode by the date its network gave, and the
 * reader's timezone must not move it into another week.
 */

import { api, query } from './api'
import { localDay } from './format'
import type { SeasonChart } from './types'

export const SEASONS = ['winter', 'spring', 'summer', 'autumn'] as const
export type SeasonName = (typeof SEASONS)[number]

export interface SeasonRef {
  year: number
  season: SeasonName
}

export function isSeason(value: string | undefined): value is SeasonName {
  return (SEASONS as readonly string[]).includes(value ?? '')
}

/** The season a moment falls in, where the reader is. */
export function seasonOf(date: Date): SeasonRef {
  return { year: date.getFullYear(), season: SEASONS[Math.floor(date.getMonth() / 3)]! }
}

/** The season `by` seasons away: `-1` the one before, `4` a year on. */
export function stepSeason({ year, season }: SeasonRef, by: number): SeasonRef {
  const index = year * 4 + SEASONS.indexOf(season) + by
  return { year: Math.floor(index / 4), season: SEASONS[((index % 4) + 4) % 4]! }
}

export function seasonPath({ year, season }: SeasonRef): string {
  return `/seasons/${year}/${season}`
}

/**
 * A season's chart, as the season page and the front page both ask for it —
 * one query, so going from one to the other costs nothing.
 *
 * With the reader's own day: what has aired and what airs next are counted
 * from it, as the page words them, not from the server's midnight.
 */
export function chartQuery(at: SeasonRef, language: string, today = localDay(new Date())) {
  return {
    queryKey: ['season', at.year, at.season, language, today] as const,
    queryFn: () =>
      api.get<SeasonChart>(`/seasons/${at.year}/${at.season}${query({ language, today })}`),
  }
}

const utc = (day: string) => Date.UTC(Number(day.slice(0, 4)), Number(day.slice(5, 7)) - 1, Number(day.slice(8, 10)))
const iso = (time: number) => new Date(time).toISOString().slice(0, 10)
const DAY = 86_400_000

/** The Monday of a day's week. */
export function mondayOf(day: string): string {
  const time = utc(day)
  const weekday = (new Date(time).getUTCDay() + 6) % 7
  return iso(time - weekday * DAY)
}

/** One week of a quarter, cut to the days the quarter has of it. */
export interface Week {
  /** The Monday that names it: the anchor and the key. */
  monday: string
  first: string
  last: string
}

/** The weeks a quarter is shown in, first to last. */
export function weeksOf(from: string, to: string): Week[] {
  const weeks: Week[] = []
  for (let monday = utc(mondayOf(from)); monday <= utc(to); monday += 7 * DAY) {
    const first = Math.max(monday, utc(from))
    const last = Math.min(monday + 6 * DAY, utc(to))
    weeks.push({ monday: iso(monday), first: iso(first), last: iso(last) })
  }
  return weeks
}

/** How far into its week a day is, from 0 on Monday to 6 on Sunday. */
export function weekday(day: string): number {
  return (new Date(utc(day)).getUTCDay() + 6) % 7
}

const formats = new Map<string, Intl.DateTimeFormat | Intl.RelativeTimeFormat>()

/** A day, formatted in UTC where a day is a day: one formatter per shape, kept. */
export function formatDay(day: string, locale: string, options: Intl.DateTimeFormatOptions): string {
  const id = `${locale}:${JSON.stringify(options)}`
  let format = formats.get(id) as Intl.DateTimeFormat | undefined
  if (!format) {
    format = new Intl.DateTimeFormat(locale, { ...options, timeZone: 'UTC' })
    formats.set(id, format)
  }
  return format.format(utc(day))
}

/**
 * `tomorrow`, `in 3 days`, `in 2 weeks`, `3 weeks ago`: how far off a day is,
 * while that is worth saying. Past two months the date says it better.
 */
export function around(day: string, locale: string, now = new Date()): string | undefined {
  const days = Math.round((utc(day) - utc(localDay(now))) / DAY)
  const id = `${locale}:relative`
  let format = formats.get(id) as Intl.RelativeTimeFormat | undefined
  if (!format) {
    format = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' })
    formats.set(id, format)
  }
  if (Math.abs(days) < 14) return format.format(days, 'day')
  if (Math.abs(days) < 63) return format.format(Math.round(days / 7), 'week')
  return undefined
}
