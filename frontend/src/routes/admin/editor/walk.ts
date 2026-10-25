/**
 * The catalogue's list as it was last shown, kept for the work editor: which
 * work comes after the one open, and which before — what "Save and open the
 * next" opens, and the arrows beside the way back to the catalogue.
 *
 * Kept in this tab alone (`sessionStorage`): another tab may be walking
 * another list. The list is the page the register showed; past its end, the
 * next page is asked of the server with the list's own question.
 */

import { api } from '../../../lib/api'
import type { ItemPage } from '../../../lib/types'

/** The list, as the catalogue left it. */
export interface Run {
  /** The question the list was asked with, without its page: `kind=series&sort=title`. */
  query: string
  ids: string[]
  titles: Record<string, string>
  /** How many works the whole list holds. */
  total: number
}

/** A work beside the one open. */
export interface Neighbour {
  id: string
  title: string
}

const KEY = 'ams.catalogue.run'

/** How many works a page asked for past the end holds. */
const PAGE = 60

/** Keep the list as the catalogue shows it now. */
export function keepRun(run: Run) {
  try {
    window.sessionStorage.setItem(KEY, JSON.stringify(run))
  } catch {
    // No storage: the editor then has no next work to offer, and says so.
  }
}

/** The list as it was last kept, when it reads as one. */
export function readRun(): Run | null {
  try {
    const raw = window.sessionStorage.getItem(KEY)
    if (!raw) return null
    const run = JSON.parse(raw) as Partial<Run>
    if (typeof run.query !== 'string' || !Array.isArray(run.ids) || typeof run.total !== 'number') return null
    return {
      query: run.query,
      ids: run.ids.filter((id): id is string => typeof id === 'string'),
      titles: run.titles && typeof run.titles === 'object' ? run.titles : {},
      total: run.total,
    }
  } catch {
    return null
  }
}

/** Where a work stands in the list: its place from one, and the works on each side as far as they are known. */
export function around(run: Run | null, id: string, alias?: string): { at: number; total: number; previous?: Neighbour; next?: Neighbour } | null {
  if (!run) return null
  const index = run.ids.findIndex((one) => one === id || one === alias)
  if (index < 0) return null
  const neighbour = (at: number): Neighbour | undefined => {
    const other = run.ids[at]
    return other === undefined ? undefined : { id: other, title: run.titles[other] ?? other }
  }
  return { at: index + 1, total: run.total, previous: neighbour(index - 1), next: neighbour(index + 1) }
}

/**
 * The work after `id`: from the list kept, or from the next page of it,
 * asked of the server and kept in turn. `undefined` when the list ends
 * there, or the work is not in it.
 */
export async function nextOf(id: string, alias?: string): Promise<Neighbour | undefined> {
  const run = readRun()
  const place = around(run, id, alias)
  if (!run || !place) return undefined
  if (place.next) return place.next
  if (run.ids.length >= run.total) return undefined

  const page = await api.get<ItemPage>(`/items?${run.query}${run.query ? '&' : ''}limit=${PAGE}&offset=${run.ids.length}`)
  const known = new Set(run.ids)
  const fresh = page.items.filter((item) => !known.has(item.id))
  if (!fresh.length) return undefined
  const extended: Run = {
    query: run.query,
    ids: [...run.ids, ...fresh.map((item) => item.id)],
    titles: { ...run.titles, ...Object.fromEntries(fresh.map((item) => [item.id, item.title])) },
    total: page.total,
  }
  keepRun(extended)
  return around(extended, id, alias)?.next
}
