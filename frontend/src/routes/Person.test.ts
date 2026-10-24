import { describe, expect, it } from 'vitest'

import { work } from '../lib/fixtures'
import type { Role } from '../lib/types'
import { ageBetween, jobsOf, linesOf, mostWorked, ordered, yearSpan } from './Person'

/** A credit as a person's page lists it: on a work, or on one episode of it. */
function role(workId: string, creditType: Role['creditType'], over: Partial<Role> = {}): Role {
  return { workId, creditType, creditId: `${workId}:${creditType}:${over.episodeNumber ?? 0}`, ...over }
}

const breakingBad = work({ id: 'bb', title: 'Breaking Bad', kind: 'series', year: 2008 })
const malcolm = work({ id: 'mitm', title: 'Malcolm in the Middle', kind: 'series', year: 2000 })
const drive = work({ id: 'drive', title: 'Drive', kind: 'movie', year: 2011 })

describe('the jobs on a person’s page', () => {
  it('count works, not credits, and put a guest with the cast', () => {
    const jobs = jobsOf([
      role('bb', 'actor'),
      role('bb', 'producer'),
      role('mitm', 'guest', { seasonNumber: 1, episodeNumber: 1 }),
      role('mitm', 'guest', { seasonNumber: 1, episodeNumber: 2 }),
      role('bb', 'director', { seasonNumber: 3, episodeNumber: 1 }),
      role('bb', 'director', { seasonNumber: 4, episodeNumber: 1 }),
    ])
    expect(jobs).toEqual([
      { job: 'acting', works: 2 },
      { job: 'directing', works: 1 },
      { job: 'producing', works: 1 },
    ])
  })

  it('are drawn in one order whatever order the credits came in', () => {
    expect(jobsOf([role('a', 'writer'), role('b', 'actor')]).map((entry) => entry.job)).toEqual(['acting', 'writing'])
  })

  it('open on the one with the most works, the first of those with as many', () => {
    expect(mostWorked([{ job: 'acting', works: 1 }, { job: 'directing', works: 3 }])).toBe('directing')
    expect(mostWorked([{ job: 'acting', works: 2 }, { job: 'producing', works: 2 }])).toBe('acting')
    expect(mostWorked([])).toBeUndefined()
  })
})

describe('a tab’s lines', () => {
  const person = {
    works: [drive, breakingBad, malcolm],
    roles: [
      role('drive', 'actor', { character: 'Shannon' }),
      role('bb', 'actor', { character: 'Walter White' }),
      role('mitm', 'guest', { seasonNumber: 2, episodeNumber: 5, airDate: '2001-02-04', episodeTitle: 'Lois’ Birthday' }),
      role('mitm', 'guest', { seasonNumber: 1, episodeNumber: 3, airDate: '2000-01-23', episodeTitle: 'Home Alone 4' }),
      role('bb', 'producer', { character: 'Producer' }),
    ],
  }

  it('hold each work the job was done on once, in the order the works came', () => {
    expect(linesOf(person, 'acting').map((line) => line.work.id)).toEqual(['drive', 'bb', 'mitm'])
    expect(linesOf(person, 'producing').map((line) => line.work.id)).toEqual(['bb'])
    expect(linesOf(person, 'writing')).toEqual([])
  })

  it('gather a series’ episodes under it, in the order of the series, apart from the credits on the whole', () => {
    const [series] = linesOf(person, 'acting').filter((line) => line.work.id === 'mitm')
    expect(series?.roles).toEqual([])
    expect(series?.episodes.map((episode) => episode.episodeNumber)).toEqual([3, 5])
  })

  it('are set against the last year their episodes aired in, else the work’s own', () => {
    const years = Object.fromEntries(linesOf(person, 'acting').map((line) => [line.work.id, line.year]))
    expect(years).toEqual({ drive: 2011, bb: 2008, mitm: 2001 })
  })

  it('come the newest first, by title, or the best rated first, the newer first where two are alike', () => {
    const rated = {
      ...person,
      works: [
        work({ id: 'drive', title: 'Drive', kind: 'movie', year: 2011, ratings: [{ source: 'imdb', value: 7.8, votes: 700000 }] }),
        work({ id: 'bb', title: 'breaking bad', kind: 'series', year: 2008, ratings: [{ source: 'imdb', value: 9.5, votes: 2000000 }] }),
        work({ id: 'mitm', title: 'Malcolm in the Middle', kind: 'series', year: 2000 }),
      ],
    }
    const lines = linesOf(rated, 'acting')
    const ids = (order: 'year' | 'title' | 'rating') => ordered(lines, order, 'en-GB').map((line) => line.work.id)
    expect(ids('year')).toEqual(['drive', 'bb', 'mitm'])
    // A title's case is no order of its own.
    expect(ids('title')).toEqual(['bb', 'drive', 'mitm'])
    expect(ids('rating')).toEqual(['bb', 'drive', 'mitm'])
  })
})

describe('the years a run of episodes aired in', () => {
  it('are the first and the last, or the one they all did', () => {
    expect(yearSpan([{ airDate: '2022-03-01' }, { airDate: '2019-10-12' }, { airDate: '2020-01-01' }])).toBe('2019–2022')
    expect(yearSpan([{ airDate: '2019-10-12' }, { airDate: '2019-11-02' }])).toBe('2019')
  })

  it('are nothing where no episode is dated', () => {
    expect(yearSpan([{}, { airDate: 'soon' }])).toBeUndefined()
  })
})

describe('an age', () => {
  it('counts whole years, a birthday not yet come this year not counted', () => {
    expect(ageBetween('1956-03-07', '2026-03-06')).toBe(69)
    expect(ageBetween('1956-03-07', '2026-03-07')).toBe(70)
  })

  it('is nothing for a day that is not one, or a death before the birth', () => {
    expect(ageBetween('not a day')).toBeUndefined()
    expect(ageBetween('2000-01-01', '1999-01-01')).toBeUndefined()
  })
})
