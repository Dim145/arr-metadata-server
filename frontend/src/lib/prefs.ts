/**
 * The reader's own settings, kept in this browser as the interface's
 * language and the lamp are: the language the catalogue's texts are read in,
 * and whether the posters of works for adults are shown as they are — which
 * an account signed in may have chosen for itself, and then its choice is
 * the one that counts (see `adultPosters.ts`).
 *
 * Neither leaves the browser but as what it changes: the language rides on
 * every read of the catalogue as its `?language=` (see `api.ts`), and the
 * posters are the page's business alone — the account keeps its own choice,
 * never this browser's.
 */

import { useSyncExternalStore } from 'react'

export const TEXT_LANGUAGE_KEY = 'ams.textLanguage'
export const ADULT_POSTERS_KEY = 'ams.adultPosters'

/**
 * The languages the texts are offered in: those TMDB holds the most
 * translations in. The server takes any tag, and fetches a language the first
 * time a work is read in it — a short list keeps a reader from spending the
 * operator's quota on a language nobody here reads.
 */
export const TEXT_LANGUAGES = [
  'ar',
  'cs',
  'da',
  'de',
  'el',
  'en',
  'es',
  'fi',
  'fr',
  'he',
  'hi',
  'hu',
  'id',
  'it',
  'ja',
  'ko',
  'nl',
  'no',
  'pl',
  'pt',
  'pt-BR',
  'ro',
  'ru',
  'sv',
  'th',
  'tr',
  'uk',
  'vi',
  'zh',
  'zh-TW',
] as const

const listeners = new Set<() => void>()

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function told() {
  for (const listener of listeners) listener()
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    // Private browsing, or storage switched off: nothing was chosen.
    return null
  }
}

function write(key: string, value: string | undefined) {
  try {
    if (value === undefined) localStorage.removeItem(key)
    else localStorage.setItem(key, value)
  } catch {
    // The choice still applies to this visit.
  }
}

/**
 * Whether a value has the shape of a language the server takes: two or three
 * letters, then at most a region or a script. A value kept from elsewhere, or
 * written by hand, that does not is no choice at all — sent, it would make
 * every read of the catalogue a refusal.
 */
export function isLanguageTag(value: string | null | undefined): value is string {
  return typeof value === 'string' && /^[a-z]{2,3}(?:-(?:[A-Z]{2}|[A-Z][a-z]{3}))?$/.test(value)
}

/* ── The texts' language ──────────────────────────────────────────────────── */

/** What was read from the storage, once: `null` until it has been. */
let language: string | undefined | null = null

/**
 * The language the reader asked the catalogue's texts in — titles, synopses,
 * taglines — where it is not the interface's own; nothing where they did not.
 */
export function textLanguage(): string | undefined {
  if (language === null) {
    const stored = read(TEXT_LANGUAGE_KEY)
    language = isLanguageTag(stored) ? stored : undefined
  }
  return language
}

/** Read the texts in this language from now on, or in the interface's again. */
export function setTextLanguage(next: string | undefined) {
  language = isLanguageTag(next) ? next : undefined
  write(TEXT_LANGUAGE_KEY, language)
  told()
}

export function useTextLanguage(): string | undefined {
  return useSyncExternalStore(subscribe, textLanguage, () => undefined)
}

/* ── Posters for adults ───────────────────────────────────────────────────── */

/** This browser's choice, as read from the storage once: `null` until it has been. */
let adultPosters: boolean | null = null

/**
 * The choice of the account signed in, where it made one: nothing for a
 * visitor, nor for an account that never chose. Held for the visit alone —
 * the account keeps it, and puts it here again on the next.
 */
let accountPosters: boolean | undefined

/**
 * Whether the reader asked to see the posters of works for adults as they
 * are: as the account signed in chose, else as this browser did. Not until
 * one of them has: a catalogue shown on a living-room screen, or over a
 * shoulder, was showing one to whoever looked.
 */
export function adultPostersShown(): boolean {
  if (accountPosters !== undefined) return accountPosters
  if (adultPosters === null) adultPosters = read(ADULT_POSTERS_KEY) === 'shown'
  return adultPosters
}

/**
 * This browser's choice: a visitor's, and that of anybody signed in whose
 * account has not made one. Kept in the storage, for the next visit.
 */
export function showAdultPosters(shown: boolean) {
  adultPosters = shown
  write(ADULT_POSTERS_KEY, shown ? 'shown' : undefined)
  told()
}

/**
 * The choice of the account signed in, which counts over this browser's
 * while it is there; `undefined` when nobody is signed in, or the account
 * never chose, and this browser's counts again. Never written to the
 * storage: whoever uses the browser next is not the account.
 */
export function adoptAdultPosters(shown: boolean | undefined) {
  if (accountPosters === shown) return
  accountPosters = shown
  told()
}

export function useAdultPostersShown(): boolean {
  return useSyncExternalStore(subscribe, adultPostersShown, () => false)
}

/**
 * The works whose poster the reader uncovered in this visit, one at a time:
 * once asked for, it stays uncovered on every page that shows it, until the
 * tab is closed. Kept nowhere: the setting is the way to keep it.
 */
const unveiled = new Set<string>()

export function unveil(id: string) {
  if (unveiled.has(id)) return
  unveiled.add(id)
  told()
}

export function useUnveiled(id: string): boolean {
  return useSyncExternalStore(
    subscribe,
    () => unveiled.has(id),
    () => false,
  )
}

/**
 * Whether a rating keeps a work from anyone under eighteen: a figure of
 * eighteen or more in it — `18`, `R18+`, `-18`, `FSK 18`, `VM18`, Singapore's
 * `R21` — or one of the few that say it in letters. America's `NC-17` admits
 * nobody of seventeen and under, so it is one of them; `TV-MA` and `R` admit
 * a younger reader with a parent, so they are not.
 */
export function adultRating(rating: string | null | undefined): boolean {
  const text = rating?.trim().toUpperCase()
  if (!text) return false
  if (['NC-17', 'X', 'XXX', 'AO', 'ADULTS ONLY'].includes(text)) return true
  const figure = /\d+/.exec(text)
  const age = figure ? Number(figure[0]) : Number.NaN
  return age >= 18 && age <= 21
}

/**
 * Whether a work's poster is veiled for a reader who has not asked to see
 * it: the catalogue marks the work as for adults, or its rating is.
 */
export function forAdults(work: { isAdult?: boolean | null; contentRating?: string | null }): boolean {
  return work.isAdult === true || adultRating(work.contentRating)
}
