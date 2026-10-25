import { describe, expect, it } from 'vitest'

import { BUILT_IN, readViews, refusal, VIEW_NAME, VIEWS_MOST, viewOf, type SavedView } from './views'

describe('a view of the catalogue', () => {
  it('is written one way only, whatever order the address said it in', () => {
    expect(viewOf('sort=title&kind=series&term=bad')).toBe('term=bad&kind=series&sort=title')
    expect(viewOf('missing=overview,poster')).toBe('missing=poster%2Coverview')
    expect(viewOf(new URLSearchParams('missing= imdb , trailer ,'))).toBe('missing=imdb')
  })

  it('leaves out what is the register’s own way', () => {
    expect(viewOf('')).toBe('')
    expect(viewOf('sort=popularity')).toBe('')
    // A sort's own direction is no part of the view; the other one is.
    expect(viewOf('sort=title&order=asc')).toBe('sort=title')
    expect(viewOf('sort=title&order=desc')).toBe('sort=title&order=desc')
    expect(viewOf('sort=added&order=desc')).toBe('sort=added')
    // An order with nothing to order, or that orders nothing, is no view either.
    expect(viewOf('order=desc')).toBe('')
    expect(viewOf('sort=title&order=sideways')).toBe('sort=title')
  })

  it('reads a flag as on or off, and drops what it does not know', () => {
    expect(viewOf('refreshFailed=true&manual=1&tab=record')).toBe('manual=1&refreshFailed=1')
    expect(viewOf('refreshFailed=0')).toBe('')
  })

  it('offers its own views as the addresses its chips lead to', () => {
    for (const offered of BUILT_IN) expect(viewOf(offered.query)).toBe(offered.query)
  })
})

describe('the views a person saved', () => {
  it('are read back as views, what is not one left out', () => {
    expect(readViews('nothing')).toBeUndefined()
    expect(
      readViews([
        { name: 'Séries sans affiche', query: 'missing=poster&kind=series' },
        { name: 42, query: 'kind=movie' },
        null,
        { name: 'Sans requête' },
      ]),
    ).toEqual([{ name: 'Séries sans affiche', query: 'kind=series&missing=poster' }])
  })

  it('are kept to as many as may be', () => {
    const many = Array.from({ length: VIEWS_MOST + 5 }, (_, n) => ({ name: `v${n}`, query: `term=${n}` }))
    expect(readViews(many)).toHaveLength(VIEWS_MOST)
  })
})

describe('saving a view', () => {
  const saved: SavedView[] = [{ name: 'Films à revoir', query: 'kind=movie&missing=overview' }]

  it('takes a name, short, of its own', () => {
    expect(refusal('', 'kind=series&sort=title', saved)).toEqual({ why: 'empty' })
    expect(refusal('   ', 'kind=series&sort=title', saved)).toEqual({ why: 'empty' })
    expect(refusal('x'.repeat(VIEW_NAME + 1), 'kind=series&sort=title', saved)).toEqual({ why: 'long' })
    expect(refusal('films À REVOIR', 'kind=series&sort=title', saved)).toEqual({ why: 'taken' })
    expect(refusal('Séries par titre', 'kind=series&sort=title', saved)).toBeNull()
  })

  it('is refused for a view kept already, saying under which name', () => {
    expect(refusal('Encore', 'kind=movie&missing=overview', saved)).toEqual({ why: 'same', saved: 'Films à revoir' })
    expect(refusal('Les séries', 'kind=series', saved)).toEqual({ why: 'same', builtIn: 'series' })
    expect(refusal('Tout', '', saved)).toEqual({ why: 'same', builtIn: 'all' })
  })

  it('is refused past the most a person keeps', () => {
    const full = Array.from({ length: VIEWS_MOST }, (_, n) => ({ name: `v${n}`, query: `term=${n}` }))
    expect(refusal('Une de plus', 'term=other', full)).toEqual({ why: 'many' })
  })
})
