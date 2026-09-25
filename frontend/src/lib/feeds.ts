/**
 * Where the feeds are: the schedule and one work's dates as calendars, what
 * arrives and what airs as Atom. Paths on this origin, so a page links them
 * wherever the catalogue is served from.
 */

import { query } from './api'

export const feeds = {
  calendar: (language?: string) => `/api/v1/calendar.ics${query({ language })}`,
  work: (id: string, language?: string) =>
    `/api/v1/items/${encodeURIComponent(id)}/calendar.ics${query({ language })}`,
  added: '/api/v1/feed/added.atom',
  airing: '/api/v1/feed/airing.atom',
}

/**
 * A calendar's address as a subscription. `webcal:` is what a phone's or a
 * desktop's calendar app opens as one it keeps current, where `https:` would
 * hand it the file once. The apps read `webcal:` as `http:` and `webcals:` as
 * `https:`, so the scheme follows the page's.
 */
export function webcal(path: string): string {
  const scheme = window.location.protocol === 'https:' ? 'webcals' : 'webcal'
  return `${scheme}://${window.location.host}${path}`
}
