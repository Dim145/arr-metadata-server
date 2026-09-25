/**
 * The genres, as a filter that stays usable when there are many of them.
 *
 * The ones chosen come first, whatever their count. Then the commonest ten,
 * the ones most readers reach for; the rest fold away, A to Z, where a name
 * is found by eye — a `<details>`, so the browser's own find still reaches
 * them. From a dozen genres up, a field finds one by name, accents or not.
 * Two or more chosen combine as all of them, the way a filter narrows, or as
 * any of them, the way a mood browses.
 */

import { useId, useState } from 'react'

import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { genreLabel } from '../lib/labels'
import type { Facet } from '../lib/types'
import { Glyph, Input } from './ui'
import { stockOf } from './ui/chips'

/** Shown before the rest fold away. */
const COMMONEST = 10
/** From this many genres, a field to find one by name. */
const SEARCHABLE_FROM = 12

export type GenreMode = 'all' | 'any'

export function GenreFilter({
  genres,
  selected,
  mode,
  onToggle,
  onMode,
  idPrefix,
}: {
  /** As the counts list them: commonest first. */
  genres: Facet[]
  selected: string[]
  mode: GenreMode
  onToggle: (genre: string) => void
  onMode: (mode: GenreMode) => void
  idPrefix: string
}) {
  const { t, lang, locale } = useI18n()
  const [term, setTerm] = useState('')
  const [open, setOpen] = useState(false)
  const radios = useId()

  const label = (value: string) => genreLabel(value, lang)
  // Accents aside, so "science" finds "Science-fiction" and "Science Fiction".
  const fold = (text: string) => text.normalize('NFD').replace(/[̀-ͯ]/g, '').toLocaleLowerCase()
  const needle = fold(term.trim())

  // Chosen first, whatever their count — and one the address holds that the
  // counts do not list, so it can still be taken off.
  const chosen: { value: string; count?: number }[] = selected.map(
    (value) => genres.find((g) => g.value === value) ?? { value },
  )
  const rest = genres.filter((g) => !selected.includes(g.value))
  const found = needle
    ? rest.filter((g) => fold(label(g.value)).includes(needle) || fold(g.value).includes(needle))
    : null
  const commonest = rest.slice(0, COMMONEST)
  const others = [...rest.slice(COMMONEST)].sort((a, b) => label(a.value).localeCompare(label(b.value), locale))

  const chip = (genre: { value: string; count?: number }, on: boolean) => (
    <button
      key={genre.value}
      type="button"
      aria-pressed={on}
      onClick={() => onToggle(genre.value)}
      className={cn(
        'hit inline-flex min-h-8 cursor-pointer items-center gap-1.5 rounded-full border px-2.5 text-xs',
        'transition-colors duration-150',
        on
          ? 'border-vermillion bg-vermillion/15 text-bone'
          : 'border-rule-bright text-bone-dim hover:border-bone-faint hover:text-bone',
      )}
    >
      {on ? (
        <Glyph name="check" className="size-3 text-vermillion" />
      ) : (
        // The genre's own tint, as its chip wears it on a work's page.
        <span
          aria-hidden
          className="size-1.5 shrink-0 rounded-full"
          style={{ backgroundColor: `var(--color-stock-${stockOf(genre.value)})` }}
        />
      )}
      {label(genre.value)}
      {genre.count !== undefined ? (
        <span className="font-mono text-[0.625rem] text-bone-faint tabular-nums">{genre.count}</span>
      ) : null}
    </button>
  )

  return (
    <fieldset>
      <legend className="label mb-2">{t.work.genres}</legend>

      {genres.length >= SEARCHABLE_FROM ? (
        <div className="relative mb-3">
          <Glyph
            name="search"
            className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-bone-faint"
          />
          <Input
            id={`${idPrefix}-genre-search`}
            type="search"
            value={term}
            onChange={(event) => setTerm(event.target.value)}
            placeholder={t.browse.genresSearch}
            aria-label={t.browse.genresSearch}
            autoComplete="off"
            className="h-9 pl-8 text-sm"
          />
        </div>
      ) : null}

      <div className="flex flex-wrap gap-x-1.5 gap-y-3">
        {chosen.map((genre) => chip(genre, true))}
        {(found ?? commonest).map((genre) => chip(genre, false))}
      </div>
      {found && !found.length ? <p className="mt-2 text-xs text-bone-faint">{t.browse.genresNone}</p> : null}

      {!found && others.length ? (
        <details
          open={open}
          onToggle={(event) => setOpen((event.target as HTMLDetailsElement).open)}
          className="group mt-3"
        >
          <summary className="hit flex min-h-8 cursor-pointer list-none items-center gap-1 text-xs font-medium text-bone-dim transition-colors duration-150 hover:text-bone [&::-webkit-details-marker]:hidden">
            <Glyph name="chevronDown" className="size-3 transition-transform duration-200 group-open:rotate-180" />
            {t.browse.genresOthers(others.length)}
          </summary>
          <div className="mt-3 flex flex-wrap gap-x-1.5 gap-y-3">{others.map((genre) => chip(genre, false))}</div>
        </details>
      ) : null}

      {selected.length >= 2 ? (
        <div role="group" aria-label={t.browse.genreMode} className="mt-4">
          <span className="label mb-1.5 block">{t.browse.genreMode}</span>
          <div className="grid grid-cols-2 gap-1 rounded-full border border-rule p-1">
            {(['all', 'any'] as const).map((value) => (
              <label
                key={value}
                className={cn(
                  'hit relative grid min-h-9 cursor-pointer place-items-center rounded-full px-2 text-[0.8125rem]',
                  'transition-colors duration-150 has-focus-visible:outline-2 has-focus-visible:outline-vermillion',
                  mode === value ? 'bg-ink-top text-bone' : 'text-bone-dim hover:text-bone',
                )}
              >
                <input
                  type="radio"
                  name={`${idPrefix}-genre-mode-${radios}`}
                  value={value}
                  checked={mode === value}
                  onChange={() => onMode(value)}
                  className="sr-only"
                />
                {value === 'all' ? t.browse.genreAll : t.browse.genreAny}
              </label>
            ))}
          </div>
        </div>
      ) : null}
    </fieldset>
  )
}
