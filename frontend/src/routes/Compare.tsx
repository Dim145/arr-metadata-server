/**
 * Two works side by side: their scores source by source, how long they run,
 * how many seasons and episodes a series has, their years and genres — and the
 * people they share, which is what comparing two works is mostly for: the
 * director of one who produced the other, the actor in both.
 *
 * Both are in the address, `?a=…&b=…`, so a comparison is a link. Either is
 * chosen, or changed, by searching the catalogue in its own column.
 */

import { useQuery } from '@tanstack/react-query'
import { useId, useState } from 'react'
import { Link, useSearchParams } from 'react-router'

import { Artwork, Placeholder } from '../components/media'
import { Button, EmptyState, Genre, Glyph, Input, Label, SectionTitle, Skeleton } from '../components/ui'
import { api, query, segment } from '../lib/api'
import { useSettled } from '../lib/debounce'
import { genrePath, networkPath, yearPath } from '../lib/facets'
import * as fmt from '../lib/format'
import { useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { listedGenres, providerName } from '../lib/labels'
import { poster, ratingsOf } from '../lib/media'
import type { Comparison, ItemPage, MediaItem, SharedPerson } from '../lib/types'

type Side = 'a' | 'b'

export function Compare() {
  const { t, lang } = useI18n()
  const [params, setParams] = useSearchParams()
  const a = (params.get('a') ?? '').trim()
  const b = (params.get('b') ?? '').trim()
  useTitle(t.compare.title)

  const choose = (side: Side, id: string) => {
    const next = new URLSearchParams(window.location.search)
    if (id) next.set(side, id)
    else next.delete(side)
    setParams(next, { replace: true })
  }

  const both = Boolean(a && b && a !== b)
  const comparison = useQuery({
    queryKey: ['compare', a, b, lang],
    queryFn: () => api.get<Comparison>(`/compare${query({ a, b, language: lang })}`),
    enabled: both,
  })
  const data = both ? comparison.data : undefined

  return (
    <div className="pt-10 pb-12">
      <header className="mb-8">
        <Label>{t.compare.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.compare.heading}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.compare.lead}</p>
      </header>

      <div className="grid gap-6 sm:grid-cols-2">
        {(['a', 'b'] as const).map((side) => {
          const id = side === 'a' ? a : b
          const work = data ? (side === 'a' ? data.first.work : data.second.work) : undefined
          return (
            <Slot
              key={side}
              side={side}
              id={id}
              work={work}
              other={side === 'a' ? b : a}
              onChoose={(chosen) => choose(side, chosen)}
            />
          )
        })}
      </div>

      {a && b && a === b ? (
        <p role="alert" className="mt-6 text-sm text-vermillion">
          {t.compare.same}
        </p>
      ) : null}

      {both && comparison.isPending ? (
        <div aria-hidden className="mt-12 space-y-3">
          {Array.from({ length: 6 }, (_, index) => (
            <Skeleton key={index} className="h-8 w-full" />
          ))}
        </div>
      ) : null}

      {both && comparison.isError ? (
        <div className="mt-12">
          <EmptyState
            kind="error"
            title={t.common.error}
            hint={t.compare.loadFailed}
            action={
              <Button size="sm" onClick={() => void comparison.refetch()}>
                <Glyph name="refresh" className="size-3.5" />
                {t.common.retry}
              </Button>
            }
          />
        </div>
      ) : null}

      {data ? <Table comparison={data} /> : null}
      {data ? <Shared comparison={data} /> : null}
    </div>
  )
}

/**
 * One of the two columns: the work chosen, with the way to change it, or a
 * search of the catalogue to choose one.
 */
function Slot({
  side,
  id,
  work,
  other,
  onChoose,
}: {
  side: Side
  id: string
  work?: MediaItem
  other: string
  onChoose: (id: string) => void
}) {
  const { t, lang } = useI18n()
  // The work alone, while the other column waits for its own.
  const alone = useQuery({
    queryKey: ['compare-side', id, lang],
    queryFn: () => api.get<MediaItem>(`/items/${segment(id)}${query({ language: lang })}`),
    enabled: Boolean(id) && !work,
    staleTime: 5 * 60_000,
    retry: false,
  })
  const shown = work ?? alone.data
  const label = side === 'a' ? t.compare.first : t.compare.second

  if (id && shown) {
    const art = poster(shown)
    return (
      <section aria-label={label} className="flex gap-4 rounded-plate border border-rule p-4">
        <Link to={`/work/${shown.id}`} className="shrink-0">
          {art ? (
            <Artwork url={art} role="poster" alt={t.a11y.poster(shown.title)} className="w-24 rounded-card border border-rule sm:w-28" />
          ) : (
            <Placeholder title={shown.title} kind={shown.kind} label={t.a11y.poster(shown.title)} className="aspect-2/3 w-24 rounded-card border border-rule sm:w-28" />
          )}
        </Link>
        <div className="min-w-0">
          <Label>{label}</Label>
          <h2 className="mt-1.5 font-display text-xl leading-tight font-medium text-bone sm:text-2xl">
            <Link to={`/work/${shown.id}`} className="hover:underline">
              {shown.title}
            </Link>
          </h2>
          <p className="mt-1 font-mono text-sm text-bone-faint tabular-nums">
            {[shown.year, shown.kind === 'series' ? t.home.kindSeries : t.home.kindFilm].filter(Boolean).join(' · ')}
          </p>
          <Button size="sm" variant="quiet" className="mt-3" onClick={() => onChoose('')}>
            <Glyph name="close" className="size-3.5" />
            {t.compare.change}
          </Button>
        </div>
      </section>
    )
  }

  if (id && alone.isPending) {
    return <Skeleton className="h-40 w-full rounded-plate" />
  }

  return <Picker label={label} other={other} onChoose={onChoose} missing={Boolean(id) && alone.isError} />
}

/** A search of the catalogue, its first works offered to choose from. */
function Picker({
  label,
  other,
  onChoose,
  missing,
}: {
  label: string
  other: string
  onChoose: (id: string) => void
  missing: boolean
}) {
  const { t, lang } = useI18n()
  const [term, setTerm] = useState('')
  const settled = useSettled(term.trim())
  const field = useId()
  const found = useQuery({
    queryKey: ['compare-search', settled, lang],
    queryFn: () => api.get<ItemPage>(`/items${query({ term: settled, limit: 8, language: lang })}`),
    enabled: settled.length >= 2,
    placeholderData: (previous) => previous,
  })
  const items = (found.data?.items ?? []).filter((item) => item.id !== other)

  return (
    <section aria-label={label} className="rounded-plate border border-dashed border-rule-bright p-4">
      <label htmlFor={field} className="label mb-2 block">
        {label}
      </label>
      <Input
        id={field}
        type="search"
        value={term}
        onChange={(event) => setTerm(event.target.value)}
        placeholder={t.compare.search}
        autoComplete="off"
      />
      {missing ? <p className="mt-2 text-xs text-vermillion">{t.compare.missing}</p> : null}
      {settled.length >= 2 ? (
        <ul className="mt-3 divide-y divide-rule" aria-live="polite" aria-busy={found.isFetching || undefined}>
          {items.map((item) => (
            <li key={item.id}>
              <button
                type="button"
                onClick={() => onChoose(item.id)}
                className="flex min-h-11 w-full cursor-pointer items-baseline justify-between gap-3 px-1 text-left text-sm text-bone transition-colors duration-150 hover:text-vermillion"
              >
                <span className="truncate">{item.title}</span>
                <span className="shrink-0 font-mono text-xs text-bone-faint tabular-nums">
                  {[item.year, item.kind === 'series' ? t.home.kindSeries : t.home.kindFilm].filter(Boolean).join(' · ')}
                </span>
              </button>
            </li>
          ))}
          {found.isSuccess && !items.length ? <li className="py-3 text-sm text-bone-faint">{t.compare.nothing}</li> : null}
        </ul>
      ) : null}
    </section>
  )
}

/** The two works' facts, a row each, the two side by side. */
function Table({ comparison }: { comparison: Comparison }) {
  const { t, locale } = useI18n()
  const { first, second } = comparison
  const works = [first.work, second.work]

  // Every source either is scored by, the one the cards lead with first.
  const sources: string[] = []
  for (const work of works) {
    for (const rating of ratingsOf(work.ratings)) {
      if (!sources.includes(rating.source)) sources.push(rating.source)
    }
  }
  const score = (work: MediaItem, source: string) => ratingsOf(work.ratings).find((r) => r.source === source)

  const maker = (work: MediaItem) => work.network ?? work.studio
  const rows: { label: string; cells: [React.ReactNode, React.ReactNode] }[] = [
    {
      label: t.compare.year,
      cells: [first, second].map((side) =>
        side.work.year ? (
          <Link key={side.work.id} to={yearPath(side.work.year)} className="tabular-nums hover:underline">
            {side.work.year}
          </Link>
        ) : (
          '—'
        ),
      ) as [React.ReactNode, React.ReactNode],
    },
    {
      label: t.compare.runtime,
      cells: [fmt.runtime(first.work.runtime, locale) ?? '—', fmt.runtime(second.work.runtime, locale) ?? '—'],
    },
  ]
  if (first.seasons !== undefined || second.seasons !== undefined) {
    rows.push({
      label: t.compare.seasons,
      cells: [first, second].map((side) => (side.seasons === undefined ? '—' : fmt.count(side.seasons, locale))) as [
        React.ReactNode,
        React.ReactNode,
      ],
    })
    rows.push({
      label: t.compare.episodes,
      cells: [first, second].map((side) => (side.episodes === undefined ? '—' : fmt.count(side.episodes, locale))) as [
        React.ReactNode,
        React.ReactNode,
      ],
    })
  }
  rows.push({
    label: t.compare.network,
    cells: works.map((work) => {
      const name = maker(work)
      return name ? (
        <Link key={work.id} to={networkPath(name)} className="hover:underline">
          {name}
        </Link>
      ) : (
        '—'
      )
    }) as [React.ReactNode, React.ReactNode],
  })
  rows.push({
    label: t.compare.genres,
    cells: works.map((work) => (
      <span key={work.id} className="flex flex-wrap gap-1.5">
        {listedGenres(work.genres).map((genre) => (
          <Genre key={genre} name={genre} to={genrePath(genre)} />
        ))}
      </span>
    )) as [React.ReactNode, React.ReactNode],
  })
  for (const source of sources) {
    rows.push({
      label: providerName(source),
      cells: works.map((work) => {
        const rating = score(work, source)
        if (!rating?.value) return '—'
        const other = works.map((w) => score(w, source)?.value ?? 0)
        const ahead = rating.value === Math.max(...other) && other[0] !== other[1]
        return (
          <span key={work.id} className="flex items-center gap-3">
            <span className={ahead ? 'font-medium text-bone' : 'text-bone-dim'}>{fmt.score(rating.value, locale)}</span>
            <span aria-hidden className="h-1.5 w-20 overflow-hidden rounded-full bg-ink-high sm:w-28">
              <span className="block h-full rounded-full bg-bone-dim" style={{ width: `${Math.min(100, rating.value * 10)}%` }} />
            </span>
            {rating.votes ? <span className="hidden font-mono text-xs text-bone-faint tabular-nums sm:inline">{t.compare.votes(fmt.count(rating.votes, locale))}</span> : null}
          </span>
        )
      }) as [React.ReactNode, React.ReactNode],
    })
  }

  return (
    <section aria-label={t.compare.facts} className="mt-12">
      <SectionTitle>{t.compare.facts}</SectionTitle>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-rule">
              <td className="py-2 pr-4" />
              {works.map((work) => (
                <th key={work.id} scope="col" className="py-2 pr-4 font-display text-base font-medium text-bone">
                  {work.title}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.label} className="border-b border-rule align-top">
                <th scope="row" className="label py-3 pr-4 font-normal whitespace-nowrap">
                  {row.label}
                </th>
                <td className="py-3 pr-4 text-bone">{row.cells[0]}</td>
                <td className="py-3 pr-4 text-bone">{row.cells[1]}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}

/** The people both works credit, and what each did on each. */
function Shared({ comparison }: { comparison: Comparison }) {
  const { t } = useI18n()
  const { first, second, shared } = comparison

  return (
    <section aria-label={t.compare.shared} className="mt-14">
      <SectionTitle>{t.compare.shared}</SectionTitle>
      {shared.length ? (
        <ul className="divide-y divide-rule border-y border-rule">
          {shared.map((person) => (
            <li key={person.personId} className="grid gap-1 py-3 sm:grid-cols-[minmax(0,14rem)_minmax(0,1fr)_minmax(0,1fr)] sm:gap-4">
              <Link to={`/people/${person.personId}`} className="font-medium text-bone hover:underline">
                {person.name}
              </Link>
              <Parts person={person} which="first" title={first.work.title} />
              <Parts person={person} which="second" title={second.work.title} />
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-sm text-bone-faint">{t.compare.sharedNone}</p>
      )}
    </section>
  )
}

/** What somebody did on one of the two works. */
function Parts({ person, which, title }: { person: SharedPerson; which: 'first' | 'second'; title: string }) {
  const { t } = useI18n()
  const parts = person[which]
  return (
    <p className="text-sm text-bone-dim">
      <span className="sr-only">{t.compare.on(title)} </span>
      {parts
        .map((part) => {
          const job = t.compare.jobs[part.creditType] ?? part.creditType
          return part.character ? `${job} (${part.character})` : job
        })
        .join(', ')}
    </p>
  )
}
