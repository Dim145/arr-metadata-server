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

2. Register it in `../i18n.tsx`, under `LANGUAGES`, with its name in itself
   and the locale `Intl` should format dates and numbers with:

   ```ts
   de: { name: 'Deutsch', locale: 'de-DE', dictionary: de },
   ```

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
