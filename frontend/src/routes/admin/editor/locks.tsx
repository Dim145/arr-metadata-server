/**
 * Which lock holds a row, in the language the work is shown in.
 *
 * A text — a title, an overview, a tagline — may be locked in one language
 * alone as well as in every one, and a reader of that language is given the
 * lock made in it first. So a row cannot be told its lock by its scope and
 * field alone: it needs the language "Show in" names, and every lock the
 * work carries. The editor provides both once, here, and each row reads
 * them, whichever tab drew it.
 *
 * The locks on a credit the sources gave, hidden or corrected, are named
 * here too, as the server names them, so that a tab can say which of its
 * rows a person corrected.
 */

import { createContext, use } from 'react'

import type { Credit, Override } from '../../../lib/types'

export interface EditorLocks {
  /** The language "Show in" names, as it is chosen there (`fr`), or empty for the work as stored. */
  language: string
  overrides: Override[]
  /** What the work, as shown, says is locked: `scope/field`, and `scope/field@fra` for a text locked in another language only. */
  lockedFields: string[]
  /** The fields a lock may hold in one language alone. */
  translated: string[]
}

export const EditorLocksContext = createContext<EditorLocks | null>(null)

/** The locks of the work being edited, or nothing outside the editor. */
export function useEditorLocks(): EditorLocks | null {
  return use(EditorLocksContext)
}

/** The languages "Show in" offers, by the code it is chosen with and the one translations are filed under. */
const SHELVES: Record<string, string> = {
  en: 'eng',
  fr: 'fra',
  de: 'deu',
  es: 'spa',
  it: 'ita',
  pt: 'por',
  nl: 'nld',
  ja: 'jpn',
  ko: 'kor',
  zh: 'zho',
  ru: 'rus',
}

/** A language as translations are filed, and a lock in it is kept: `fra` for `fr`. */
export function shelfOf(language: string): string {
  const code = language.trim().toLowerCase().split(/[-_]/)[0] ?? ''
  return SHELVES[code] ?? code
}

/** A language a lock is kept under, as `Intl` names it best: `fr` for `fra`. */
export function codeOf(shelf: string): string {
  return Object.entries(SHELVES).find(([, filed]) => filed === shelf)?.[0] ?? shelf
}

/**
 * The lock a field is shown under: for a text, the one made in the language
 * shown, else the one made in every language; for any other field, the one
 * the caller found, which is the field's only one.
 *
 * Shown as stored, the work is in the language it is stored in, and a text
 * locked in that language comes first. Which language that is the work
 * itself says: it names every other language's lock as that language's
 * (`item/title@fra`), and not this one.
 */
export function lockOf(locks: EditorLocks | null, scope: string, field: string, found?: Override): Override | undefined {
  if (!locks || !locks.translated.includes(field)) return found
  const same = (lock: Override) => lock.scope === scope && lock.field === field
  const everywhere = locks.overrides.find((lock) => same(lock) && !lock.language)
  if (locks.language) {
    const shelf = shelfOf(locks.language)
    return locks.overrides.find((lock) => same(lock) && lock.language === shelf) ?? everywhere
  }
  const stored = locks.overrides.find(
    (lock) => same(lock) && lock.language && !locks.lockedFields.includes(`${scope}/${field}@${lock.language}`),
  )
  return stored ?? everywhere
}

/** What a lock names a credit a source gave by: what the person did and who they are. */
export function creditScope(credit: Pick<Credit, 'creditType' | 'tmdbPersonId' | 'personName'>): string {
  return credit.tmdbPersonId !== undefined && credit.tmdbPersonId !== null
    ? `credit:${credit.creditType}:tmdb:${credit.tmdbPersonId}`
    : `credit:${credit.creditType}:name:${credit.personName.trim()}`
}

/** The locks one child carries, by the scope it is named by. */
export function locksOn(locks: EditorLocks | null, scope: string): Override[] {
  return (locks?.overrides ?? []).filter((lock) => lock.scope === scope)
}
