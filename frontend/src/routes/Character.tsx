/**
 * A character, and the works of the catalogue it is in.
 *
 * Laid out as a person's filmography is: the part itself above — its
 * picture, its name in its own script too, where AniList and MyAnimeList
 * keep it — then a line for each work it is in, the newest first: the year
 * in the margin, the poster, the title, what it is there, who plays it, and
 * who voices it in each dub. A character is one for every work it is in, so
 * a film and its series list it once each, on the same page.
 *
 * It is only ever the works this server holds — the page says so — and a
 * work the reader may not see is not listed; a character in none of them is
 * nobody's page.
 */

import { useQuery } from '@tanstack/react-query'
import { Fragment } from 'react'
import { Link, useParams } from 'react-router'

import { ExternalLink } from '../components/elsewhere'
import { Artwork } from '../components/media'
import { Button, EmptyState, Glyph, Label, Skeleton } from '../components/ui'
import { ApiError, api, query, segment } from '../lib/api'
import { cn } from '../lib/cn'
import { useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { languageName } from '../lib/labels'
import { poster } from '../lib/media'
import type { CharacterAppearance, CharacterPage, CharacterVoice, MediaItem } from '../lib/types'
import { NotFound } from './NotFound'

export function Character() {
  const { id = '' } = useParams()
  const { t, lang } = useI18n()

  const page = useQuery({
    queryKey: ['character', id, lang],
    queryFn: () => api.get<CharacterPage>(`/characters/${segment(id)}${query({ language: lang })}`),
    enabled: id !== '',
  })

  if (!id) return <NotFound />
  if (page.isPending) return <CharacterSkeleton />
  if (page.error instanceof ApiError && page.error.status === 404) return <NotFound />
  if (page.isError || !page.data) {
    return (
      <div className="pt-16">
        <EmptyState
          kind="error"
          title={t.common.error}
          hint={t.character.loadFailed}
          action={
            <Button size="sm" onClick={() => void page.refetch()}>
              <Glyph name="refresh" className="size-3.5" />
              {t.common.retry}
            </Button>
          }
        />
      </div>
    )
  }

  return <Appearances character={page.data} />
}

function Appearances({ character }: { character: CharacterPage }) {
  const { t } = useI18n()
  const c = t.character
  useTitle(character.name)

  const works = new Map(character.works.map((work) => [work.id, work]))
  const elsewhere = [
    character.anilistId ? (
      <ExternalLink key="anilist" href={`https://anilist.co/character/${character.anilistId}`}>
        {c.onAnilist}
      </ExternalLink>
    ) : null,
    character.malId ? (
      <ExternalLink key="mal" href={`https://myanimelist.net/character/${character.malId}`}>
        {c.onMal}
      </ExternalLink>
    ) : null,
  ].filter((link) => link !== null)

  return (
    <article className="pt-10 pb-12">
      <header className="rise flex flex-col gap-6 sm:flex-row sm:items-end">
        <div className="w-32 shrink-0 sm:w-44">
          {character.image ? (
            <Artwork
              url={character.image}
              role="portrait"
              eager
              alt={c.picture(character.name)}
              className="aspect-2/3 w-full rounded-plate border border-rule-bright object-cover shadow-[var(--shadow-plate)]"
            />
          ) : (
            <div className="grid aspect-2/3 w-full place-items-center rounded-plate border border-rule bg-ink-high">
              <Glyph name="user" className="size-8 text-bone-faint" />
            </div>
          )}
        </div>

        <div className="min-w-0">
          <Label>{c.label}</Label>
          <h1 className="mt-1 font-display text-4xl leading-tight font-medium text-bone sm:text-5xl">{character.name}</h1>
          {/* In its own script, said in its own language where it is known,
              so that a voice reads フリーレン as Japanese. */}
          {character.nativeName ? (
            <p lang={character.nativeNameLang} className="mt-1 font-display text-xl text-bone-dim">
              {character.nativeName}
            </p>
          ) : null}
          <p className="mt-3 font-mono text-xs text-bone-faint tabular-nums">{c.works(character.appearances.length)}</p>
          <p className="mt-4 max-w-prose text-sm leading-relaxed text-bone-dim">
            {c.partial}
            {elsewhere.map((link, index) => (
              <Fragment key={index}>
                {index === 0 ? ' ' : ' · '}
                {link}
              </Fragment>
            ))}
          </p>
        </div>
      </header>

      <ol className="stagger mt-10 divide-y divide-rule border-y border-rule">
        {character.appearances.flatMap((appearance) => {
          const work = works.get(appearance.workId)
          return work ? [<Entry key={work.id} work={work} appearance={appearance} />] : []
        })}
      </ol>
    </article>
  )
}

/** The dubs of one work, a language at a time, in the order they came. */
function byLanguage(voices: CharacterVoice[]): [string, CharacterVoice[]][] {
  const grouped = new Map<string, CharacterVoice[]>()
  for (const voice of voices) {
    const same = grouped.get(voice.language) ?? []
    same.push(voice)
    grouped.set(voice.language, same)
  }
  return [...grouped.entries()].map(([language, same]) => [language, [...same].sort((a, b) => a.sortOrder - b.sortOrder)])
}

/** One work the character is in: what it is there, who plays it, who voices it. */
function Entry({ work, appearance }: { work: MediaItem; appearance: CharacterAppearance }) {
  const { t, locale } = useI18n()
  const c = t.character
  const art = poster(work)
  const performer = appearance.performer
  const dubs = byLanguage(appearance.voices ?? [])
  const tail = [appearance.role ? c.roles[appearance.role] : undefined, work.kind === 'series' ? t.home.kindSeries : t.home.kindFilm]
    .filter(Boolean)
    .join(' · ')
  const columns = 'grid grid-cols-[3.5rem_2.5rem_minmax(0,1fr)] gap-x-4 sm:grid-cols-[4.5rem_3rem_minmax(0,1fr)]'

  return (
    <li>
      <Link to={`/work/${work.id}`} className={cn(columns, 'group items-center py-3 transition-colors duration-150 hover:bg-ink-raised')}>
        <span className="font-mono text-sm text-bone-faint tabular-nums">{work.year ?? '—'}</span>
        <div className="aspect-2/3 w-full overflow-hidden rounded-card border border-rule bg-ink-high">
          {art ? (
            <Artwork url={art} role="card" alt="" className="size-full object-cover" />
          ) : (
            <div className="grid size-full place-items-center">
              <Glyph name={work.kind === 'series' ? 'tv' : 'film'} className="size-3.5 text-bone-faint" />
            </div>
          )}
        </div>
        <div className="min-w-0">
          <p className="truncate font-display text-lg leading-snug text-bone transition-colors duration-150 group-hover:text-vermillion">
            {work.title}
          </p>
          <p className="truncate text-sm text-bone-faint">{tail}</p>
        </div>
      </Link>

      {/* Who plays it and who voices it, under the line: links of their own. */}
      <div className={cn(columns, 'pb-3')}>
        <dl className="col-start-3 min-w-0 space-y-0.5 text-sm">
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
            <dt className="label">{c.performer}</dt>
            <dd className="min-w-0 text-bone-dim">
              {performer ? (
                <Link
                  to={`/people/${encodeURIComponent(performer.id)}`}
                  className="underline decoration-rule-bright underline-offset-4 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
                >
                  {performer.name}
                </Link>
              ) : (
                <span className="text-bone-faint italic">{c.performerUnknown}</span>
              )}
            </dd>
          </div>
          {dubs.map(([language, voices]) => (
            <div key={language} className="flex min-w-0 flex-wrap items-baseline gap-x-2">
              <dt className="label">{c.voice(languageName(language, locale) ?? language)}</dt>
              <dd className="min-w-0 text-bone-dim">{voices.map((voice) => voice.name).join(', ')}</dd>
            </div>
          ))}
        </dl>
      </div>
    </li>
  )
}

function CharacterSkeleton() {
  return (
    <div className="space-y-6 pt-10">
      <div className="flex gap-6">
        <Skeleton className="aspect-2/3 w-32 sm:w-44" />
        <div className="flex-1 space-y-3 pt-20">
          <Skeleton className="h-10 w-1/2" />
          <Skeleton className="h-4 w-1/3" />
        </div>
      </div>
      {Array.from({ length: 3 }, (_, index) => (
        <Skeleton key={index} className="h-16 w-full" />
      ))}
    </div>
  )
}
