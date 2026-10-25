/**
 * Where a genre, a network, a year and a decade have pages of their own.
 *
 * A genre's page is addressed by its slug, which is the same in every
 * language: the server knows "Drama" and "Drame" as `drama` and answers to
 * either, so a work's chip may name a genre as the work was given it, and the
 * page then takes the address the server calls it by. A network's is the slug
 * of its name. A year's is the year, a decade's its first year.
 */

import { segment } from './api'

/**
 * A name made into the slug the server would make of it: letters in lower
 * case without their accents, anything else a single hyphen. A name with
 * letters beyond those the server transliterates differently — `œ`, a script
 * other than the Latin — is handed over as it is, for the server to fold.
 */
export function slugOf(name: string): string {
  const folded = name.trim().normalize('NFKD').replace(/\p{M}/gu, '')
  if (/[^\x20-\x7e]/.test(folded)) {
    return encodeURIComponent(name.trim())
  }
  return folded
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
}

/** A genre's page, from any name it goes by. */
export function genrePath(name: string): string {
  return `/genre/${slugOf(name)}`
}

/** A network's or a studio's page. */
export function networkPath(name: string): string {
  return `/network/${slugOf(name)}`
}

/** A year's page. */
export function yearPath(year: number): string {
  return `/year/${year}`
}

/** The page of the decade a year belongs to. */
export function decadePath(year: number): string {
  return `/decade/${year - (((year % 10) + 10) % 10)}`
}

/** What the pages of the catalogue are of, as their addresses say it. */
export type FacetKind = 'genre' | 'network' | 'year' | 'decade'

/** The API path a page of the catalogue is read from. */
export function facetApiPath(kind: FacetKind, key: string): string {
  const plural = { genre: 'genres', network: 'networks', year: 'years', decade: 'decades' }[kind]
  return `/${plural}/${segment(key)}`
}
