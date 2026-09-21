/**
 * Numbers, dates and durations, in the reader's language.
 *
 * `Intl` does the work; this only decides which of its shapes each kind of
 * value deserves, and what to show when the value is missing — which, in a
 * catalogue assembled from four providers, is often.
 */

const cache = new Map<string, Intl.DateTimeFormat | Intl.NumberFormat>()

function dateFormat(locale: string, options: Intl.DateTimeFormatOptions, key: string) {
  const id = `${locale}:${key}`
  let found = cache.get(id)

  if (!found) {
    found = new Intl.DateTimeFormat(locale, options)
    cache.set(id, found)
  }

  return found as Intl.DateTimeFormat
}

/** `12 January 2008`. For a release or an air date. */
export function longDate(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date) return undefined

  return dateFormat(locale, { day: 'numeric', month: 'long', year: 'numeric' }, 'long').format(date)
}

/** `12/01/2008`. For a table, where the column must stay narrow. */
export function shortDate(value: string | undefined, locale: string): string | undefined {
  const date = parse(value)
  if (!date) return undefined

  return dateFormat(locale, { day: '2-digit', month: '2-digit', year: 'numeric' }, 'short').format(date)
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
