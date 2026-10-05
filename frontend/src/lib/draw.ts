/**
 * One work at random, among those a set of filters lists.
 *
 * The server has the count and takes an offset, so a draw is one request for
 * the work — two where the caller does not already hold the count. The list
 * is read in the order the page shows it, so every work has the same chance.
 *
 * A count the caller holds may be the last filters' — the page keeps those up
 * while the new ones load — or a work may have gone since: an offset past the
 * end finds nothing, and is answered with the count as it now stands, which
 * is drawn from once more.
 */

import { api, query } from './api'
import type { ItemPage, MediaItem } from './types'

export type Narrowing = Record<string, string | number | boolean | undefined>

export async function drawWork(narrowing: Narrowing, total?: number): Promise<MediaItem | undefined> {
  let count = total ?? (await api.get<ItemPage>(`/items${query({ ...narrowing, limit: 1 })}`)).total
  for (let attempt = 0; attempt < 2 && count; attempt += 1) {
    // Which poster comes up: nothing anybody can lose by.
    const offset = Math.floor(Math.random() * count)
    const page = await api.get<ItemPage>(`/items${query({ ...narrowing, limit: 1, offset })}`)
    if (page.items[0]) return page.items[0]
    count = page.total
  }
  return undefined
}
