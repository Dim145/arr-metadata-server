/**
 * What the numbering banner says of a series, read off the numbering its
 * orders carry: who numbers it, since when and why — TMDB in TheTVDB's place
 * while TheTVDB does not answer for it, or the numbering it was served by,
 * kept, once TheTVDB no longer knows it — and what giving it back did.
 */

import type { Dict } from '../../../lib/i18n'
import * as fmt from '../../../lib/format'
import type { Orders } from '../../../lib/types'

import { TVDB_LAST } from './orders'

type Numbering = Orders['numbering']

type Words = Dict['work']['tmdbNumbering']

/** What the banner says: a line that leads, the lines that explain it, a caution beside the way back. */
export interface Said {
  /**
   * `held` while the series is numbered otherwise than by TheTVDB — by TMDB,
   * or as it was served when TheTVDB stopped knowing it; `note` while
   * TheTVDB has said once that it has no such series, which changes nothing
   * yet and is for whoever keeps the catalogue.
   */
  tone: 'held' | 'note'
  title: string
  lines: string[]
  /** Said beside "Back to TheTVDB", for whoever may press it. */
  caution?: string
  /** Whether the way back is offered. */
  back: boolean
}

/** What the banner says of a series, or nothing where TheTVDB numbers it and has said nothing amiss. */
export function numberingSaid(numbering: Numbering | undefined, w: Words, locale: string): Said | undefined {
  if (!numbering) return undefined
  const into = numbering.mergedInto === undefined ? undefined : String(numbering.mergedInto)

  if (numbering.droppedSince) {
    const day = fmt.longDate(numbering.droppedSince, locale) ?? numbering.droppedSince
    const d = w.dropped
    const title =
      numbering.droppedWhy === 'merged' && into
        ? d.merged(into, day)
        : numbering.droppedWhy === 'deleted'
          ? d.deleted(day)
          : d.missing(day)
    const lines = [numbering.frozen === TVDB_LAST ? d.served : d.thawed]
    if (numbering.heldByTmdb) lines.push(d.listedByTmdb)
    else if (numbering.droppedWhy === 'merged' && into) lines.push(d.listedByMerged(into))
    return {
      tone: 'held',
      title,
      lines,
      caution: numbering.droppedWhy === 'merged' && into ? d.mergedCaution(into) : undefined,
      back: true,
    }
  }

  if (numbering.heldByTmdb) {
    const since = numbering.fallbackSince ? fmt.longDate(numbering.fallbackSince, locale) : undefined
    return {
      tone: 'held',
      title: since ? w.since(since) : w.heldByTmdb,
      lines: [numbering.frozen ? w.whyFrozen : w.why],
      back: true,
    }
  }

  if (numbering.missing > 0) {
    return {
      tone: 'note',
      title: w.missing(numbering.missing),
      lines: [numbering.fallbackAfter > 0 ? w.missingNext : w.missingOff],
      back: false,
    }
  }
  return undefined
}

/**
 * What giving the numbering back did, said where the banner was: from the
 * numbering before, and the one the server answered with once the series
 * was refreshed.
 */
export function backSaid(before: Numbering | undefined, after: Numbering, w: Words): string {
  if (before?.droppedSince) {
    const into = after.mergedInto === undefined ? undefined : String(after.mergedInto)
    if (after.droppedSince) return into ? w.dropped.backFailedMerged(into) : w.dropped.backFailed
    return into ? w.dropped.backDoneMerged(into) : w.dropped.backDone
  }
  return after.heldByTmdb ? w.backFailed : w.backDone
}
