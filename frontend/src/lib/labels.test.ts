import { describe, expect, it } from 'vitest'

import { genreLabel, genreParts, languageGroups, languageName, listedGenres, lookup, titleOrigin } from './labels'

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

// Named by the engine (`Intl.DisplayNames`), in the reader's language, rather
// than from a table kept here: the only table left maps a country's three
// letters to the two `DisplayNames` takes, which it cannot do itself.
describe('a language, named', () => {
  it('is said in the reader’s language, from two letters or three', () => {
    expect(languageName('jpn', 'en-GB')).toBe('Japanese')
    expect(languageName('ja', 'fr-FR')).toBe('Japonais')
    expect(languageName('fra', 'fr-FR')).toBe('Français')
  })

  it('is said once for several codes, by the first', () => {
    expect(languageName('ja,jpn', 'en-GB')).toBe('Japanese')
  })

  it('is its code, in capitals, where the engine has no name for it', () => {
    expect(languageName('qqq', 'en-GB')).toBe('QQQ')
    expect(languageName(undefined, 'en-GB')).toBeUndefined()
  })

  it('gathers the codes one tongue goes by, in the order they came', () => {
    const counted = [
      { code: 'ja', works: 2 },
      { code: 'en', works: 1 },
      { code: 'jpn', works: 3 },
    ]
    expect(languageGroups(counted, (row) => row.code, (row) => row.works, 'en-GB')).toEqual([
      { codes: ['ja', 'jpn'], label: 'Japanese', count: 5 },
      { codes: ['en'], label: 'English', count: 1 },
    ])
  })
})

describe('where an alternative title is from', () => {
  it('is a country where the code is one the server files titles under', () => {
    expect(titleOrigin('bra', 'en-GB')).toBe('Brazil')
    expect(titleOrigin('chn', 'fr-FR')).toBe('Chine')
    expect(titleOrigin('BR', 'fr-FR')).toBe('Brésil')
  })

  it('is a language where the caller says the code is one', () => {
    expect(titleOrigin('fra', 'en-GB', 'language')).toBe('French')
  })

  it('is left unsaid where the code means two things, or nothing', () => {
    expect(titleOrigin('bel', 'en-GB')).toBeUndefined()
    expect(titleOrigin('qaa', 'en-GB')).toBeUndefined()
    expect(titleOrigin('', 'en-GB')).toBeUndefined()
  })
})
