/**
 * The orders a series' episodes come in, as the pages that show them name
 * them: the editor's orders panel, its other numberings, a season's sheet
 * and the work's banner while TMDB numbers the series.
 *
 * An order is asked for by its key — TheTVDB's own type (`dvd`, `alttwo`),
 * `tmdb-` and a TMDB group's id, `ams-` and the name of one made here — and
 * called by its label: the series' own name for one of TheTVDB's ("Superfan
 * Order"), the group's on TMDB, the name it was given here.
 */

import type { EpisodeOrder, Episode, PlacedEpisode } from '../../../lib/types'

/**
 * TheTVDB's names for its types, which an order of a series that names it
 * nothing of its own carries: such an order is called in the reader's
 * language instead, from the dictionary's words for the types.
 */
const TVDB_TYPE_NAMES: Record<string, string> = {
  dvd: 'DVD Order',
  absolute: 'Absolute Order',
  alternate: 'Alternate Order',
  regional: 'Regional Order',
  altdvd: 'Alternate DVD Order',
  alttwo: 'Alternate Order 2',
}

type Named = Pick<EpisodeOrder, 'key' | 'label' | 'source' | 'isDefault'>

/**
 * What an order is called: the aired one, and one of TheTVDB's named only by
 * its type, in the reader's language (`words`, the dictionary's
 * `season.order`); any other by its own name.
 */
export function orderName(order: Named | undefined, words: Record<string, string>): string {
  if (!order || order.isDefault) return words.official ?? 'official'
  if (order.source === 'tvdb' && TVDB_TYPE_NAMES[order.key] === order.label) return words[order.key] ?? order.label
  return order.label
}

/**
 * Each order's name by its key, where several are shown side by side: the
 * name, and where the order comes from after a name another order goes by
 * too — TheTVDB and TMDB may both have a "Superfan Order", and a reader
 * choosing between them is owed the difference (`sources`, the dictionary's
 * `season.orderSource`).
 */
export function orderNames(orders: Named[], words: Record<string, string>, sources: Record<string, string>): Map<string, string> {
  const named = orders.map((order) => ({ order, name: orderName(order, words) }))
  const uses = new Map<string, number>()
  for (const { name } of named) uses.set(name.toLowerCase(), (uses.get(name.toLowerCase()) ?? 0) + 1)
  return new Map(
    named.map(({ order, name }) => [
      order.key,
      (uses.get(name.toLowerCase()) ?? 0) > 1 ? `${name} · ${sources[order.source] ?? order.source}` : name,
    ]),
  )
}

/** The seasons an order has, specials last. */
export function orderSeasons(order: Pick<EpisodeOrder, 'episodes'>): number[] {
  const numbers = [...new Set(order.episodes.map((e) => e.seasonNumber))]
  return numbers.sort((a, b) => (a === 0 ? 1 : b === 0 ? -1 : a - b))
}

/**
 * The episode a place stands for: the one the work holds, by the id the
 * order names it with — as the server resolves it when it answers — or the
 * record the order carries of one the work does not hold, as an episode of
 * its own, numbered by its place. Nothing for a place that names an episode
 * the work no longer holds and carries no record.
 */
export function placedEpisode(
  placed: PlacedEpisode,
  byId: Map<string, Episode>,
): { episode: Episode; own: boolean } | undefined {
  const held = placed.episodeId ? byId.get(placed.episodeId) : undefined
  if (held) return { episode: held, own: false }
  const record = placed.record
  if (!record) return undefined
  return {
    own: true,
    episode: {
      id: placed.id,
      seasonNumber: placed.seasonNumber,
      episodeNumber: placed.episodeNumber,
      absoluteEpisodeNumber: placed.absoluteNumber,
      title: record.title,
      overview: record.overview,
      airDate: record.aired,
      runtime: record.runtime,
      image: record.image,
      finaleType: record.finaleType,
      tvdbId: placed.tvdbId,
      tmdbId: placed.tmdbId,
      isManual: false,
    },
  }
}
