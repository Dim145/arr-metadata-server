import { describe, expect, it } from 'vitest'

import type { Person } from '../../lib/types'
import { enteredOf, entryOf, idsOf } from './People'

/** A record as its page sends it to whoever keeps the catalogue. */
function person(over: Partial<Person> = {}): Person {
  return { id: 'p1', name: 'Bryan Cranston', roles: [], works: [], ...over }
}

describe('a record, as its form starts it', () => {
  it('holds the record as it is kept, and of the other names only those typed by hand', () => {
    const entry = entryOf(
      person({
        image: `/media/${'a'.repeat(64)}.jpg`,
        aliases: ['Bryan Lee Cranston', 'Walt'],
        record: {
          id: 'p1',
          name: 'Bryan Cranston',
          sortName: 'Cranston, Bryan',
          tmdbId: 17419,
          imdbId: 'nm0186505',
          gender: 2,
          image: 'https://image.tmdb.org/t/p/original/bryan.jpg',
          isManual: true,
          createdAt: '2026-10-01T00:00:00Z',
          updatedAt: '2026-10-01T00:00:00Z',
          aliases: [
            { name: 'Bryan Lee Cranston', source: 'credit' },
            { name: 'Walt', source: 'manual' },
          ],
        },
      }),
    )
    expect(entry).toMatchObject({
      sortName: 'Cranston, Bryan',
      tmdbId: '17419',
      imdbId: 'nm0186505',
      gender: '2',
      // The source's address, which a record takes, not this server's copy.
      image: 'https://image.tmdb.org/t/p/original/bryan.jpg',
      aliases: 'Walt',
    })
  })

  it('holds what the page says of them where no record comes with it, a name a line for the other names', () => {
    const entry = entryOf(
      person({
        tmdbId: 17419,
        tvdbId: 314,
        originalName: 'ブライアン・クランストン',
        aliases: ['Bryan Lee Cranston', 'Bryan Cranstan'],
        details: { imdbId: 'nm0186505', gender: 2, birthday: '1956-03-07', placeOfBirth: 'Hollywood' },
      }),
    )
    expect(entry).toMatchObject({
      name: 'Bryan Cranston',
      tmdbId: '17419',
      tvdbId: '314',
      imdbId: 'nm0186505',
      gender: '2',
      birthday: '1956-03-07',
      aliases: 'Bryan Lee Cranston\nBryan Cranstan',
    })
  })

  it('leaves out a portrait this server keeps, which a record does not take back', () => {
    expect(entryOf(person({ image: `/media/${'a'.repeat(64)}.jpg` })).image).toBe('')
    expect(entryOf(person({ image: 'https://image.tmdb.org/t/p/original/x.jpg' })).image).toBe(
      'https://image.tmdb.org/t/p/original/x.jpg',
    )
  })

  it('is empty for somebody entered by hand', () => {
    expect(Object.values(entryOf()).every((value) => value === '')).toBe(true)
  })
})

describe('a record, as the form sends it', () => {
  const empty = entryOf()

  it('leaves out a field left empty, which clears it', () => {
    const sent = enteredOf({ ...empty, name: '  Jane Doe ', placeOfBirth: '   ' })
    expect(sent.name).toBe('Jane Doe')
    expect(sent.placeOfBirth).toBeUndefined()
    expect(JSON.parse(JSON.stringify(sent))).toEqual({ name: 'Jane Doe', aliases: [] })
  })

  it('sends ids as numbers, and nothing for one that is not', () => {
    expect(enteredOf({ ...empty, name: 'A', tmdbId: '17419', tvdbId: 'x1' })).toMatchObject({ tmdbId: 17419, tvdbId: undefined })
  })

  it('sends each other name once, a line each, blank lines aside', () => {
    expect(enteredOf({ ...empty, name: 'A', aliases: 'B\n\n C \nB' }).aliases).toEqual(['B', 'C'])
  })

  it('sends the gender as TMDB counts it, and nothing where it is not said', () => {
    expect(enteredOf({ ...empty, name: 'A', gender: '3' }).gender).toBe(3)
    expect(enteredOf({ ...empty, name: 'A' }).gender).toBeUndefined()
  })
})

describe('the ids somebody is known by', () => {
  it('are said in a line, those they lack left out', () => {
    expect(idsOf({ tmdbId: 17419, imdbId: 'nm0186505', tvdbId: 314 })).toBe('TMDB 17419 · IMDb nm0186505 · TheTVDB 314')
    expect(idsOf({ imdbId: 'nm0186505' })).toBe('IMDb nm0186505')
    expect(idsOf({})).toBe('')
  })
})
