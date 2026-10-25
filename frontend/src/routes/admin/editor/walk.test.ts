import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { around, keepRun, nextOf, readRun, type Run } from './walk'

const run: Run = {
  query: 'kind=series&includeDisabled=true',
  ids: ['a', 'b', 'c'],
  titles: { a: 'Andor', b: 'Barry', c: 'Chernobyl' },
  total: 4,
}

describe('where a work stands in the catalogue’s list', () => {
  it('is its place from one, and the works on each side as far as they are known', () => {
    expect(around(run, 'a')).toEqual({ at: 1, total: 4, previous: undefined, next: { id: 'b', title: 'Barry' } })
    expect(around(run, 'b')).toEqual({
      at: 2,
      total: 4,
      previous: { id: 'a', title: 'Andor' },
      next: { id: 'c', title: 'Chernobyl' },
    })
    expect(around(run, 'c')?.next).toBeUndefined()
  })

  it('is found under the work’s address as well as its id', () => {
    expect(around(run, 'not-in-it', 'b')?.at).toBe(2)
    expect(around(run, 'not-in-it')).toBeNull()
    expect(around(null, 'a')).toBeNull()
  })
})

describe('the list kept for the editor', () => {
  const kept = new Map<string, string>()
  beforeEach(() => {
    kept.clear()
    vi.stubGlobal('window', {
      location: { pathname: '/admin/catalogue/c' },
      sessionStorage: {
        getItem: (key: string) => kept.get(key) ?? null,
        setItem: (key: string, value: string) => kept.set(key, value),
      },
    })
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('is read back as it was kept, and not when it no longer reads as one', () => {
    expect(readRun()).toBeNull()
    keepRun(run)
    expect(readRun()).toEqual(run)
    kept.set('ams.catalogue.run', '{"ids": "nope"}')
    expect(readRun()).toBeNull()
  })

  it('is walked past its end by asking the server for its next page', async () => {
    keepRun(run)
    const asked: string[] = []
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) => {
        asked.push(url)
        return Promise.resolve(
          new Response(JSON.stringify({ items: [{ id: 'c' }, { id: 'd', title: 'Dark' }], total: 4 }), { status: 200 }),
        )
      }),
    )

    expect(await nextOf('b')).toEqual({ id: 'c', title: 'Chernobyl' })
    expect(asked).toHaveLength(0)

    expect(await nextOf('c')).toEqual({ id: 'd', title: 'Dark' })
    expect(asked).toEqual(['/api/v1/items?kind=series&includeDisabled=true&limit=60&offset=3'])
    // Kept, so the next step needs no question.
    expect(readRun()?.ids).toEqual(['a', 'b', 'c', 'd'])
    expect(await nextOf('d')).toBeUndefined()
    expect(asked).toHaveLength(1)
  })
})
