import { afterEach, describe, expect, it, vi } from 'vitest'

import { feeds, webcal } from './feeds'

describe('the feeds', () => {
  it('are paths on this origin, in the language asked for when one is', () => {
    expect(feeds.calendar()).toBe('/api/v1/calendar.ics')
    expect(feeds.calendar('fr')).toBe('/api/v1/calendar.ics?language=fr')
    expect(feeds.added).toBe('/api/v1/feed/added.atom')
  })

  it('keep a work’s id inside its segment', () => {
    expect(feeds.work('a b/c', 'en')).toBe('/api/v1/items/a%20b%2Fc/calendar.ics?language=en')
  })
})

describe('a calendar subscription', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('is webcals: where the page is https:', () => {
    vi.stubGlobal('window', { location: { protocol: 'https:', host: 'cine.example:8443' } })
    expect(webcal(feeds.calendar())).toBe('webcals://cine.example:8443/api/v1/calendar.ics')
  })

  it('is webcal: where the page is http:', () => {
    vi.stubGlobal('window', { location: { protocol: 'http:', host: '192.168.1.20:8080' } })
    expect(webcal('/api/v1/calendar.ics')).toBe('webcal://192.168.1.20:8080/api/v1/calendar.ics')
  })
})
