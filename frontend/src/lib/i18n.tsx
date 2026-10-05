/**
 * One shape, as many languages as are registered.
 *
 * English is the source of truth: `Dict` is derived from it. French ships
 * complete and is typed as the whole, so a missing translation there is a
 * type error at the moment it is introduced. Any other language may be
 * partial — what it lacks is shown in English — so a translation can start
 * small and grow. See `lang/README.md` to add one.
 */

import { createContext, use, useCallback, useEffect, useMemo, useState } from 'react'

import { en, type Dict, type Translation } from './lang/en'
import { fr } from './lang/fr'

export type { Dict, Translation } from './lang/en'

/** A language the interface speaks. */
interface Language {
  /** Its name, in itself. */
  name: string
  /** The BCP-47 tag Intl formats dates and numbers with, and the server's `?language=`. */
  locale: string
  dictionary: Translation
}

/**
 * Every language the interface speaks, by the tag it is chosen and stored by.
 * The toggles list these; the browser's preference is matched against them.
 */
export const LANGUAGES = {
  en: { name: 'English', locale: 'en-GB', dictionary: {} },
  fr: { name: 'Français', locale: 'fr-FR', dictionary: fr },
} satisfies Record<string, Language>

export type Lang = keyof typeof LANGUAGES

export const LANGS = Object.keys(LANGUAGES) as Lang[]

const STORAGE_KEY = 'ams.lang'

/**
 * A translation laid over English: every key it carries, and English for the
 * rest, down to the leaves. A function or an array is a leaf.
 */
function completed(base: unknown, over: unknown): unknown {
  if (over === undefined) return base
  if (typeof base !== 'object' || base === null || Array.isArray(base)) return over
  if (typeof over !== 'object' || over === null) return over
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) }
  for (const [key, value] of Object.entries(over as Record<string, unknown>)) {
    out[key] = completed((base as Record<string, unknown>)[key], value)
  }
  return out
}

/**
 * Every table of words without a prototype.
 *
 * Several are looked up by what the server sends — a gallery's kinds, a
 * relation's type, an audit trail's actions — and on a plain object
 * `table['constructor']` is the `Object` function, which a page then renders,
 * folds or sorts as if it were a word. With no prototype, a key the table
 * does not hold finds nothing, and the `?? key` after it says the key itself.
 */
function bare(value: unknown): unknown {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return value
  const out = Object.create(null) as Record<string, unknown>
  for (const [key, inner] of Object.entries(value)) out[key] = bare(inner)
  return out
}

const DICTIONARIES = Object.fromEntries(
  LANGS.map((lang) => [lang, bare(completed(en, LANGUAGES[lang].dictionary)) as Dict]),
) as Record<Lang, Dict>

function isLang(value: unknown): value is Lang {
  return typeof value === 'string' && LANGS.includes(value as Lang)
}

/** What the browser asked for, if this interface speaks it. */
function preferred(): Lang {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (isLang(stored)) return stored
  } catch {
    // Private browsing, or storage disabled. The default is fine.
  }

  // The longest tag first, so a regional language is found before its parent.
  const byLength = [...LANGS].sort((a, b) => b.length - a.length)
  for (const tag of navigator.languages ?? [navigator.language]) {
    const lower = tag?.toLowerCase() ?? ''
    const match = byLength.find((code) => lower === code || lower.startsWith(`${code}-`))
    if (match) return match
  }

  return 'en'
}

type Value = {
  lang: Lang
  t: Dict
  setLang: (lang: Lang) => void
  /** The BCP-47 tag, for Intl and for the server's `?language=`. */
  locale: string
}

const Context = createContext<Value | null>(null)

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [lang, setLangState] = useState<Lang>(() => preferred())

  const setLang = useCallback((next: Lang) => {
    setLangState(next)
    try {
      localStorage.setItem(STORAGE_KEY, next)
    } catch {
      // The choice still applies to this visit.
    }
  }, [])

  // Assistive technology reads the page in the language the document claims.
  useEffect(() => {
    document.documentElement.lang = lang
  }, [lang])

  const value = useMemo<Value>(
    () => ({
      lang,
      t: DICTIONARIES[lang],
      setLang,
      locale: LANGUAGES[lang].locale,
    }),
    [lang, setLang],
  )

  return <Context value={value}>{children}</Context>
}

export function useI18n(): Value {
  const value = use(Context)

  if (!value) {
    throw new Error('useI18n must be used inside <I18nProvider>')
  }

  return value
}
