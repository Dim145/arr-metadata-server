/**
 * Numbers, dates and durations, in the reader's language.
 *
 * `Intl` does the work; this only decides which of its shapes each kind of
 * value deserves, and what to show when the value is missing — which, in a
 * catalogue assembled from four providers, is often.
 */

const cache = new Map<string, Intl.DateTimeFormat | Intl.NumberFormat>()

function dateFormat(locale: string, options: Intl.DateTimeFormatOptions, key: string) {
  const id = `${locale}:${key}:${options.timeZone ?? ''}`
  let found = cache.get(id)

  if (!found) {
    found = new Intl.DateTimeFormat(locale, options)
    cache.set(id, found)
  }

  return found as Intl.DateTimeFormat
}

/**
 * A calendar date with no time of day — `2008-01-20` — is a day, not a moment.
 *
 * `new Date` reads it as midnight UTC, so formatting it in the reader's own
 * timezone moved it: west of Greenwich, every air date and release date was
 * shown as the day before. It is formatted in UTC, where it is still that day.
 */
function zoneFor(value: string): Intl.DateTimeFormatOptions {
  return /^\d{4}-\d{2}-\d{2}$/.test(value.trim()) ? { timeZone: 'UTC' } : {}
}

/** `12 January 2008`. For a release or an air date. */
export function longDate(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date || !value) return undefined

  return dateFormat(
    locale,
    { day: 'numeric', month: 'long', year: 'numeric', ...zoneFor(value) },
    'long',
  ).format(date)
}

/** `Monday 21 September 2009`. For the day an episode aired, in full. */
export function weekdayDate(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date || !value) return undefined

  return dateFormat(
    locale,
    { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', ...zoneFor(value) },
    'weekday-long',
  ).format(date)
}

/** `Mon 21 Sept`. A day heading in a listing that is already inside one year. */
export function dayHeading(value: Date, locale: string): string {
  return dateFormat(locale, { weekday: 'long', day: 'numeric', month: 'long' }, 'day-heading').format(value)
}

/**
 * `21:30`, in the reader's own timezone.
 *
 * Only for a moment a provider actually knew. A time made up from a date —
 * midnight UTC, the fallback Sonarr is given — is not shown as one.
 */
export function clock(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date || !value || zoneFor(value).timeZone) return undefined

  return dateFormat(locale, { hour: '2-digit', minute: '2-digit' }, 'clock').format(date)
}

/** `12/01/2008`. For a table, where the column must stay narrow. */
export function shortDate(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date || !value) return undefined

  return dateFormat(
    locale,
    { day: '2-digit', month: '2-digit', year: 'numeric', ...zoneFor(value) },
    'short',
  ).format(date)
}

/** `12/01/2008, 21:00`. For anything a machine timestamped. */
export function dateTime(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date) return undefined

  return dateFormat(
    locale,
    { day: '2-digit', month: '2-digit', year: 'numeric', hour: '2-digit', minute: '2-digit' },
    'datetime',
  ).format(date)
}

/** The year alone, which is all a poster card has room for. */
export function year(value: string | undefined): string | undefined {
  const date = parse(value)
  return date ? String(date.getUTCFullYear()) : undefined
}

/**
 * `2 h 16` rather than `136 min` once a film passes an hour.
 *
 * An episode stays in minutes: `48 min` is how anyone refers to one, and
 * `0 h 48` reads like a stopwatch.
 */
export function runtime(minutes: number | undefined, locale: string): string | undefined {
  if (!minutes || minutes <= 0) return undefined

  if (minutes < 60) {
    return locale.startsWith('fr') ? `${minutes} min` : `${minutes} min`
  }

  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60

  if (rest === 0) {
    return locale.startsWith('fr') ? `${hours} h` : `${hours}h`
  }

  return locale.startsWith('fr')
    ? `${hours} h ${String(rest).padStart(2, '0')}`
    : `${hours}h ${rest}m`
}

/** `61.1 MB`: a size in the unit a person reads, counted by 1024 as the dashboard does. */
export function bytes(value: number, locale: string): string {
  const units = ['B', 'kB', 'MB', 'GB', 'TB']
  let n = value
  let unit = 0
  while (n >= 1024 && unit < units.length - 1) {
    n /= 1024
    unit += 1
  }
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: unit === 0 ? 0 : 1 }).format(n)} ${units[unit]}`
}

/** `TMDB, TheTVDB and TVmaze` / `TMDB, TheTVDB et TVmaze`. */
export function list(items: string[], locale: string): string {
  return new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(items)
}

/** `28 707` / `28,707`. Vote counts, catalogue sizes. */
export function count(value: number | undefined, locale: string): string {
  if (value === undefined || value === null) return '—'

  const id = `${locale}:count`
  let found = cache.get(id)

  if (!found) {
    found = new Intl.NumberFormat(locale)
    cache.set(id, found)
  }

  return (found as Intl.NumberFormat).format(value)
}

/** A score on ten, to one decimal, with the locale's decimal mark. */
export function score(value: number | undefined, locale: string): string | undefined {
  if (typeof value !== 'number' || value <= 0) return undefined

  const id = `${locale}:score`
  let found = cache.get(id)

  if (!found) {
    found = new Intl.NumberFormat(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 })
    cache.set(id, found)
  }

  return (found as Intl.NumberFormat).format(value)
}

/** `2026-09-21`: the reader's own calendar date of a moment. */
export function localDay(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

/**
 * `today`, `tomorrow`, `in 3 days`, `2 days ago`: how far a `YYYY-MM-DD` day
 * is from the reader's today, in their own calendar days. Elapsed time rounded
 * is wrong for this: thirty-eight hours from breakfast on a Thursday is
 * Friday night, which is tomorrow, not "in 2 days".
 */
export function dayDistance(day: string, locale: string, now = new Date()): string {
  const utc = (d: string) => Date.UTC(Number(d.slice(0, 4)), Number(d.slice(5, 7)) - 1, Number(d.slice(8, 10)))
  const days = Math.round((utc(day) - utc(localDay(now))) / 86_400_000)
  return new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(days, 'day')
}

/**
 * `3 days ago`. For a refresh time, where the exact minute is noise.
 */
export function relative(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date) return undefined

  const seconds = Math.round((date.getTime() - Date.now()) / 1000)
  const formatter = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' })

  const units: [Intl.RelativeTimeFormatUnit, number][] = [
    ['year', 31_536_000],
    ['month', 2_592_000],
    ['day', 86_400],
    ['hour', 3_600],
    ['minute', 60],
  ]

  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) {
      return formatter.format(Math.round(seconds / size), unit)
    }
  }

  return formatter.format(seconds, 'second')
}

/** An ISO string the server may not have sent, or may have sent empty. */
function parse(value: string | undefined): Date | undefined {
  if (!value) return undefined

  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? undefined : date
}
