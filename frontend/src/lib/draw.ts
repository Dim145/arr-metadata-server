/**
 * One work at random, among those a set of filters lists.
 *
 * The server has the count and takes an offset, so a draw is one request for
 * the work — two where the caller does not already hold the count. The list
 * is read in the order the page shows it, so every work has the same chance.
 */

import { api, query } from './api'
import type { ItemPage, MediaItem } from './types'

export type Narrowing = Record<string, string | number | boolean | undefined>

export async function drawWork(narrowing: Narrowing, total?: number): Promise<MediaItem | undefined> {
  const count = total ?? (await api.get<ItemPage>(`/items${query({ ...narrowing, limit: 1 })}`)).total
  if (!count) return undefined
  // Which poster comes up: nothing anybody can lose by.
  const offset = Math.floor(Math.random() * count)
  return (await api.get<ItemPage>(`/items${query({ ...narrowing, limit: 1, offset })}`)).items[0]
}
