import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiError, api, inTextLanguage, query, segment } from './api'

describe('a query string', () => {
  it('leaves out what is empty, and keeps a nought', () => {
    expect(query({ language: 'fr', term: '', adult: false, page: 0, kind: undefined, all: true })).toBe(
      '?language=fr&page=0&all=true',
    )
    expect(query({ term: undefined })).toBe('')
  })
})

describe('a read, in the language the reader chose for the texts', () => {
  it('asks in that language what was asked in the interface’s', () => {
    expect(inTextLanguage('/items/w1?language=en', 'de', '/work/w1')).toBe('/items/w1?language=de')
    expect(inTextLanguage('/calendar?from=a&to=b&language=fr', 'pt-BR', '/calendar')).toBe(
      '/calendar?from=a&to=b&language=pt-BR',
    )
  })

  it('leaves alone a read that asks for no language, and every read where nothing was chosen', () => {
    expect(inTextLanguage('/items/w1/orders', 'de', '/work/w1')).toBe('/items/w1/orders')
    expect(inTextLanguage('/facets?kind=series', 'de', '/browse')).toBe('/facets?kind=series')
    expect(inTextLanguage('/items/w1?language=en', undefined, '/work/w1')).toBe('/items/w1?language=en')
  })

  it('leaves the administration in the language it edits in', () => {
    expect(inTextLanguage('/items/w1?language=fr', 'de', '/admin/catalogue/w1')).toBe('/items/w1?language=fr')
  })
})

describe('a path segment', () => {
  it('keeps a slash, a question mark or a hash inside it', () => {
    expect(segment('../stats')).toBe('..%2Fstats')
    expect(segment('a?b#c')).toBe('a%3Fb%23c')
    expect(segment(42)).toBe('42')
  })

  it('refuses a segment of dots, however they are spelled', () => {
    for (const dots of ['.', '..', '%2e%2E', '.%2e']) {
      expect(() => segment(dots), dots).toThrow(ApiError)
    }
  })
})

describe('an answer', () => {
  afterEach(() => vi.unstubAllGlobals())

  const answering = (status: number, body: string, statusText = '') =>
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(status === 204 ? null : body, { status, statusText }))))

  it('is the JSON the server sent', async () => {
    answering(200, '{"total":3}')
    expect(await api.get<{ total: number }>('/items')).toEqual({ total: 3 })
  })

  it('is nothing for a 204', async () => {
    answering(204, '')
    expect(await api.delete('/items/1')).toBeUndefined()
  })

  it('carries the server’s own code and words when it refuses', async () => {
    answering(403, '{"error":"forbidden","message":"Administrators only"}', 'Forbidden')
    const refused = api.get('/users')
    await expect(refused).rejects.toMatchObject({ status: 403, code: 'forbidden', message: 'Administrators only' })
  })

  it('keeps its status when a proxy answered with a page instead', async () => {
    answering(401, '<html>Sign in</html>', 'Unauthorized')
    const error: unknown = await api.get('/auth/me').catch((caught: unknown) => caught)
    expect(error).toBeInstanceOf(ApiError)
    expect((error as ApiError).isUnauthorized).toBe(true)
    expect((error as ApiError).code).toBe('unknown')
  })

  it('is an error, not a parse failure, when a success is not JSON', async () => {
    answering(200, '<html>captive portal</html>', 'OK')
    await expect(api.get('/items')).rejects.toMatchObject({ status: 200, code: 'unreadable' })
  })
})
