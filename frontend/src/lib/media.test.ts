import { describe, expect, it } from 'vitest'

import { episode, image, work } from './fixtures'
import {
  adjacentEpisodes,
  airTime,
  episodeCode,
  hasAired,
  hasSources,
  headlineRating,
  latestEpisode,
  nextEpisode,
  pick,
  poster,
  ratingsOf,
  seasonName,
  seasonNumbers,
  sized,
} from './media'

describe('an image in a role', () => {
  const tmdb = 'https://image.tmdb.org/t/p/original/abc.jpg'

  it('is fetched from TMDB’s ladder at the sizes it is drawn at', () => {
    const card = sized(tmdb, 'card')
    expect(card.src).toBe('https://image.tmdb.org/t/p/w342/abc.jpg')
    expect(card.srcSet).toBe(
      'https://image.tmdb.org/t/p/w185/abc.jpg 185w, https://image.tmdb.org/t/p/w342/abc.jpg 342w, https://image.tmdb.org/t/p/w500/abc.jpg 500w',
    )
    expect(card.original).toBe(tmdb)
  })

  it('reaches past the ladder where TMDB has a step beyond it', () => {
    expect(sized(tmdb, 'frame').srcSet).toContain('https://image.tmdb.org/t/p/original/abc.jpg 1920w')
  })

  it('is the original for the lightbox', () => {
    expect(sized(tmdb, 'full')).toEqual({ src: tmdb, original: tmdb })
  })

  it('is a copy kept here’s thumbnail, with the copy itself for a wide screen', () => {
    const kept = `/media/${'a'.repeat(64)}.png`
    const sourced = sized(kept, 'poster')
    expect(sourced.src).toBe(`/media/${'a'.repeat(64)}-t.jpg`)
    expect(sourced.srcSet).toBe(`/media/${'a'.repeat(64)}-t.jpg 480w, ${kept} 2000w`)
  })

  it('is TheTVDB’s thumbnail in the small roles, and its original elsewhere', () => {
    const tvdb = 'https://artworks.thetvdb.com/banners/posters/81189-1.jpg'
    expect(sized(tvdb, 'card').src).toBe('https://artworks.thetvdb.com/banners/posters/81189-1_t.jpg')
    expect(sized(tvdb, 'poster').src).toBe(tvdb)
  })

  it('is any other address as it was given', () => {
    expect(sized('https://example.org/a.jpg', 'card')).toEqual({ src: 'https://example.org/a.jpg', original: 'https://example.org/a.jpg' })
  })
})

describe('the image to show', () => {
  const images = [
    image('best'),
    image('manual', { isManual: true }),
    image('chosen'),
    image('season', { seasonNumber: 1 }),
    image('fanart', { coverType: 'fanart' }),
  ]

  it('is the one a person chose, then one added by hand, then the best offered', () => {
    expect(pick(images, 'poster', undefined, 'chosen')?.id).toBe('chosen')
    expect(pick(images, 'poster')?.id).toBe('manual')
    expect(pick(images.filter((one) => !one.isManual), 'poster')?.id).toBe('best')
  })

  it('is a season’s own, or the work’s, never the other’s', () => {
    expect(pick(images, 'poster', 1)?.id).toBe('season')
    expect(pick(images, 'poster', 2)).toBeUndefined()
  })

  it('leads a work with the poster chosen for it', () => {
    expect(poster(work({ images, primaryImages: { poster: 'chosen' } }))).toBe('https://example.org/chosen.jpg')
  })
})

describe('ratings', () => {
  const ratings = [
    { source: 'tmdb', value: 7.9, votes: 12_000 },
    { source: 'tvdb', value: 92_000 },
    { source: 'imdb', value: 9.3, votes: 400 },
    { source: 'trakt' },
    { source: 'metacritic', value: 8.8, votes: 50_000 },
  ]

  it('are marks out of ten alone, by votes, with IMDb’s first', () => {
    expect(ratingsOf(ratings).map((r) => r.source)).toEqual(['imdb', 'metacritic', 'tmdb'])
  })

  it('are led by the one Sonarr is given', () => {
    expect(headlineRating(ratings)?.source).toBe('imdb')
    expect(headlineRating(ratings.filter((r) => r.source !== 'imdb'))?.source).toBe('metacritic')
    expect(headlineRating(undefined)).toBeUndefined()
  })
})

describe('seasons and episodes', () => {
  const series = work({
    episodes: [episode(0, 1), episode(2, 1), episode(1, 2), episode(1, 1)],
    seasons: [{ id: 's3', seasonNumber: 3, isManual: false }],
  })

  it('are numbered as they are read, specials last', () => {
    expect(seasonNumbers(series)).toEqual([1, 2, 3, 0])
  })

  it('are written as every release name writes them', () => {
    expect(episodeCode({ seasonNumber: 1, episodeNumber: 2 })).toBe('S01E02')
    expect(episodeCode({ seasonNumber: 12, episodeNumber: 104 })).toBe('S12E104')
  })

  it('lead on to the next, across a season’s end', () => {
    const { before, after } = adjacentEpisodes(series, { seasonNumber: 1, episodeNumber: 2 })
    expect(before?.id).toBe('e1x1')
    expect(after?.id).toBe('e2x1')
  })

  it('are named by their own title where it is one, and by number where it is a placeholder', () => {
    const byNumber = (n: number) => `Season ${n}`
    expect(seasonName('The Final Act', 4, byNumber)).toBe('The Final Act')
    expect(seasonName('Season 4', 4, (n) => `Saison ${n}`)).toBe('Saison 4')
    expect(seasonName('Hors-série', 0, () => 'Specials')).toBe('Specials')
    expect(seasonName(undefined, 2, byNumber)).toBe('Season 2')
  })
})

describe('when an episode airs', () => {
  it('is a moment where a provider gave one, on the reader’s own day', () => {
    // 01:00 UTC on the 22nd is the evening of the 21st in Los Angeles.
    const time = airTime({ airDate: '2026-09-21', airDateUtc: '2026-09-22T01:00:00Z' })
    expect(time?.day).toBe('2026-09-21')
    expect(time?.moment?.toISOString()).toBe('2026-09-22T01:00:00.000Z')
  })

  it('is a day where only a date is known, the network’s, for every reader', () => {
    expect(airTime({ airDate: '2026-09-22' })).toEqual({ day: '2026-09-22' })
    // A bare date where a moment was expected — an override typed by hand.
    expect(airTime({ airDateUtc: '2026-09-22' })).toEqual({ day: '2026-09-22' })
    expect(airTime({ airDate: 'soon' })).toBeUndefined()
  })

  it('has aired once its moment is past, or once its day is over', () => {
    const evening = new Date('2026-09-22T02:00:00Z')
    expect(hasAired({ day: '2026-09-21', moment: new Date('2026-09-22T01:00:00Z') }, evening)).toBe(true)
    expect(hasAired({ day: '2026-09-21' }, evening)).toBe(false)
    expect(hasAired({ day: '2026-09-20' }, evening)).toBe(true)
  })

  it('makes tonight’s episode the next one until tonight is over', () => {
    const series = work({
      episodes: [
        episode(1, 1, { airDate: '2026-09-14' }),
        episode(1, 2, { airDate: '2026-09-21' }),
        episode(1, 3, { airDate: '2026-09-28' }),
        episode(0, 1, { airDate: '2026-09-22' }),
      ],
    })
    const tonight = new Date(2026, 8, 21, 20, 0)
    expect(nextEpisode(series, tonight)?.id).toBe('e1x2')
    expect(latestEpisode(series, tonight)?.id).toBe('e1x1')
  })
})

describe('a work’s sources', () => {
  it('are the identifiers a refresh can ask a provider by', () => {
    expect(hasSources(work())).toBe(false)
    expect(hasSources(work({ externalIds: { mal: [] } }))).toBe(false)
    expect(hasSources(work({ externalIds: { tmdb: 1399 } }))).toBe(true)
  })
})
