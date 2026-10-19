import { describe, expect, it } from 'vitest'

import type { Episode, PlacedEpisode } from '../../../lib/types'
import { orderName, orderNames, orderSeasons, placedEpisode } from './orders'

const words = { official: 'As aired', dvd: 'DVD', alternate: 'Alternate', alttwo: 'Alternate 2' }

const order = (key: string, label: string, source = 'tvdb') => ({ key, label, source, isDefault: false })

describe('an order’s name, as the pages say it', () => {
  it('says TheTVDB’s types in the reader’s words, and every other order by its own name', () => {
    expect(orderName(undefined, words)).toBe('As aired')
    expect(orderName({ ...order('official', 'Aired Order'), isDefault: true }, words)).toBe('As aired')
    expect(orderName(order('dvd', 'DVD Order'), words)).toBe('DVD')
    expect(orderName(order('alternate', 'Alternate Order'), words)).toBe('Alternate')
    // The series’ own name for one of TheTVDB’s slots says more than the slot.
    expect(orderName(order('alttwo', 'Superfan Order'), words)).toBe('Superfan Order')
    expect(orderName(order('tmdb-5acf93e60e0a26346d0000ce', 'Blu-ray Order', 'tmdb'), words)).toBe('Blu-ray Order')
    expect(orderName(order('ams-mine', 'Mine', 'local'), words)).toBe('Mine')
  })

  it('says where an order comes from only when another goes by the same name', () => {
    const sources = { tvdb: 'TheTVDB', tmdb: 'TMDB', local: 'made here' }
    const names = orderNames(
      [
        order('alttwo', 'Superfan Order'),
        order('tmdb-66bd170e3990ba12fee9990d', 'Superfan order', 'tmdb'),
        order('tmdb-694f4f2c9eb1c78ff3fb1098', 'Peacock', 'tmdb'),
        order('dvd', 'DVD Order'),
      ],
      words,
      sources,
    )
    expect([...names.values()]).toEqual(['Superfan Order · TheTVDB', 'Superfan order · TMDB', 'Peacock', 'DVD'])
  })

  it('lists an order’s seasons in order, the specials last', () => {
    const at = (seasonNumber: number) => ({ id: String(seasonNumber), seasonNumber, episodeNumber: 1 })
    expect(orderSeasons({ episodes: [at(2), at(0), at(1), at(2)] })).toEqual([1, 2, 0])
  })
})

describe('what a place of an order stands for', () => {
  const held: Episode = { id: 'row-11', seasonNumber: 1, episodeNumber: 1, title: 'Pilot', isManual: false }
  const byId = new Map([[held.id, held]])
  const place = (more: Partial<PlacedEpisode>): PlacedEpisode => ({ id: 'p', seasonNumber: 3, episodeNumber: 7, ...more })

  it('is the episode the work holds, by the id the server names it with', () => {
    expect(placedEpisode(place({ episodeId: 'row-11', tvdbId: 11 }), byId)).toEqual({ episode: held, own: false })
  })

  it('is the order’s own record of one the work does not hold, numbered by its place', () => {
    const found = placedEpisode(place({ tvdbId: 10718410, record: { title: 'Pilot (Extended Cut)', runtime: 38 } }), byId)
    expect(found?.own).toBe(true)
    expect(found?.episode).toMatchObject({
      id: 'p',
      seasonNumber: 3,
      episodeNumber: 7,
      title: 'Pilot (Extended Cut)',
      runtime: 38,
      tvdbId: 10718410,
    })
  })

  it('is nothing for an episode the work no longer holds, of which the order keeps no record', () => {
    expect(placedEpisode(place({ episodeId: 'gone', tvdbId: 99 }), byId)).toBeUndefined()
  })
})
