import { describe, expect, it } from 'vitest'

import { en } from '../../../lib/lang/en'
import { fr } from '../../../lib/lang/fr'
import type { Orders } from '../../../lib/types'
import { backSaid, numberingSaid } from './numbering'

type Numbering = Orders['numbering']

const plain: Numbering = { heldByTmdb: false, silent: 0, fallbackAfter: 3, missing: 0, offered: [] }

const w = en.work.tmdbNumbering

describe('what the numbering banner says of a series', () => {
  it('says nothing of a series TheTVDB numbers and has said nothing amiss of', () => {
    expect(numberingSaid(undefined, w, 'en')).toBeUndefined()
    expect(numberingSaid(plain, w, 'en')).toBeUndefined()
  })

  it('says since when TMDB numbers a series TheTVDB stopped answering for', () => {
    const said = numberingSaid({ ...plain, heldByTmdb: true, fallbackSince: '2026-10-01T08:00:00Z', silent: 3 }, w, 'en')
    expect(said?.tone).toBe('held')
    expect(said?.title).toBe('Numbered by TMDB since October 1, 2026')
    expect(said?.lines).toEqual([w.why])
    expect(said?.back).toBe(true)
  })

  it('says why a series TheTVDB no longer knows is served as it was, and how the page numbers it', () => {
    const deleted = numberingSaid(
      {
        ...plain,
        heldByTmdb: true,
        droppedSince: '2026-11-02T10:00:00Z',
        droppedWhy: 'deleted',
        missing: 0,
        frozen: 'tvdb-last',
      },
      w,
      'en',
    )
    expect(deleted?.title).toBe(
      'TheTVDB deleted this series: it is served numbered as it was on November 2, 2026, the episodes aired since placed after it',
    )
    expect(deleted?.lines).toEqual([w.dropped.served, w.dropped.listedByTmdb])
    expect(deleted?.caution).toBeUndefined()

    // Merged into another that TheTVDB numbers it under: the page lists it
    // so, and the way back is cautioned.
    const merged = numberingSaid(
      { ...plain, droppedSince: '2026-11-02T10:00:00Z', droppedWhy: 'merged', mergedInto: 81189, frozen: 'tvdb-last' },
      w,
      'en',
    )
    expect(merged?.title).toContain('merged this series into series 81189')
    expect(merged?.lines).toEqual([w.dropped.served, w.dropped.listedByMerged('81189')])
    expect(merged?.caution).toBe(w.dropped.mergedCaution('81189'))

    // Answered twice that it has none, and the frozen order thawed since.
    const missing = numberingSaid(
      { ...plain, heldByTmdb: true, droppedSince: '2026-11-02T10:00:00Z', droppedWhy: 'missing', missing: 2 },
      fr.work.tmdbNumbering,
      'fr',
    )
    expect(missing?.title).toBe(
      'TheTVDB a répondu deux fois qu’il n’a pas cette série : elle est servie numérotée comme elle l’était le 2 novembre 2026, les épisodes diffusés depuis placés à la suite',
    )
    expect(missing?.lines[0]).toBe(fr.work.tmdbNumbering.dropped.thawed)
  })

  it('tells whoever keeps the catalogue, quietly, that TheTVDB said once it has no such series', () => {
    const once = numberingSaid({ ...plain, missing: 1 }, w, 'en')
    expect(once).toMatchObject({ tone: 'note', title: 'TheTVDB answered once that it has no such series', back: false })
    expect(once?.lines).toEqual([w.missingNext])
    // With no fallback at all, nothing will follow.
    expect(numberingSaid({ ...plain, missing: 4, fallbackAfter: 0 }, w, 'en')?.lines).toEqual([w.missingOff])
  })

  it('says what giving the numbering back did', () => {
    const dropped: Numbering = { ...plain, droppedSince: '2026-11-02T10:00:00Z', droppedWhy: 'deleted' }
    expect(backSaid(dropped, dropped, w)).toBe(w.dropped.backFailed)
    expect(backSaid(dropped, plain, w)).toBe(w.dropped.backDone)
    const merged: Numbering = { ...dropped, droppedWhy: 'merged', mergedInto: 81189 }
    expect(backSaid(merged, merged, w)).toBe(w.dropped.backFailedMerged('81189'))
    // The id it was merged into is still said once the numbering is back.
    expect(backSaid(merged, { ...plain, mergedInto: 81189 }, w)).toBe(w.dropped.backDoneMerged('81189'))
    // TMDB's fallback, as before.
    const held: Numbering = { ...plain, heldByTmdb: true }
    expect(backSaid(held, held, w)).toBe(w.backFailed)
    expect(backSaid(held, plain, w)).toBe(w.backDone)
  })
})
