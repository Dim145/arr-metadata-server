import { describe, expect, it } from 'vitest'

import { work } from './fixtures'
import { isStreamingPlatform, releaseDay, releasedNotAired } from './platforms'
import { certificationIn, cinemaDayIn, countryCode, readerRegion, releasesIn } from './releases'

describe('a reader’s country', () => {
  it('is a two-letter code, upper case, or nothing', () => {
    expect(countryCode(' fr ')).toBe('FR')
    expect(countryCode('FRA')).toBeUndefined()
    expect(countryCode(null)).toBeUndefined()
  })

  it('is the one they picked, the one answered for, or the language’s', () => {
    const me = { identity: 'x', canWrite: false, isAdmin: false, publicBrowse: true, region: 'be' }
    expect(readerRegion(me, 'fr-FR', 'CA')).toBe('CA')
    expect(readerRegion(me, 'fr-FR', undefined)).toBe('BE')
    expect(readerRegion(undefined, 'fr-FR', undefined)).toBe('FR')
    expect(readerRegion(undefined, 'en', undefined)).toBe('US')
  })
})

describe('a film’s releases in a country', () => {
  const film = work({
    kind: 'movie',
    contentRating: 'PG-13',
    contentRatingCountry: 'US',
    releases: [
      { country: 'FR', releaseType: 'digital', date: '2010-12-01', certification: 'TP' },
      { country: 'fr', releaseType: 'theatrical', date: '2010-07-21', certification: 'Tous publics' },
      { country: 'FR', certification: '12' },
      { country: 'FR', releaseType: 'premiere', date: '2010-07-08' },
      { country: 'US', releaseType: 'theatrical', date: '2010-07-16', certification: 'PG-13' },
    ],
  })

  it('come earliest first, a rating with no day after the dated ones', () => {
    expect(releasesIn(film, 'FR').map((r) => r.date ?? r.certification)).toEqual(['2010-07-08', '2010-07-21', '2010-12-01', '12'])
  })

  it('give the rating of the cinemas before the rest', () => {
    expect(certificationIn(film, 'FR')).toBe('Tous publics')
  })

  it('give the record’s own rating where it is that country’s', () => {
    expect(certificationIn(film, 'US')).toBe('PG-13')
  })

  it('open in cinemas on the first theatrical day, not at a festival', () => {
    expect(cinemaDayIn(film, 'FR')).toBe('2010-07-21')
    expect(cinemaDayIn(film, 'DE')).toBeUndefined()
  })
})

describe('a streaming platform', () => {
  it('is known by its whole name, not a fragment of a longer one', () => {
    expect(isStreamingPlatform('Amazon Prime Video')).toBe(true)
    expect(isStreamingPlatform('HBO Max')).toBe(true)
    expect(isStreamingPlatform('Maxx')).toBe(false)
    expect(isStreamingPlatform('AMC')).toBe(false)
    expect(isStreamingPlatform(undefined)).toBe(false)
  })

  it('puts a season up on a day: its midnight UTC is a release, not a broadcast', () => {
    const midnight = { day: '2016-07-15', moment: new Date('2016-07-16T00:00:00Z') }
    expect(releasedNotAired('Netflix', midnight)).toBe(true)
    expect(releasedNotAired('AMC', midnight)).toBe(false)
    expect(releasedNotAired('Netflix', { day: '2016-07-15', moment: new Date('2016-07-16T01:00:00Z') })).toBe(false)
    // The day that midnight opens, for a reader in Los Angeles too.
    expect(releaseDay(midnight)).toBe('2016-07-16')
  })
})
