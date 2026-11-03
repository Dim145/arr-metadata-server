/**
 * A work's characters, under its cast: each with its picture, what it is to
 * the work — a lead, one beside them, one in the background — who plays it,
 * and who voices it in the dub the reader chose; each the way to its own
 * page, where every work of the catalogue it is in is listed.
 *
 * Who plays a character is the credit the server names among the work's
 * own; one a lock hides is no performer to show, and the character says its
 * performer is unknown. The dubs are offered the work's own language first —
 * an anime's original voices — then the reader's, the reader's chosen first
 * where the characters are voiced in it; the original voice is not said
 * twice where it is the performer already.
 */

import { useState } from 'react'
import { Link } from 'react-router'

import { Artwork } from '../../components/media'
import { Glyph, SectionTitle, Select } from '../../components/ui'
import { characterPath, dubsOf, firstDub, performerOf, voicesIn } from '../../lib/characters'
import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import { languageName } from '../../lib/labels'
import type { CharacterCredit, Credit, MediaItem } from '../../lib/types'
import { Fold } from './fold'
import { SECTION_MARGIN } from './record'

/** How many characters show before the rest are folded, as the cast does. */
const SHOWN_CHARACTERS = 6

/** The language a name in a work's own script is in, as a page's `lang` takes it: two letters where it has them. */
function langOf(code: string | null | undefined): string | undefined {
  const base = code?.trim().toLowerCase().split(/[-_,]/)[0]
  if (!base) return undefined
  try {
    return new Intl.Locale(base).language
  } catch {
    return undefined
  }
}

export function Characters({ item }: { item: MediaItem }) {
  const { t, lang, locale } = useI18n()
  const c = t.character
  const characters = item.characters ?? []
  const named = (language: string) => languageName(language, locale) ?? language
  const dubs = dubsOf(characters, item.originalLanguage, lang, named)
  const [chosen, setChosen] = useState<string>()
  const dub = chosen && dubs.includes(chosen) ? chosen : firstDub(dubs, item.originalLanguage, lang)

  if (!characters.length) return null

  const credits = item.credits ?? []
  const byCredit = item.people?.byCredit
  const nativeLang = langOf(item.originalLanguage)
  const grid = 'grid gap-x-8 gap-y-6 sm:grid-cols-2 xl:grid-cols-3'
  const rest = characters.slice(SHOWN_CHARACTERS)

  const row = (one: CharacterCredit) => (
    <CharacterRow
      key={one.character.id || one.character.name}
      one={one}
      performer={performerOf(one, credits)}
      person={(credit: Credit) => byCredit?.[credit.id]?.personId ?? undefined}
      dub={dub}
      dubName={dub ? named(dub) : undefined}
      nativeLang={nativeLang}
    />
  )

  return (
    <section id="characters" className={SECTION_MARGIN}>
      <SectionTitle
        action={
          dubs.length > 1 ? (
            <label className="flex shrink-0 items-center gap-2 text-xs text-bone-faint">
              {c.dub}
              <Select value={dub} onChange={(event) => setChosen(event.target.value)} className="min-h-9 w-auto text-sm">
                {dubs.map((language) => (
                  <option key={language} value={language}>
                    {named(language)}
                  </option>
                ))}
              </Select>
            </label>
          ) : null
        }
      >
        {c.section}
      </SectionTitle>
      <p className="-mt-3 mb-5 max-w-prose text-xs text-bone-faint">{c.sectionLead}</p>
      <ul className={cn('stagger', grid)}>{characters.slice(0, SHOWN_CHARACTERS).map(row)}</ul>
      {rest.length ? (
        <Fold label={c.whole(characters.length)} className="mt-6">
          <ul className={cn('stagger mt-5', grid)}>{rest.map(row)}</ul>
        </Fold>
      ) : null}
    </section>
  )
}

function CharacterRow({
  one,
  performer,
  person,
  dub,
  dubName,
  nativeLang,
}: {
  one: CharacterCredit
  performer: Credit | undefined
  /** The record a credit is filed under, where the work says. */
  person: (credit: Credit) => string | undefined
  dub: string | undefined
  dubName: string | undefined
  nativeLang: string | undefined
}) {
  const { t } = useI18n()
  const c = t.character
  const { character } = one
  const voices = voicesIn(one, dub, performer)

  // Decorative: its name, the way to its page, stands beside it.
  const picture = character.image ? (
    <Artwork
      url={character.image}
      role="headshot"
      alt=""
      className="h-[4.5rem] w-12 shrink-0 rounded-card border border-rule object-cover"
    />
  ) : (
    <div className="grid h-[4.5rem] w-12 shrink-0 place-items-center rounded-card border border-rule bg-ink-high">
      <Glyph name="user" className="size-4 text-bone-faint" />
    </div>
  )

  // The performer's page: by TMDB's id, as the cast's, or by their record.
  const record = performer ? person(performer) : undefined
  const performerPage = performer?.tmdbPersonId
    ? `/person/${performer.tmdbPersonId}`
    : record
      ? `/people/${encodeURIComponent(record)}`
      : undefined

  return (
    <li className="flex min-w-0 items-start gap-3.5">
      {picture}
      <div className="min-w-0">
        <p className="flex flex-wrap items-baseline gap-x-2 text-sm font-medium text-bone">
          {character.id ? (
            <Link
              to={characterPath(character.id)}
              className="underline decoration-rule-bright underline-offset-4 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
            >
              {character.name}
            </Link>
          ) : (
            character.name
          )}
          {one.role ? <span className="label text-bone-faint">{c.roles[one.role] ?? one.role}</span> : null}
        </p>
        {character.nativeName && character.nativeName !== character.name ? (
          <p lang={nativeLang} className="truncate text-xs text-bone-faint">
            {character.nativeName}
          </p>
        ) : null}
        {performer ? (
          <p className="mt-0.5 truncate text-sm text-bone-dim">
            <span className="label mr-1.5">{c.performer}</span>
            {performerPage ? (
              <Link to={performerPage} className="transition-colors duration-150 hover:text-vermillion">
                {performer.personName}
              </Link>
            ) : (
              performer.personName
            )}
          </p>
        ) : (
          <p className="mt-0.5 text-xs text-bone-faint italic">{c.performerUnknown}</p>
        )}
        {voices.length && dubName ? (
          <p className="truncate text-xs text-bone-dim">
            <span className="label mr-1.5">{c.voice(dubName)}</span>
            {voices.map((voice) => voice.name).join(', ')}
          </p>
        ) : null}
      </div>
    </li>
  )
}
