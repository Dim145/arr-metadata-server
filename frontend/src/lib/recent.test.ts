import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { image, work } from './fixtures'
import { forAdults } from './prefs'
import type { Recent } from './recent'

/** A browser's storage, kept in a map. */
function storage(initial: Record<string, string> = {}) {
  const kept = new Map(Object.entries(initial))
  return {
    kept,
    getItem: (key: string) => kept.get(key) ?? null,
    setItem: (key: string, value: string) => void kept.set(key, value),
    removeItem: (key: string) => void kept.delete(key),
  }
}

/** The module afresh: it holds what it read last. */
async function fresh() {
  vi.resetModules()
  return import('./recent')
}

function shelf(kept: Map<string, string>): { id: string }[] {
  return JSON.parse(kept.get('ams.recent') ?? '[]') as { id: string }[]
}

describe('the works opened lately', () => {
  let local: ReturnType<typeof storage>

  beforeEach(() => {
    local = storage()
    vi.stubGlobal('localStorage', local)
  })

  afterEach(() => vi.unstubAllGlobals())

  it('keep what a card needs of a work, newest first', async () => {
    const { remember } = await fresh()
    remember(
      work({
        id: 'a',
        title: 'Heat',
        kind: 'movie',
        year: 1995,
        images: [image('p', { url: 'https://example.org/heat.jpg' })],
        ratings: [{ source: 'imdb', value: 8.3, votes: 700_000 }],
      }),
    )
    remember(work({ id: 'b' }))

    const [latest, first] = shelf(local.kept) as Record<string, unknown>[]
    expect(latest?.id).toBe('b')
    expect(first).toMatchObject({ id: 'a', title: 'Heat', kind: 'movie', year: 1995, poster: 'https://example.org/heat.jpg' })
    expect(first?.rating).toMatchObject({ source: 'imdb', value: 8.3 })
  })

  it('keep what veils a work for adults, and nothing where there is nothing to veil', async () => {
    const { remember } = await fresh()
    remember(work({ id: 'marked', isAdult: true }))
    remember(work({ id: 'rated', contentRating: 'R18+' }))
    remember(work({ id: 'teen', contentRating: 'TV-14' }))
    remember(work({ id: 'plain' }))

    const kept = shelf(local.kept) as Recent[]
    expect(kept.find((recent) => recent.id === 'marked')).toMatchObject({ isAdult: true })
    expect(kept.find((recent) => recent.id === 'rated')).toMatchObject({ contentRating: 'R18+' })
    const plain = kept.find((recent) => recent.id === 'plain')
    expect(plain).not.toHaveProperty('isAdult')
    expect(plain).not.toHaveProperty('contentRating')
    // The rule every other row applies to a work reads the shelf's entries alike.
    expect(kept.filter(forAdults).map((recent) => recent.id)).toEqual(['rated', 'marked'])
  })

  it('read an entry kept before the veil as a work for anyone, and pass over one whose veil is broken', async () => {
    local.kept.set(
      'ams.recent',
      JSON.stringify([
        { id: 'before', title: 'Before', kind: 'movie', at: 0 },
        { id: 'figure', title: 'Figure', kind: 'movie', at: 0, contentRating: 18 },
        { id: 'word', title: 'Word', kind: 'movie', at: 0, isAdult: 'yes' },
      ]),
    )
    const { remember } = await fresh()
    remember(work({ id: 'new' }))

    const kept = shelf(local.kept) as Recent[]
    expect(kept.map((recent) => recent.id)).toEqual(['new', 'before'])
    expect(kept.filter(forAdults)).toEqual([])
  })

  it('hold each work once, moved to the front when it is opened again', async () => {
    const { remember } = await fresh()
    for (const id of ['a', 'b', 'a']) remember(work({ id }))
    expect(shelf(local.kept).map((recent) => recent.id)).toEqual(['a', 'b'])
  })

  it('are a shelf’s worth: twelve', async () => {
    const { remember } = await fresh()
    for (let n = 1; n <= 13; n += 1) remember(work({ id: `w${n}` }))
    const ids = shelf(local.kept).map((recent) => recent.id)
    expect(ids).toHaveLength(12)
    expect(ids[0]).toBe('w13')
    expect(ids).not.toContain('w1')
  })

  it('pass over what was stored broken, or by something else', async () => {
    local.kept.set('ams.recent', JSON.stringify([{ id: 1 }, { id: 'old', title: 'Old', kind: 'series', at: 0 }]))
    const { remember } = await fresh()
    remember(work({ id: 'new' }))
    expect(shelf(local.kept).map((recent) => recent.id)).toEqual(['new', 'old'])

    local.kept.set('ams.recent', '{not json')
    const again = await fresh()
    again.remember(work({ id: 'only' }))
    expect(shelf(local.kept).map((recent) => recent.id)).toEqual(['only'])
  })

  it('are cleared altogether', async () => {
    const { remember, forget } = await fresh()
    remember(work({ id: 'a' }))
    forget()
    expect(local.kept.has('ams.recent')).toBe(false)
  })

  it('are kept for the visit where the browser refuses to store them', async () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('denied')
      },
      setItem: () => {
        throw new Error('denied')
      },
      removeItem: () => {
        throw new Error('denied')
      },
    })
    const { remember } = await fresh()
    expect(() => remember(work({ id: 'a' }))).not.toThrow()
  })
})
