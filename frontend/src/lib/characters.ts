/**
 * A work's characters, as its page draws them: who plays each — the credit
 * the server names, among those the work serves — and who voices each in the
 * dub the reader chose, the dubs offered in a useful order.
 *
 * The model keeps languages as ISO 639-3 (`jpn`, `eng`); the reader's
 * language, and a work's own as TMDB gives it, may be two letters: both are
 * read as the model writes them before they are compared.
 */

import type { CharacterCredit, CharacterVoice, Credit } from './types'

/** Two letters to three, for the languages a dub or a reader is likely in. */
const THREE: Record<string, string> = {
  ar: 'ara',
  cs: 'ces',
  da: 'dan',
  de: 'deu',
  el: 'ell',
  en: 'eng',
  es: 'spa',
  fi: 'fin',
  fr: 'fra',
  he: 'heb',
  hi: 'hin',
  hu: 'hun',
  id: 'ind',
  it: 'ita',
  ja: 'jpn',
  ko: 'kor',
  nl: 'nld',
  no: 'nor',
  pl: 'pol',
  pt: 'por',
  ru: 'rus',
  sv: 'swe',
  th: 'tha',
  tl: 'tgl',
  tr: 'tur',
  uk: 'ukr',
  vi: 'vie',
  zh: 'zho',
}

/** A language as the model writes it: `ja`, `ja-JP` and `jpn` are all `jpn`. */
export function languageKey(code: string | null | undefined): string | undefined {
  const base = code?.trim().toLowerCase().split(/[-_,]/)[0]
  if (!base) return undefined
  return THREE[base] ?? base
}

/**
 * The dubs a work's characters are voiced in: the work's own language first
 * — an anime's original voices — then the reader's, then the rest in the
 * order `named` sorts their names.
 */
export function dubsOf(
  characters: Pick<CharacterCredit, 'voices'>[],
  original: string | null | undefined,
  reader: string | null | undefined,
  named: (language: string) => string = (language) => language,
): string[] {
  const found = new Set<string>()
  for (const character of characters) {
    for (const voice of character.voices ?? []) found.add(voice.language)
  }
  const first = [languageKey(original), languageKey(reader)].filter(
    (language, index, all): language is string => !!language && found.has(language) && all.indexOf(language) === index,
  )
  const rest = [...found]
    .filter((language) => !first.includes(language))
    .sort((a, b) => named(a).localeCompare(named(b)))
  return [...first, ...rest]
}

/** The dub a page opens on: the reader's where the characters are voiced in it, else the work's own, else the first. */
export function firstDub(
  dubs: string[],
  original: string | null | undefined,
  reader: string | null | undefined,
): string | undefined {
  for (const language of [languageKey(reader), languageKey(original)]) {
    if (language && dubs.includes(language)) return language
  }
  return dubs[0]
}

/**
 * The credit that plays a character, among those the work serves: none
 * where it names none, or one a lock hides — whose performer is then
 * unknown, as the page says.
 */
export function performerOf(character: Pick<CharacterCredit, 'creditId'>, credits: Credit[]): Credit | undefined {
  if (!character.creditId) return undefined
  return credits.find((credit) => credit.id === character.creditId)
}

/** A name as two are compared: its case and accents aside. */
function folded(name: string): string {
  return name.normalize('NFD').replace(/\p{M}/gu, '').toLocaleLowerCase().trim()
}

/**
 * Who voices a character in a dub, in the source's order — those who are its
 * performer already aside, since an anime's original voice is both.
 */
export function voicesIn(
  character: Pick<CharacterCredit, 'voices'>,
  dub: string | undefined,
  performer?: Pick<Credit, 'personName'>,
): CharacterVoice[] {
  if (!dub) return []
  const played = performer ? folded(performer.personName) : undefined
  return (character.voices ?? [])
    .filter((voice) => voice.language === dub && folded(voice.name) !== played)
    .sort((a, b) => a.sortOrder - b.sortOrder)
}

/** The page of a character, by this server's id for it. */
export function characterPath(id: string): string {
  return `/characters/${encodeURIComponent(id)}`
}
