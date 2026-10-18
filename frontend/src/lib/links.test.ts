import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { work } from './fixtures'
import { identifierLink, isWebAddress, relationLink, sitePath, trailerEmbed, workLinks } from './links'

describe('an identifier', () => {
  it('leads to the work’s page on the site that gave it', () => {
    expect(identifierLink('tmdb', 1399, 'series')).toBe('https://www.themoviedb.org/tv/1399')
    expect(identifierLink('tmdb', 603, 'movie')).toBe('https://www.themoviedb.org/movie/603')
    expect(identifierLink('imdb', 'tt0944947', 'series')).toBe('https://www.imdb.com/title/tt0944947/')
  })

  it('leads nowhere rather than to the wrong page', () => {
    expect(identifierLink('tvdb', 1, 'movie')).toBeUndefined()
    expect(identifierLink('fankai', 12, 'series')).toBeUndefined()
    expect(identifierLink('wikidata', 'not an item', 'movie')).toBeUndefined()
    expect(identifierLink('trakt', 1, 'series')).toBeUndefined()
  })

  it('stays inside its segment', () => {
    expect(identifierLink('imdb', 'tt1/../x', 'movie')).toBe('https://www.imdb.com/title/tt1%2F..%2Fx/')
  })
})

describe('a work’s pages elsewhere', () => {
  it('come in the identifiers panel’s order, the first of several entries standing for the work', () => {
    const links = workLinks(work({ externalIds: { anilist: [21, 22], imdb: 'tt1', tmdb: 2, mal: [] } }))
    expect(links).toEqual([
      { source: 'tmdb', href: 'https://www.themoviedb.org/tv/2' },
      { source: 'imdb', href: 'https://www.imdb.com/title/tt1/' },
      { source: 'anilist', href: 'https://anilist.co/anime/21' },
    ])
  })
})

describe('an address a browser may be sent to', () => {
  it('is http or https, and nothing else', () => {
    expect(isWebAddress('https://example.org/')).toBe(true)
    expect(isWebAddress('http://example.org/')).toBe(true)
    expect(isWebAddress('javascript:alert(1)')).toBe(false)
    expect(isWebAddress('data:text/html,hi')).toBe(false)
    expect(isWebAddress('not an address')).toBe(false)
  })
})

describe('a page of this site to go back to', () => {
  beforeEach(() => vi.stubGlobal('window', { location: { origin: 'https://cine.example' } }))
  afterEach(() => vi.unstubAllGlobals())

  it('is a path, with its query and its fragment', () => {
    expect(sitePath('/work/1?tab=cast#seasons')).toBe('/work/1?tab=cast#seasons')
    expect(sitePath('/')).toBe('/')
  })

  it('is never another host, however it is spelled', () => {
    for (const next of [
      '//evil.example',
      '/\\evil.example',
      '/\tevil.example',
      '/./evil.example',
      '/../admin',
      '/%2e%2e/admin',
      'https://evil.example/',
      'evil.example',
    ]) {
      expect(sitePath(next), next).toBeUndefined()
    }
  })

  it('is nothing when none was asked for', () => {
    expect(sitePath(null)).toBeUndefined()
    expect(sitePath(undefined)).toBeUndefined()
    expect(sitePath('')).toBeUndefined()
  })
})

describe('the other links', () => {
  it('find a related work where it is filed', () => {
    expect(relationLink({ source: 'mal', externalId: 5114, medium: 'manga' })).toBe('https://myanimelist.net/manga/5114')
    expect(relationLink({ source: 'anilist', externalId: 1, medium: 'anime' })).toBe('https://anilist.co/anime/1')
    expect(relationLink({ source: 'fankai', externalId: 7, medium: 'anime' })).toBe('https://fankai.fr/productions/7')
  })

  it('play a trailer from YouTube’s no-cookie domain', () => {
    expect(trailerEmbed('a b')).toBe('https://www.youtube-nocookie.com/embed/a%20b?autoplay=1&rel=0')
  })
})
