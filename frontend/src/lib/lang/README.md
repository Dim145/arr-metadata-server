# Interface languages

English is the source of truth: `en.ts` defines the shape, and every other
language is measured against it. French (`fr.ts`) ships complete and is typed
as the whole dictionary, so it cannot fall behind.

## Adding a language

1. Copy `fr.ts` to `<tag>.ts` — the tag is BCP-47, lower case: `de`, `es`,
   `pt-br` — and type it as `Translation` instead of `Dict`:

   ```ts
   import type { Translation } from './en'

   export const de: Translation = {
     nav: { browse: 'Stöbern' },
   }
   ```

   Translate what you can. Every key is optional, down to the leaves; what is
   missing is shown in English until somebody translates it. A value that
   takes an argument is a function, translated whole.

2. Register it in `../i18n.tsx`, under `LANGUAGES`, with its name in itself,
   the locale `Intl` should format dates and numbers with, and where its
   words are — a `part`, which English completes:

   ```ts
   de: { name: 'Deutsch', locale: 'de-DE', words: () => import('./lang/de').then(({ de }) => ({ part: de })) },
   ```

   Keep the import a dynamic one: each dictionary is a file of its own,
   fetched by those who read it and nobody else.

That is all: the language toggles list every entry of `LANGUAGES`, the
browser's preference is matched against their tags, and `npm run typecheck`
refuses a key that does not exist in English.

## What a dictionary does not cover

The genre and job vocabularies in `../labels.ts` are French where the
language is French and English otherwise: a third language shows English
genres and credits until those tables learn it too. And the six `as
Record<string, string>` blocks — relations, formats, orders, departments,
notes, audit actions — are keyed by what the server sends, so a key that is
not in English is not refused by the build; keep them to the same keys.
`npm test` holds French to it: `../i18n.test.ts` fails on a key one of the
two dictionaries has and the other lacks, at any depth.

## Counting

A value that follows a count is written with the dictionary's own `plural`:
start the file with `import { plurals } from '../i18n'` and
`const plural = plurals('<tag>')`, then
`count: (n: number) => plural(n, { one: \`${n} work\`, other: \`${n} works\` })`.
Give the forms your language has (`zero`, `one`, `two`, `few`, `many`,
`other` — the categories of `Intl.PluralRules`); `other` is required and
stands in for any form left out. A word that means exactly one (« un
compte ») goes in front with `n === 1 ? … :`, since some languages file 0
under `one`. The rules are those of the dictionary's own language, not the
reader's: an English sentence left in a partial translation keeps English
grammar.
