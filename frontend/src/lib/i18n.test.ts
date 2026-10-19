import { afterEach, describe, expect, it, vi } from 'vitest'

import { bare, completed, dictionary, LANGS, plurals, preferred } from './i18n'
import { en, type Dict } from './lang/en'
import { fr } from './lang/fr'

/** Every leaf of a dictionary, by its path, with what kind of value it is. */
function leaves(value: unknown, at = ''): string[] {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return [`${at} ${typeof value}`]
  return Object.entries(value).flatMap(([key, inner]) => leaves(inner, at ? `${at}.${key}` : key))
}

describe('a translation laid over English', () => {
  const count = (n: number) => `${n} works`
  const base = { nav: { browse: 'Browse', lists: 'Selections' }, count, tips: ['one'] }

  it('takes every key it carries, and English for the rest, down to the leaves', () => {
    expect(completed(base, { nav: { browse: 'Stöbern' } })).toEqual({
      nav: { browse: 'Stöbern', lists: 'Selections' },
      count,
      tips: ['one'],
    })
  })

  it('replaces a function or a list whole', () => {
    const counted = (n: number) => `${n} Werke`
    expect(completed(base, { count: counted, tips: ['eins', 'zwei'] })).toMatchObject({ count: counted, tips: ['eins', 'zwei'] })
  })

  it('is English itself when there is nothing to lay over it', () => {
    expect(completed(base, undefined)).toBe(base)
  })
})

describe('a table of words', () => {
  const table = bare({ policy: { open: 'Open' }, tips: ['one'] }) as { policy: Record<string, string>; tips: string[] }

  it('answers nothing for a key it does not hold, `constructor` included', () => {
    expect(Object.getPrototypeOf(table)).toBeNull()
    expect(Object.getPrototypeOf(table.policy)).toBeNull()
    expect(table.policy.open).toBe('Open')
    expect(table.policy['constructor']).toBeUndefined()
    expect('toString' in table.policy).toBe(false)
  })

  it('leaves a list a list', () => {
    expect(table.tips).toEqual(['one'])
  })
})

describe('the dictionaries', () => {
  it('are every language registered', () => {
    expect(LANGS).toEqual(['en', 'fr'])
  })

  it('are fetched once each', async () => {
    const first = await dictionary('fr')
    expect(await dictionary('fr')).toBe(first)
    expect(first.brand.tagline).toBe(fr.brand.tagline)
  })

  it('are read without a prototype, at every depth', async () => {
    const t = await dictionary('en')
    expect(Object.getPrototypeOf(t)).toBeNull()
    expect((t.labels.policy as Record<string, string>)['constructor']).toBeUndefined()
  })

  // The tables keyed by what the server sends are cast to a record in English,
  // so the build does not refuse a key French lacks there; this does.
  it('hold every key in French that they hold in English, and no other', () => {
    expect(leaves(fr).sort()).toEqual(leaves(en).sort())
  })

  it('complete a partial language from English', async () => {
    const de = bare(completed(await dictionary('en'), { nav: { browse: 'Stöbern' } })) as Dict
    expect(de.nav.browse).toBe('Stöbern')
    expect(de.nav.lists).toBe(en.nav.lists)
    expect(de.figures.addedRecently(2)).toBe(en.figures.addedRecently(2))
  })
})

describe('a count and the words after it', () => {
  const forms = { one: 'one', other: 'other' }

  it('follow English, where 1 alone is singular', () => {
    const plural = plurals('en')
    expect([0, 1, 2, 21].map((n) => plural(n, forms))).toEqual(['other', 'one', 'other', 'other'])
  })

  it('follow French, where 0 is singular too', () => {
    const plural = plurals('fr')
    expect([0, 1, 2, 21].map((n) => plural(n, forms))).toEqual(['one', 'one', 'other', 'other'])
  })

  it('take `other` for a form the language has and the dictionary does not give', () => {
    // French files a round million under `many`: "1 000 000 de films".
    const plural = plurals('fr')
    expect(plural(1_000_000, { ...forms, many: 'many' })).toBe('many')
    expect(plural(1_000_000, forms)).toBe('other')
  })

  it('are said by each dictionary in its own grammar', () => {
    expect(en.figures.addedRecently(0)).toBe('0 works added in the last thirty days.')
    expect(en.figures.addedRecently(1)).toBe('1 work added in the last thirty days.')
    expect(fr.figures.addedRecently(0)).toBe('0 œuvre ajoutée ces trente derniers jours.')
    expect(fr.figures.addedRecently(2)).toBe('2 œuvres ajoutées ces trente derniers jours.')
  })

  it('keep a word for exactly one to one, in French as in English', () => {
    expect(en.work.otherCountries(1)).toBe('One other country')
    expect(en.work.otherCountries(0)).toBe('0 other countries')
    expect(fr.work.otherCountries(1)).toBe('Un autre pays')
    expect(fr.work.otherCountries(0)).toBe('0 autre pays')
  })
})

describe('the language a reader is shown', () => {
  afterEach(() => vi.unstubAllGlobals())

  const browser = (...languages: string[]) => vi.stubGlobal('navigator', { languages, language: languages[0] })
  const stored = (value: string | null) => vi.stubGlobal('localStorage', { getItem: () => value })

  it('is the one they chose', () => {
    stored('fr')
    browser('en-US')
    expect(preferred()).toBe('fr')
  })

  it('is otherwise the browser’s, a regional tag included', () => {
    stored(null)
    browser('fr-CA', 'en')
    expect(preferred()).toBe('fr')
  })

  it('ignores a stored tag the interface does not speak, and storage that refuses', () => {
    stored('klingon')
    browser('fr')
    expect(preferred()).toBe('fr')

    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('denied')
      },
    })
    expect(preferred()).toBe('fr')
  })

  it('is English when nothing else matches', () => {
    stored(null)
    browser('de-DE', 'ja')
    expect(preferred()).toBe('en')
  })
})
