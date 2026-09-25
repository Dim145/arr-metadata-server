/**
 * Somebody's work, as this catalogue holds it — and who they are, as TMDB
 * has it.
 *
 * Laid out like the filmography at the back of a monograph: the year in the
 * margin, the title set in the display face, the part or the job in italics.
 * It is only ever the part of a career this server happens to hold — the
 * page says so, and links to TMDB for the rest. Above it, when TMDB answers,
 * the life in a few lines: when and where they were born, what they are known
 * for, a biography folded to its first lines, and a few portraits.
 */

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useParams } from 'react-router'

import { ExternalLink } from '../components/elsewhere'
import { Artwork } from '../components/media'
import { EmptyState, Glyph, Label, Skeleton } from '../components/ui'
import { ApiError, api, query } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { jobLabel } from '../lib/labels'
import { personLink } from '../lib/links'
import { poster } from '../lib/media'
import type { MediaItem, Person as PersonData, PersonDetails, Role } from '../lib/types'
import { NotFound } from './NotFound'

export function Person() {
  const { tmdbId = '' } = useParams()
  const { t, lang } = useI18n()

  const person = useQuery({
    queryKey: ['person', tmdbId, lang],
    queryFn: () => api.get<PersonData>(`/people/${encodeURIComponent(tmdbId)}${query({ language: lang })}`),
    enabled: /^\d+$/.test(tmdbId),
  })

  if (!/^\d+$/.test(tmdbId)) return <NotFound />
  if (person.isPending) return <PersonSkeleton />
  if (person.error instanceof ApiError && person.error.status === 404) return <NotFound />
  if (person.isError || !person.data) {
    return (
      <div className="pt-16">
        <EmptyState title={t.common.error} hint={t.person.loadFailed} />
      </div>
    )
  }

  return <Filmography person={person.data} />
}

function Filmography({ person }: { person: PersonData }) {
  const { t } = useI18n()
  useTitle(person.name)

  const byWork = new Map<string, Role[]>()
  for (const role of person.roles) {
    byWork.set(role.workId, [...(byWork.get(role.workId) ?? []), role])
  }

  // A guest star is cast too, not a job behind the camera.
  const onScreen = (type: string) => type === 'actor' || type === 'guest'
  const acting = person.roles.filter((r) => onScreen(r.creditType)).length
  const crew = person.roles.length - acting
  const details = person.details

  return (
    <article className="pt-10 pb-12">
      <header className="rise flex flex-col gap-6 sm:flex-row sm:items-end">
        <div className="w-32 shrink-0 sm:w-44">
          {person.image ? (
            <Artwork
              url={person.image}
              role="portrait"
              eager
              alt={t.a11y.headshot(person.name)}
              className="aspect-2/3 w-full rounded-plate border border-rule-bright object-cover shadow-[var(--shadow-plate)]"
            />
          ) : (
            <div className="grid aspect-2/3 w-full place-items-center rounded-plate border border-rule bg-ink-high">
              <Glyph name="user" className="size-8 text-bone-faint" />
            </div>
          )}
        </div>

        <div className="min-w-0">
          <Label>{t.person.label}</Label>
          <h1 className="mt-1 font-display text-4xl leading-tight font-medium text-bone sm:text-5xl">{person.name}</h1>
          <p className="mt-3 font-mono text-xs text-bone-faint tabular-nums">
            {[
              t.person.works(person.works.length),
              acting ? t.person.parts(acting) : undefined,
              crew ? t.person.jobs(crew) : undefined,
            ]
              .filter(Boolean)
              .join(' · ')}
          </p>
          {details ? <Vitals details={details} /> : null}
          <p className="mt-4 max-w-prose text-sm leading-relaxed text-bone-dim">
            {t.person.partial}{' '}
            <ExternalLink href={personLink(person.tmdbId)}>{t.person.onTmdb}</ExternalLink>
            {details?.imdbId ? (
              <>
                {' · '}
                <ExternalLink href={`https://www.imdb.com/name/${encodeURIComponent(details.imdbId)}/`}>
                  {t.person.onImdb}
                </ExternalLink>
              </>
            ) : null}
            {details?.homepage ? (
              <>
                {' · '}
                <ExternalLink href={details.homepage}>{t.person.website}</ExternalLink>
              </>
            ) : null}
          </p>
        </div>
      </header>

      {details?.biography ? <Biography text={details.biography} /> : null}
      {details?.alsoKnownAs?.length ? (
        <p className="mt-6 text-sm text-bone-dim">
          <span className="label mr-2">{t.person.alsoKnownAs}</span>
          <span className="font-mono text-xs text-bone-faint">{details.alsoKnownAs.join(' · ')}</span>
        </p>
      ) : null}
      {details?.photos && details.photos.length > 1 ? <Portraits name={person.name} photos={details.photos} /> : null}

      <ol className="stagger mt-10 divide-y divide-rule border-y border-rule">
        {person.works.map((work) => (
          <Entry key={work.id} work={work} roles={byWork.get(work.id) ?? []} />
        ))}
      </ol>
    </article>
  )
}

/** When and where they were born, when they died, what they are known for. */
function Vitals({ details }: { details: PersonDetails }) {
  const { t, locale } = useI18n()
  const born = details.birthday ? fmt.longDate(details.birthday, locale) : undefined
  const died = details.deathday ? fmt.longDate(details.deathday, locale) : undefined
  const age = details.birthday ? ageBetween(details.birthday, details.deathday) : undefined

  const lines = [
    born
      ? [
          t.person.born(born),
          details.placeOfBirth ? t.person.in(details.placeOfBirth) : undefined,
          age !== undefined && !details.deathday ? `(${t.person.aged(age)})` : undefined,
        ]
          .filter(Boolean)
          .join(' ')
      : undefined,
    died ? [t.person.died(died), age !== undefined ? t.person.at(age) : undefined].filter(Boolean).join(' ') : undefined,
    details.knownFor ? t.person.knownFor(t.person.department[details.knownFor] ?? details.knownFor) : undefined,
  ].filter(Boolean) as string[]
  if (!lines.length) return null

  return (
    <ul className="mt-3 space-y-0.5 text-sm text-bone-dim">
      {lines.map((line) => (
        <li key={line}>{line}</li>
      ))}
    </ul>
  )
}

/** Whole years from one day to another — today, when the second is not given. */
export function ageBetween(from: string, until?: string): number | undefined {
  const start = new Date(`${from}T00:00:00Z`)
  const end = until ? new Date(`${until}T00:00:00Z`) : new Date()
  if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime()) || end < start) return undefined
  let years = end.getUTCFullYear() - start.getUTCFullYear()
  const beforeBirthday =
    end.getUTCMonth() < start.getUTCMonth() ||
    (end.getUTCMonth() === start.getUTCMonth() && end.getUTCDate() < start.getUTCDate())
  if (beforeBirthday) years -= 1
  return years
}

/** The biography, folded to its first lines until asked for whole. */
function Biography({ text }: { text: string }) {
  const { t } = useI18n()
  const [whole, setWhole] = useState(false)
  // Short enough to show whole: a fold that hides one line is a tease.
  const long = text.length > 700

  return (
    <section className="rise mt-8 max-w-prose" style={{ animationDelay: '80ms' }}>
      <Label>{t.person.biography}</Label>
      <p className={cn('mt-2 text-sm leading-relaxed whitespace-pre-line text-bone-dim', long && !whole && 'line-clamp-6')}>
        {text}
      </p>
      {long ? (
        <button
          type="button"
          onClick={() => setWhole((v) => !v)}
          aria-expanded={whole}
          className="mt-2 text-xs text-vermillion underline-offset-2 hover:underline"
        >
          {whole ? t.person.readLess : t.person.readMore}
        </button>
      ) : null}
    </section>
  )
}

/** A few portraits, in a row that scrolls sideways. */
function Portraits({ name, photos }: { name: string; photos: string[] }) {
  const { t } = useI18n()
  return (
    <section className="mt-8">
      <Label>{t.person.portraits}</Label>
      <ul className="mt-3 flex gap-3 overflow-x-auto pb-2" aria-label={t.person.portraits}>
        {photos.map((url, index) => (
          <li key={url} className="w-20 shrink-0 sm:w-24">
            <Artwork
              url={url}
              role="thumb"
              alt={t.person.portrait(name, index + 1)}
              className="aspect-2/3 w-full rounded-card border border-rule object-cover"
            />
          </li>
        ))}
      </ul>
    </section>
  )
}

/** One line of the filmography. */
function Entry({ work, roles }: { work: MediaItem; roles: Role[] }) {
  const { t, lang } = useI18n()
  const art = poster(work)

  // A job in words: TMDB files a director with no job title of its own, and
  // "director" is the name of a type, not how either language says it.
  const jobs = t.admin.editor.children as unknown as Record<string, unknown>
  const job = (type: string) => (typeof jobs[type] === 'string' ? (jobs[type] as string) : type)

  const parts = roles
    .map((role) =>
      role.creditType === 'actor' || role.creditType === 'guest'
        ? role.character
          ? t.person.as(role.character)
          : t.person.cast
        : role.character
          ? jobLabel(role.character, lang)
          : job(role.creditType),
    )
    .filter((text, index, all) => all.indexOf(text) === index)

  return (
    <li>
      <Link
        to={`/work/${work.id}`}
        className="group grid grid-cols-[3.5rem_2.5rem_minmax(0,1fr)] items-center gap-4 py-3 transition-colors duration-150 hover:bg-ink-raised sm:grid-cols-[4.5rem_3rem_minmax(0,1fr)]"
      >
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
          <p className="truncate text-sm text-bone-faint italic">
            {parts.join(' · ')}
            <span className="not-italic">
              {' · '}
              {work.kind === 'series' ? t.browse.series : t.browse.films}
            </span>
          </p>
        </div>
      </Link>
    </li>
  )
}

function PersonSkeleton() {
  return (
    <div className="space-y-6 pt-10">
      <div className="flex gap-6">
        <Skeleton className="aspect-2/3 w-32 sm:w-44" />
        <div className="flex-1 space-y-3 pt-20">
          <Skeleton className="h-10 w-1/2" />
          <Skeleton className="h-4 w-1/3" />
        </div>
      </div>
      {Array.from({ length: 5 }, (_, index) => (
        <Skeleton key={index} className="h-16 w-full" />
      ))}
    </div>
  )
}
