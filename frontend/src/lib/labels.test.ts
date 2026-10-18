import { describe, expect, it } from 'vitest'

import { genreLabel, genreParts, listedGenres, lookup } from './labels'

describe('a word looked up by what the server sent', () => {
  it('is the table’s own, never something every object inherits', () => {
    const table = { open: 'Open' }
    expect(lookup(table, 'open')).toBe('Open')
    expect(lookup(table, 'constructor')).toBeUndefined()
    expect(lookup(table, '__proto__')).toBeUndefined()
  })

  it('names a genre in French on a French page, and as it came otherwise', () => {
    expect(genreLabel('Drama', 'fr')).toBe('Drame')
    expect(genreLabel('Drama', 'en')).toBe('Drama')
    expect(genreLabel('Ninja Western', 'fr')).toBe('Ninja Western')
    expect(genreLabel('constructor', 'fr')).toBe('constructor')
  })
})

describe('a work’s genres', () => {
  it('split TMDB’s combined series genres into the film genres they stand for', () => {
    expect(genreParts('Action & Adventure')).toEqual(['Action', 'Adventure'])
    expect(genreParts(' Drama ')).toEqual(['Drama'])
    expect(genreParts('  ')).toEqual([])
  })

  it('are listed each once', () => {
    expect(listedGenres(['Action & Adventure', 'Action', 'Sci-Fi & Fantasy'])).toEqual([
      'Action',
      'Adventure',
      'Science Fiction',
      'Fantasy',
    ])
  })
})
