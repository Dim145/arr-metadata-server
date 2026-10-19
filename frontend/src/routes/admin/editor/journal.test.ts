import { describe, expect, it } from 'vitest'

import type { FieldRegistry } from '../../../lib/types'
import { episodeCode, kindOf, period, placeOf, revertible, shown } from './journal'

const words = { yes: 'Yes', no: 'No' }

describe('where a change of the journal is', () => {
  it('reads the work, a season, an episode and a child a lock names', () => {
    expect(placeOf('item')).toEqual({ kind: 'item' })
    expect(placeOf('season:2')).toEqual({ kind: 'season', season: 2 })
    expect(placeOf('episode:1x05')).toEqual({ kind: 'episode', season: 1, episode: 5 })
    expect(placeOf('episode:0x3')).toEqual({ kind: 'episode', season: 0, episode: 3 })
    expect(placeOf('credit:actor:tmdb:17419')).toEqual({ kind: 'child', scope: 'credit:actor:tmdb:17419' })
  })

  it('writes an episode as a release log does', () => {
    expect(episodeCode(1, 5)).toBe('S01E05')
    expect(episodeCode(12, 104)).toBe('S12E104')
  })
})

describe('a value, as a line of the journal says it', () => {
  it('says nothing of nothing', () => {
    for (const nothing of [null, undefined, '', []]) expect(shown(nothing, words)).toBeUndefined()
  })

  it('says a text, a number, a flag and a list as they read', () => {
    expect(shown('Ended', words)).toBe('Ended')
    expect(shown(47, words)).toBe('47')
    expect(shown(true, words)).toBe('Yes')
    expect(shown(false, words)).toBe('No')
    expect(shown(['Drama', 'Crime'], words)).toBe('Drama, Crime')
  })

  it('says where an episode moved to by its code, its absolute number beside it', () => {
    expect(shown({ season: 2, episode: 1 }, words)).toBe('S02E01')
    expect(shown({ season: 1, episode: 5, absolute: 5 }, words)).toBe('S01E05 · 5')
  })

  it('says an episode that came or went by its title and its day, and nothing when it had neither', () => {
    expect(shown({ title: 'Pilot', airDate: '2008-01-20' }, words)).toBe('Pilot · 2008-01-20')
    expect(shown({ title: 'Pilot' }, words)).toBe('Pilot')
    expect(shown({}, words)).toBeUndefined()
  })

  it('says a credit, a picture and another title that came or went by a few words of each', () => {
    expect(shown({ personName: 'Dominic West', characterName: 'McNulty', creditType: 'actor' }, words)).toBe('Dominic West · McNulty')
    expect(shown({ coverType: 'poster', seasonNumber: 7 }, words)).toBe('poster · S07')
    expect(shown({ coverType: 'fanart' }, words)).toBe('fanart')
    expect(shown({ title: 'Sur écoute', language: 'fra' }, words)).toBe('Sur écoute · fra')
  })

  it('says any other object as its JSON', () => {
    expect(shown({ tmdb: 1396, imdb: 'tt0903747' }, words)).toBe('{"tmdb":1396,"imdb":"tt0903747"}')
  })
})

describe('a child that came or went', () => {
  it('is told by the field of its own scope', () => {
    expect(kindOf('season:7', 'season')).toBe('season')
    expect(kindOf('episode:7x1', 'episode')).toBe('episode')
    expect(kindOf('image:https://images.example/p.jpg', 'image')).toBe('image')
    expect(kindOf('credit:actor:name:Dominic West', 'credit')).toBe('credit')
    expect(kindOf('alternativeTitle:fra:Sur écoute', 'alternativeTitle')).toBe('alternativeTitle')
  })

  it('is not a value of a child: an episode’s still, a credit hidden, a season named', () => {
    expect(kindOf('episode:1x2', 'image')).toBeUndefined()
    expect(kindOf('credit:actor:tmdb:1', 'hidden')).toBeUndefined()
    expect(kindOf('season:1', 'title')).toBeUndefined()
    expect(kindOf('item', 'season')).toBeUndefined()
  })
})

describe('what can be gone back to', () => {
  const def = (name: string) => ({ name, fieldType: 'text', label: name }) as FieldRegistry['item'][number]
  const registry: FieldRegistry = {
    item: [def('overview'), def('genres')],
    season: [def('title')],
    episode: [def('title'), def('airDate')],
    translated: ['title', 'overview'],
  }

  it('is a field a lock can hold, wherever it is', () => {
    expect(revertible('item', 'overview', registry)).toBe(true)
    expect(revertible('season:1', 'title', registry)).toBe(true)
    expect(revertible('episode:1x2', 'airDate', registry)).toBe(true)
  })

  it('is not an episode that moved, came or went, nor a child, nor what the scope has no field of', () => {
    expect(revertible('episode:1x2', 'number', registry)).toBe(false)
    expect(revertible('episode:1x2', 'episode', registry)).toBe(false)
    expect(revertible('season:3', 'season', registry)).toBe(false)
    expect(revertible('credit:actor:tmdb:1', 'hidden', registry)).toBe(false)
    expect(revertible('season:1', 'airDate', registry)).toBe(false)
  })
})

describe('the period a journal is read over', () => {
  it('runs from the first day to the morning after the last', () => {
    expect(period('2026-10-01', '2026-10-19')).toEqual({ since: '2026-10-01', before: '2026-10-20' })
    expect(period('', '2026-12-31')).toEqual({ since: undefined, before: '2027-01-01' })
    expect(period('2026-02-28', '')).toEqual({ since: '2026-02-28', before: undefined })
  })

  it('leaves out what is no day', () => {
    expect(period('yesterday', '19/10/2026')).toEqual({ since: undefined, before: undefined })
  })
})
