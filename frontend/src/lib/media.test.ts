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
  lookOf,
  nextEpisode,
  offerWebp,
  pick,
  poster,
  ratingsOf,
  seasonName,
  seasonNumbers,
  sized,
  type ImageRole,
  type Picture,
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

  describe('a copy kept here', () => {
    const stem = `/media/${'a'.repeat(64)}`
    const kept = `${stem}.png`
    const at = (widths: number[], format = 'webp') => widths.map((w) => `${stem}-${w}.${format} ${w}w`).join(', ')

    describe('with the WebP copies on', () => {
      const webp = (role: ImageRole, look?: Picture) => sized(kept, role, look, true)

      it('is offered at its role’s widths, as WebP, the original kept to fall back on', () => {
        const sourced = webp('poster')
        expect(sourced.src).toBe(`${stem}-480.webp`)
        expect(sourced.srcSet).toBe(at([240, 480, 960]))
        expect(sourced.avif).toBeUndefined()
        expect(sourced.original).toBe(kept)
        expect(webp('backdrop').srcSet).toBe(at([640, 1280, 1920]))
        expect(webp('still').srcSet).toBe(at([320, 640]))
        expect(webp('headshot').srcSet).toBe(at([120, 240]))
      })

      it('is never offered wider than 1920, the lightbox’s included', () => {
        expect(webp('full').srcSet).toBe(at([640, 1280, 1920]))
      })

      it('is never offered wider than it is, and offered at its own width', () => {
        const sourced = webp('card', { width: 680, height: 1000 })
        expect(sourced.srcSet).toBe(at([240, 480, 680]))
        expect([sourced.width, sourced.height]).toEqual([480, 706])
        expect(webp('card', { width: 200, height: 300 }).srcSet).toBe(at([200]))
      })

      it('offers its AVIF copies where they reach as wide as the WebP', () => {
        const poster = { width: 2000, height: 3000, avif: [480, 960] }
        expect(webp('card', poster).avif).toBe(at([480, 960], 'avif'))
        expect(webp('thumb', poster).avif).toBe(at([480], 'avif'))
        // A background's 1280 would draw a dense screen's 1920 soft.
        expect(webp('backdrop', { width: 3840, height: 2160, avif: [1280] }).avif).toBeUndefined()
        expect(webp('backdrop', { width: 1280, height: 720, avif: [1280] }).avif).toBe(at([1280], 'avif'))
      })
    })

    describe('with the WebP copies off', () => {
      const whole = (role: ImageRole, look?: Picture) => sized(kept, role, look, false)

      it('is the picture itself, its own size giving the box its proportions', () => {
        expect(whole('poster', { width: 2000, height: 3000 })).toEqual({ src: kept, width: 2000, height: 3000, original: kept })
        expect(whole('backdrop', { width: 3840, height: 2160 })).toEqual({ src: kept, width: 3840, height: 2160, original: kept })
        expect(whole('card')).toEqual({ src: kept, original: kept })
      })

      it('is offered as AVIF over the picture where those copies reach as wide as its role is drawn', () => {
        const poster = { width: 2000, height: 3000, avif: [480, 960] }
        expect(whole('card', poster)).toEqual({
          src: kept,
          sizes: sized(kept, 'card', poster, true).sizes,
          avif: at([480, 960], 'avif'),
          width: 2000,
          height: 3000,
          original: kept,
        })
        expect(whole('thumb', poster).avif).toBe(at([480], 'avif'))
        expect(whole('backdrop', { width: 1280, height: 720, avif: [1280] }).avif).toBe(at([1280], 'avif'))
        // Not as wide: the picture alone, with no width to choose among.
        const backdrop = whole('backdrop', { width: 3840, height: 2160, avif: [1280] })
        expect(backdrop).toEqual({ src: kept, width: 3840, height: 2160, original: kept })
      })

      it('is what the server said last, where the caller does not say', () => {
        expect(sized(kept, 'card').src).toBe(kept)
        offerWebp(true)
        try {
          expect(sized(kept, 'card').srcSet).toBe(at([240, 480, 960]))
        } finally {
          offerWebp(false)
        }
        expect(sized(kept, 'card').srcSet).toBeUndefined()
      })
    })

    it('is found by its address once a work has said what it looks like', () => {
      const look = { width: 1000, height: 1500, color: '#14285a', thumbhash: 'AQIDBAU=' }
      const item = work({ images: [image('p', { url: `${stem}.jpg` })], pictures: { [`${stem}.jpg`]: look } })
      expect(poster(item)).toBe(`${stem}.jpg`)
      expect(lookOf(`${stem}.jpg`)).toEqual(look)
      expect(sized(`${stem}.jpg`, 'card', undefined, true).srcSet).toBe(at([240, 480, 960]))
      expect(sized(`${stem}.jpg`, 'poster', undefined, true).width).toBe(480)
      expect(sized(`${stem}.jpg`, 'poster', undefined, false).width).toBe(1000)
    })
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
