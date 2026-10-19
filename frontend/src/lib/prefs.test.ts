import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { adultRating, forAdults, isLanguageTag } from './prefs'

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
  return import('./prefs')
}

describe('a rating for adults', () => {
  it('is eighteen or more, however a country writes it', () => {
    for (const rating of ['18', '18+', 'R18+', 'R-18', '-18', 'FSK 18', 'VM18', 'M18', 'R21', 'X18+', ' 18 ']) {
      expect(adultRating(rating), rating).toBe(true)
    }
  })

  it('is one of the few said in letters', () => {
    for (const rating of ['NC-17', 'nc-17', 'X', 'AO', 'Adults Only']) {
      expect(adultRating(rating), rating).toBe(true)
    }
  })

  it('is not one a younger reader may see with a parent, nor no rating at all', () => {
    for (const rating of ['TV-MA', 'R', 'MA15+', '16', '-16', 'PG-13', 'TV-14', '12A', '', undefined, null]) {
      expect(adultRating(rating), String(rating)).toBe(false)
    }
  })
})

describe('a work for adults', () => {
  it('is one the catalogue marks so, or one its rating keeps from minors', () => {
    expect(forAdults({ isAdult: true })).toBe(true)
    expect(forAdults({ isAdult: false, contentRating: 'R18+' })).toBe(true)
    expect(forAdults({ isAdult: false, contentRating: 'TV-MA' })).toBe(false)
    expect(forAdults({})).toBe(false)
  })
})

describe('the language of the texts', () => {
  let local: ReturnType<typeof storage>

  beforeEach(() => {
    local = storage()
    vi.stubGlobal('localStorage', local)
  })

  afterEach(() => vi.unstubAllGlobals())

  it('is a tag as the server takes one, or nothing', () => {
    for (const tag of ['de', 'pt-BR', 'zh-Hant', 'fra']) expect(isLanguageTag(tag), tag).toBe(true)
    for (const tag of ['', 'german', 'pt_BR', 'PT-br', 'de-', '../x', null, undefined]) {
      expect(isLanguageTag(tag), String(tag)).toBe(false)
    }
  })

  it('is nothing until chosen, then kept, then given back to the interface', async () => {
    const prefs = await fresh()
    expect(prefs.textLanguage()).toBeUndefined()

    prefs.setTextLanguage('de')
    expect(prefs.textLanguage()).toBe('de')
    expect(local.kept.get('ams.textLanguage')).toBe('de')

    prefs.setTextLanguage(undefined)
    expect(prefs.textLanguage()).toBeUndefined()
    expect(local.kept.has('ams.textLanguage')).toBe(false)
  })

  it('ignores a kept value that is no language', async () => {
    local.kept.set('ams.textLanguage', 'not a language')
    const prefs = await fresh()
    expect(prefs.textLanguage()).toBeUndefined()
  })
})

describe('the posters of works for adults', () => {
  let local: ReturnType<typeof storage>

  beforeEach(() => {
    local = storage()
    vi.stubGlobal('localStorage', local)
  })

  afterEach(() => vi.unstubAllGlobals())

  it('are veiled until the reader asks to see them, and the choice is kept', async () => {
    const prefs = await fresh()
    expect(prefs.adultPostersShown()).toBe(false)

    prefs.showAdultPosters(true)
    expect(prefs.adultPostersShown()).toBe(true)
    expect(local.kept.get('ams.adultPosters')).toBe('shown')

    prefs.showAdultPosters(false)
    expect(local.kept.has('ams.adultPosters')).toBe(false)
  })

  it('is read as it was left by an earlier visit', async () => {
    local.kept.set('ams.adultPosters', 'shown')
    const prefs = await fresh()
    expect(prefs.adultPostersShown()).toBe(true)
  })
})
