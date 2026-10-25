/**
 * One shape, as many languages as are registered.
 *
 * English is the source of truth: `Dict` is derived from it. French ships
 * complete and is typed as the whole, so a missing translation there is a
 * type error at the moment it is introduced. Any other language may be
 * partial — what it lacks is shown in English — so a translation can start
 * small and grow. See `lang/README.md` to add one.
 *
 * Each dictionary is a file of its own, fetched when its language is first
 * wanted: the reader's before anything is drawn (`main.tsx` waits for it), any
 * other once it is chosen. Both used to be in the first bundle: a quarter of
 * a megabyte of words sent to every visitor, who reads one language of them.
 */

import { createContext, use, useCallback, useEffect, useMemo, useRef, useState } from 'react'

import type { Dict, Translation } from './lang/en'

export type { Dict, Translation } from './lang/en'

/** A language's words: the whole dictionary, or a translation English completes. */
type Words = { whole: Dict } | { part: Translation }

/** A language the interface speaks. */
interface Language {
  /** Its name, in itself. */
  name: string
  /** The BCP-47 tag Intl formats dates and numbers with, and the server's `?language=`. */
  locale: string
  /** Its words, fetched the first time the language is wanted. */
  words: () => Promise<Words>
}

/**
 * Every language the interface speaks, by the tag it is chosen and stored by.
 * The toggles list these; the browser's preference is matched against them.
 */
export const LANGUAGES = {
  en: { name: 'English', locale: 'en-GB', words: () => import('./lang/en').then(({ en }) => ({ whole: en })) },
  fr: { name: 'Français', locale: 'fr-FR', words: () => import('./lang/fr').then(({ fr }) => ({ whole: fr })) },
} satisfies Record<string, Language>

export type Lang = keyof typeof LANGUAGES

export const LANGS = Object.keys(LANGUAGES) as Lang[]

/**
 * A word's forms by count: `other` always, which every language has and which
 * stands in for any form a dictionary does not give, and whichever of `zero`,
 * `one`, `two`, `few` and `many` the language's grammar sorts counts into —
 * `one` for English and French, `few` and `many` for Polish or Russian, `two`
 * for Welsh or Arabic.
 */
export type Plural<T = string> = { other: T } & { [form in Exclude<Intl.LDMLPluralRule, 'other'>]?: T }

/**
 * The form a count takes, chosen by one language's rules.
 *
 * Each dictionary makes its own and calls it `plural` — `const plural =
 * plurals('fr')` — then writes `plural(n, { one: …, other: … })` wherever a
 * word follows a count. The rules are `Intl.PluralRules`', not a ternary on
 * 1, which was right in English and wrong in French, where 0 takes the
 * singular, and had no way to say the third and fourth forms other
 * languages have.
 *
 * Bound to the language the words are in rather than the one on screen: a
 * partial translation is completed with English, and the English it borrows
 * must keep English grammar — read by Japanese rules, which put 1 under
 * `other`, it would have said "1 works".
 */
export function plurals(lang: Lang): <T>(count: number, forms: Plural<T>) => T {
  const rules = new Intl.PluralRules(LANGUAGES[lang].locale)
  return (count, forms) => forms[rules.select(count)] ?? forms.other
}

const STORAGE_KEY = 'ams.lang'

/**
 * A translation laid over English: every key it carries, and English for the
 * rest, down to the leaves. A function or an array is a leaf.
 */
export function completed(base: unknown, over: unknown): unknown {
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
export function bare(value: unknown): unknown {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return value
  const out = Object.create(null) as Record<string, unknown>
  for (const [key, inner] of Object.entries(value)) out[key] = bare(inner)
  return out
}

/** Every dictionary asked for, by language. */
const fetched = new Map<Lang, Promise<Dict>>()

/**
 * A language's dictionary, ready to read: completed from English where the
 * language is partial, and without a prototype. Fetched once; a fetch that
 * failed is tried again the next time it is asked for.
 */
export function dictionary(lang: Lang): Promise<Dict> {
  let found = fetched.get(lang)
  if (!found) {
    found = LANGUAGES[lang]
      .words()
      .then(async (words) => bare('whole' in words ? words.whole : completed(await dictionary('en'), words.part)) as Dict)
    fetched.set(lang, found)
    found.catch(() => fetched.delete(lang))
  }
  return found
}

function isLang(value: unknown): value is Lang {
  return typeof value === 'string' && LANGS.includes(value as Lang)
}

/** What the browser asked for, if this interface speaks it. */
export function preferred(): Lang {
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

/** The language the page opened in, and its words, once `ready` has them. */
let opening: { lang: Lang; t: Dict } | undefined

/**
 * The reader's language, fetched before the first render so that the first
 * paint is already in it: `main.tsx` waits for this. English stands in for a
 * language whose file could not be fetched.
 */
export async function ready(): Promise<void> {
  const lang = preferred()
  try {
    opening = { lang, t: await dictionary(lang) }
  } catch (error) {
    if (lang === 'en') throw error
    opening = { lang: 'en', t: await dictionary('en') }
  }
  // Said before anything is drawn, rather than once the page has been.
  document.documentElement.lang = opening.lang
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
  const [shown, setShown] = useState(() => {
    if (!opening) throw new Error('<I18nProvider> is drawn once ready() has fetched the dictionary')
    return opening
  })
  // The language asked for last: one that was slower to arrive is not shown
  // over it.
  const asked = useRef(shown.lang)

  const setLang = useCallback((next: Lang) => {
    asked.current = next
    try {
      localStorage.setItem(STORAGE_KEY, next)
    } catch {
      // The choice still applies to this visit.
    }
    // The page stays in the language it is in until the other has arrived.
    dictionary(next).then(
      (t) => {
        if (asked.current === next) setShown((current) => (current.lang === next ? current : { lang: next, t }))
      },
      // The server was updated beneath a tab that was already open, and the
      // file is no longer there: the page read again finds the new one, in
      // the language just chosen.
      () => window.location.reload(),
    )
  }, [])

  // Assistive technology reads the page in the language the document claims.
  useEffect(() => {
    document.documentElement.lang = shown.lang
  }, [shown.lang])

  const value = useMemo<Value>(
    () => ({
      lang: shown.lang,
      t: shown.t,
      setLang,
      locale: LANGUAGES[shown.lang].locale,
    }),
    [shown, setLang],
  )

  return <Context value={value}>{children}</Context>
}

/**
 * A part of a page in another language than the rest: the styleguide's
 * columns, which draw each component in every language side by side. Its
 * words are fetched as any language's are, and nothing inside it is drawn
 * until they have come — or at all, if they cannot be. It says its language
 * (`lang`), so that a screen reader reads it in that language; a language
 * chosen inside it is chosen for the page.
 */
export function LanguageScope({ lang, children }: { lang: Lang; children: React.ReactNode }) {
  const outer = useI18n()
  const [words, setWords] = useState<{ lang: Lang; t: Dict } | null>(() =>
    outer.lang === lang ? { lang, t: outer.t } : null,
  )

  useEffect(() => {
    let live = true
    dictionary(lang).then(
      (t) => {
        if (live) setWords({ lang, t })
      },
      () => {},
    )
    return () => {
      live = false
    }
  }, [lang])

  const value = useMemo<Value | null>(
    () =>
      words?.lang === lang ? { lang, t: words.t, setLang: outer.setLang, locale: LANGUAGES[lang].locale } : null,
    [words, lang, outer.setLang],
  )

  return value ? (
    <Context value={value}>
      <div lang={lang} className="contents">
        {children}
      </div>
    </Context>
  ) : null
}

export function useI18n(): Value {
  const value = use(Context)

  if (!value) {
    throw new Error('useI18n must be used inside <I18nProvider>')
  }

  return value
}
