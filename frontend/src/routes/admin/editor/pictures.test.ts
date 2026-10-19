import { describe, expect, it } from 'vitest'

import { image, work } from '../../../lib/fixtures'
import { seasonPictures } from './pictures'

describe('a season’s pictures, as the editor lists them', () => {
  const held = work({
    images: [image('work-poster'), image('s2-poster', { seasonNumber: 2 })],
    seasons: [
      { id: 's1', seasonNumber: 1, images: [image('s1-poster', { seasonNumber: 1 }), image('s1-banner', { seasonNumber: 1, coverType: 'banner' })], isManual: false },
      { id: 's2', seasonNumber: 2, images: [], isManual: false },
    ],
    hidden: {
      images: [image('s1-hidden', { seasonNumber: 1, coverType: 'fanart' }), image('work-hidden'), image('s2-hidden', { seasonNumber: 2 })],
    },
  })

  it('lists what the season is served with, then what a person hid of it', () => {
    expect(seasonPictures(held, 1).map(({ image, hidden }) => [image.id, hidden])).toEqual([
      ['s1-poster', false],
      ['s1-banner', false],
      ['s1-hidden', true],
    ])
  })

  it('takes a season’s picture filed on the work by its number, and nothing of the other seasons or the work', () => {
    expect(seasonPictures(held, 2).map(({ image, hidden }) => [image.id, hidden])).toEqual([
      ['s2-poster', false],
      ['s2-hidden', true],
    ])
  })

  it('is empty for a season with nothing, hidden or shown', () => {
    expect(seasonPictures(held, 3)).toEqual([])
    expect(seasonPictures(work(), 1)).toEqual([])
  })
})
