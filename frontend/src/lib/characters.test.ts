import { describe, expect, it } from 'vitest'

import { characterPath, dubsOf, firstDub, languageKey, performerOf, voicesIn } from './characters'
import type { CharacterCredit, CharacterVoice, Credit } from './types'

const voice = (language: string, name: string, sortOrder = 0): CharacterVoice => ({ language, name, sortOrder })

const part = (name: string, voices: CharacterVoice[], creditId?: string): CharacterCredit => ({
  character: { id: `id-${name}`, name },
  sortOrder: 0,
  creditId,
  voices,
})

const credit = (id: string, personName: string): Credit => ({ id, personName, creditType: 'actor', sortOrder: 0, isManual: false })

const english = (language: string) =>
  ({ jpn: 'Japanese', eng: 'English', fra: 'French', spa: 'Spanish', deu: 'German' })[language] ?? language

describe('a work’s characters, as its page draws them', () => {
  it('reads a language as the model writes it', () => {
    expect(languageKey('ja')).toBe('jpn')
    expect(languageKey('fr-FR')).toBe('fra')
    expect(languageKey('jpn')).toBe('jpn')
    expect(languageKey('')).toBeUndefined()
    expect(languageKey(undefined)).toBeUndefined()
  })

  it('offers the dubs the work’s own first, then the reader’s, then the rest by name', () => {
    const characters = [
      part('Frieren', [voice('spa', 'A'), voice('jpn', 'B'), voice('eng', 'C')]),
      part('Fern', [voice('fra', 'D'), voice('deu', 'E')]),
    ]
    expect(dubsOf(characters, 'ja', 'fr', english)).toEqual(['jpn', 'fra', 'eng', 'deu', 'spa'])
    // No voice in the reader's language: none of it offered.
    expect(dubsOf([part('Solo', [voice('jpn', 'B')])], 'ja', 'fr', english)).toEqual(['jpn'])
    expect(dubsOf([part('Nobody', [])], 'ja', 'fr', english)).toEqual([])
  })

  it('opens on the reader’s dub where there is one, else the work’s own', () => {
    expect(firstDub(['jpn', 'fra', 'eng'], 'ja', 'fr')).toBe('fra')
    expect(firstDub(['jpn', 'eng'], 'ja', 'fr')).toBe('jpn')
    expect(firstDub(['eng', 'spa'], 'ja', 'fr')).toBe('eng')
    expect(firstDub([], 'ja', 'fr')).toBeUndefined()
  })

  it('names the performer among the credits the work serves, and none a lock hides', () => {
    const credits = [credit('c1', 'Atsumi Tanezaki')]
    expect(performerOf({ creditId: 'c1' }, credits)?.personName).toBe('Atsumi Tanezaki')
    expect(performerOf({ creditId: 'hidden' }, credits)).toBeUndefined()
    expect(performerOf({}, credits)).toBeUndefined()
  })

  it('says the voices of a dub in their order, the performer aside', () => {
    const frieren = part('Frieren', [
      voice('jpn', 'Atsumi Tanezaki'),
      voice('eng', 'Second', 1),
      voice('eng', 'Mallorie Rodak', 0),
    ])
    expect(voicesIn(frieren, 'eng').map((v) => v.name)).toEqual(['Mallorie Rodak', 'Second'])
    // Its case aside: the credit and the dub may spell it apart.
    expect(voicesIn(frieren, 'jpn', { personName: 'atsumi tanezaki' })).toEqual([])
    expect(voicesIn(frieren, 'jpn').map((v) => v.name)).toEqual(['Atsumi Tanezaki'])
    expect(voicesIn(frieren, undefined)).toEqual([])
  })

  it('leads to a character’s page by this server’s id', () => {
    expect(characterPath('01a0d073-6ce8-760c-a2cb-113d4cf78503')).toBe('/characters/01a0d073-6ce8-760c-a2cb-113d4cf78503')
  })
})
