/**
 * How the journal tab reads a change: where it is, what its values say, and
 * whether it can be gone back to — apart from the tab, so that the tab, the
 * refresh's review and their tests read them alike.
 */

import type { FieldRegistry } from '../../../lib/types'

/** Where a change is, as a lock's scope names it. */
export type Place =
  | { kind: 'item' }
  | { kind: 'season'; season: number }
  | { kind: 'episode'; season: number; episode: number }
  | { kind: 'child'; scope: string }

export function placeOf(scope: string): Place {
  if (scope === 'item') return { kind: 'item' }
  const season = /^season:(-?\d+)$/.exec(scope)
  if (season) return { kind: 'season', season: Number(season[1]) }
  const episode = /^episode:(-?\d+)x(-?\d+)$/.exec(scope)
  if (episode) return { kind: 'episode', season: Number(episode[1]), episode: Number(episode[2]) }
  return { kind: 'child', scope }
}

/** An episode's numbers as a release log writes them: S01E05. */
export function episodeCode(season: number, episode: number): string {
  return `S${String(season).padStart(2, '0')}E${String(episode).padStart(2, '0')}`
}

/** The field the journal writes an episode that moved as. */
export const MOVED = 'number'

/**
 * The kinds of child that come or go — a season, an episode, a picture, a
 * credit, an alternative title — as the journal names the field of the
 * change: of the child's own scope, what it held nothing when it came.
 */
export const CAME_OR_WENT = ['season', 'episode', 'image', 'credit', 'alternativeTitle'] as const
export type ChildKind = (typeof CAME_OR_WENT)[number]

/**
 * The kind of child a change added or removed, when it is one: an episode's
 * `image` is its still, a value of the episode, and a picture's is the
 * picture itself.
 */
export function kindOf(scope: string, field: string): ChildKind | undefined {
  const place = placeOf(scope)
  switch (field) {
    case 'season':
      return place.kind === 'season' ? 'season' : undefined
    case 'episode':
      return place.kind === 'episode' ? 'episode' : undefined
    case 'image':
    case 'credit':
    case 'alternativeTitle':
      return place.kind === 'child' ? field : undefined
    default:
      return undefined
  }
}

/**
 * A value as a line of the journal says it: a text as it reads, a flag in
 * words, a list as its entries, an episode's place as its code — and its
 * absolute number beside it — and a child that came or went by a few words
 * of it: a season or an episode by its title and its day, a credit by who and
 * whom they played, a picture by its kind and its season, another title by
 * itself and its language. Nothing for nothing.
 */
export function shown(value: unknown, words: { yes: string; no: string }): string | undefined {
  if (value === null || value === undefined || value === '') return undefined
  if (typeof value === 'boolean') return value ? words.yes : words.no
  if (typeof value === 'string' || typeof value === 'number') return String(value)
  if (Array.isArray(value)) {
    const entries = value.map((entry) => shown(entry, words)).filter((entry) => entry !== undefined)
    return entries.length ? entries.join(', ') : undefined
  }
  if (typeof value === 'object') {
    const record = value as Record<string, unknown>
    if (typeof record.season === 'number' && typeof record.episode === 'number') {
      const code = episodeCode(record.season, record.episode)
      return typeof record.absolute === 'number' ? `${code} · ${record.absolute}` : code
    }
    const said = (parts: unknown[]) => {
      const texts = parts.filter((part): part is string => typeof part === 'string' && part !== '')
      return texts.length ? texts.join(' · ') : undefined
    }
    if (typeof record.personName === 'string') return said([record.personName, record.characterName])
    if (typeof record.coverType === 'string') {
      return said([record.coverType, typeof record.seasonNumber === 'number' ? `S${String(record.seasonNumber).padStart(2, '0')}` : undefined])
    }
    if ('title' in record || 'airDate' in record || Object.keys(record).length === 0) {
      return said([record.title, record.airDate, record.language])
    }
    return JSON.stringify(value)
  }
  // What is left — a function, a symbol — is no JSON a change holds.
  return undefined
}

/**
 * Whether a change can be gone back to from the journal, by locking the
 * field to the value it held: a field a lock can hold, of the work, a
 * season or an episode. An episode that moved, or one that came or went, is
 * no field; a child a lock names is hidden or shown again from its own tab.
 */
export function revertible(scope: string, field: string, registry: FieldRegistry): boolean {
  const place = placeOf(scope)
  const fields =
    place.kind === 'item'
      ? registry.item
      : place.kind === 'season'
        ? registry.season
        : place.kind === 'episode'
          ? registry.episode
          : undefined
  return Boolean(fields?.some((def) => def.name === field))
}

/**
 * The period a reader picked, as the journal is asked for it: from the
 * start of the first day, to the end of the last — the morning after it,
 * which the server takes as the bound it does not reach.
 */
export function period(from: string, to: string): { since?: string; before?: string } {
  const day = /^\d{4}-\d{2}-\d{2}$/
  const next = (date: string) => {
    const at = new Date(`${date}T00:00:00Z`)
    at.setUTCDate(at.getUTCDate() + 1)
    return at.toISOString().slice(0, 10)
  }
  return {
    since: day.test(from) ? from : undefined,
    before: day.test(to) ? next(to) : undefined,
  }
}
