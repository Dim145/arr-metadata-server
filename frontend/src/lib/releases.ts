/**
 * A work's releases and ratings, country by country, read for one reader.
 *
 * The country is the reader's: the one they picked on a work's page to see
 * where it is watched, else the one the server answers them for (its
 * `tmdb.watchRegion`, else the language's), else the one the interface's
 * language names. The same rules as the server's, kept beside the page that
 * reads them: `MediaItem::certification_in` and `days_in` in Rust.
 */

import type { CountryRelease, MediaItem, Me, ReleaseKind } from './types'

/** Where the country a reader picked is kept, in this browser alone. */
export const REGION_KEY = 'ams.watchRegion'

/** A country code as releases are filed under it: two letters, upper case. */
export function countryCode(raw: string | undefined | null): string | undefined {
  const code = raw?.trim().toUpperCase()
  return code && /^[A-Z]{2}$/.test(code) ? code : undefined
}

/** The country the reader picked, if they did. */
export function pickedRegion(): string | undefined {
  try {
    return countryCode(localStorage.getItem(REGION_KEY))
  } catch {
    return undefined
  }
}

/** The reader's country: picked, answered for, or the language's own. */
export function readerRegion(me: Me | undefined, locale: string, picked = pickedRegion()): string {
  return picked ?? countryCode(me?.region) ?? countryCode(locale.split(/[-_]/)[1]) ?? 'US'
}

/** A country's name where the engine knows it; its code where it does not. */
export function countryName(code: string, locale: string): string {
  try {
    return new Intl.DisplayNames([locale], { type: 'region' }).of(code.toUpperCase()) ?? code
  } catch {
    return code
  }
}

/** Whose rating stands for a country's when it lists several: cinemas first. */
const RANK: (ReleaseKind | undefined)[] = ['theatrical', 'limited', undefined, 'premiere', 'digital', 'physical', 'tv']

function rank(kind: ReleaseKind | undefined): number {
  const at = RANK.indexOf(kind)
  return at === -1 ? RANK.length : at
}

/** One country's releases, earliest first; a rating with no day after the dated ones. */
export function releasesIn(item: MediaItem, country: string): CountryRelease[] {
  return (item.releases ?? [])
    .filter((r) => r.country.toUpperCase() === country)
    .sort((a, b) => (a.date ?? '~').localeCompare(b.date ?? '~') || rank(a.releaseType) - rank(b.releaseType))
}

/**
 * The rating the work carries in a country: its record's, when that is the
 * country's — a person may have locked it — else the first its releases
 * there give, the one for cinemas before the rest.
 */
export function certificationIn(item: MediaItem, country: string): string | undefined {
  if (countryCode(item.contentRatingCountry) === country && item.contentRating?.trim()) {
    return item.contentRating.trim()
  }
  const rated = releasesIn(item, country)
    .filter((r) => r.certification?.trim())
    .sort((a, b) => rank(a.releaseType) - rank(b.releaseType) || (a.date ?? '~').localeCompare(b.date ?? '~'))
  return rated[0]?.certification?.trim()
}

/** The first day it came out in a country in cinemas: a limited run is one, a festival is not. */
export function cinemaDayIn(item: MediaItem, country: string): string | undefined {
  return releasesIn(item, country).find(
    (r) => (r.releaseType === 'theatrical' || r.releaseType === 'limited') && r.date,
  )?.date
}

/** Every country the work has releases or a rating in, but one, by name. */
export function otherCountries(item: MediaItem, but: string, locale: string): string[] {
  const codes = new Set((item.releases ?? []).map((r) => r.country.toUpperCase()))
  codes.delete(but)
  return [...codes].sort((a, b) => countryName(a, locale).localeCompare(countryName(b, locale), locale))
}
