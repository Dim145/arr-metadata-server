/**
 * Finding a work at a provider, and pulling it in.
 *
 * The catalogue otherwise fills itself — a client asks for something and this
 * server fetches it — which is the right default and a poor way to add one
 * particular film, since it would mean making Radarr ask for it.
 *
 * Everything on this screen is a call to TMDB, TheTVDB and Fankai over the network, so
 * nothing here happens on a keystroke: you say what you want, the screen says
 * it is asking, and it waits. A search that fired as you typed would open four
 * conversations with two providers to answer none of them.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  EmptyState,
  Glyph,
  Input,
  Label,
  Panel,
  PanelHead,
  Select,
  Spinner,
} from '../../components/ui'
import { Artwork } from '../../components/media'
import { api, query } from '../../lib/api'
import { useI18n } from '../../lib/i18n'
import { providerName } from '../../lib/labels'
import type { Found, MediaItem, MediaKind, SourceRules } from '../../lib/types'

/**
 * A term the server looks up rather than searches — `tvdb:81189`, or a page's
 * address — names its own source, and the one chosen beside it is not asked.
 */
function isLookup(term: string) {
  return (
    /^\s*(tvdb|tvdbid|tmdb|tmdbid|mal|myanimelist|anilist|fankai|fan-kai|fankaiid|imdb|imdbid)\s*:\s*\S/i.test(term) ||
    /^\s*https?:\/\//i.test(term)
  )
}

/** Mirrors the server's: who can be searched alone, for which kind. */
const SEARCHABLE: { id: string; kinds: MediaKind[] }[] = [
  { id: 'tmdb', kinds: ['series', 'movie'] },
  { id: 'tvdb', kinds: ['series'] },
  { id: 'skyhook', kinds: ['series'] },
  { id: 'fankai', kinds: ['series'] },
  { id: 'radarr', kinds: ['movie'] },
]

/** What a hit is, across searches: whichever identifiers the provider gave. */
function keyOf(found: Found) {
  return `${found.kind}:${found.tmdbId ?? ''}:${found.tvdbId ?? ''}:${found.imdbId ?? ''}:${found.fankaiId ?? ''}`
}

/** Whether the import route has anything to fetch this one by. */
function fetchable(found: Found) {
  return found.kind === 'series'
    ? Boolean(found.tvdbId || found.tmdbId || found.imdbId || found.fankaiId)
    : Boolean(found.tmdbId || found.imdbId)
}

export function Discover() {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const [term, setTerm] = useState('')
  const [kind, setKind] = useState<'' | MediaKind>('')
  const [year, setYear] = useState('')
  const [source, setSource] = useState('')

  // What was actually asked, as opposed to what is in the fields. The two
  // differ for as long as somebody is typing, which is the point.
  const [asked, setAsked] = useState<{ term: string; kind: string; year: string; source: string } | null>(null)
  const [imported, setImported] = useState<Record<string, string>>({})

  const results = useQuery({
    queryKey: ['discover', asked],
    queryFn: () =>
      asked
        ? api.get<Found[]>(
            `/discover${query({ term: asked.term, kind: asked.kind, year: asked.year, source: asked.source })}`,
          )
        : Promise.resolve([]),
    enabled: asked !== null,
    // A provider that has just failed will fail again, and somebody is sitting
    // here watching the spinner while it does.
    retry: false,
    staleTime: 5 * 60_000,
  })

  // Which sources answer at all, to offer the rest as switched off.
  const rules = useQuery({
    queryKey: ['sources', 'rules'],
    queryFn: () => api.get<SourceRules>('/sources/rules'),
    staleTime: 60_000,
  })
  const on = (id: string) => rules.data?.providers.find((p) => p.id === id)?.on ?? true
  // A page of Fankai's website: numbered otherwise than the ids here.
  const fankaiPage = /^\s*https?:\/\/(www\.)?fankai\.fr\/productions\//i.test(term)
  const offered = SEARCHABLE.filter((s) => kind === '' || s.kinds.includes(kind))

  const take = useMutation({
    mutationFn: (found: Found) =>
      api.post<MediaItem>('/discover/import', {
        kind: found.kind,
        tmdbId: found.tmdbId,
        tvdbId: found.tvdbId,
        imdbId: found.imdbId,
        fankaiId: found.fankaiId,
      }),
    onSuccess: (item, found) => {
      setImported((held) => ({ ...held, [keyOf(found)]: item.id }))
      void queryClient.invalidateQueries({ queryKey: ['items'] })
      void queryClient.invalidateQueries({ queryKey: ['stats'] })
    },
  })

  return (
    <div className="mx-auto max-w-4xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {t.admin.discover}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
          {t.admin.intake.lead}
        </p>
      </header>

      <Panel label={t.admin.discover} className="rise mb-6" style={{ animationDelay: '60ms' }}>
        <form
          className="grid gap-4 p-5 sm:grid-cols-[9rem_minmax(0,1fr)_7rem]"
          onSubmit={(event) => {
            event.preventDefault()
            if (fankaiPage) return
            take.reset()
            setAsked({ term: term.trim(), kind, year, source })
          }}
        >
          <div className="sm:col-span-3">
            <label htmlFor="intake-term" className="label mb-1.5 block">
              {t.admin.intake.term}
            </label>
            <div className="relative">
              <Glyph
                name="search"
                className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
              />
              <Input
                id="intake-term"
                required
                value={term}
                onChange={(event) => setTerm(event.target.value)}
                className="pl-9"
              />
            </div>
            <p className="mt-1 text-xs text-bone-faint">{t.admin.intake.termHint}</p>
            {fankaiPage ? (
              <p role="alert" className="mt-2 flex items-start gap-2 text-xs leading-relaxed text-vermillion">
                <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
                {t.admin.intake.fankaiPage}
              </p>
            ) : null}
          </div>

          <div>
            <label htmlFor="intake-kind" className="label mb-1.5 block">
              {t.admin.intake.kind}
            </label>
            <Select
              id="intake-kind"
              value={kind}
              onChange={(event) => {
                const next = event.target.value as '' | MediaKind
                setKind(next)
                // A source that has none of that kind is no longer on offer.
                if (next && !SEARCHABLE.find((s) => s.id === source)?.kinds.includes(next)) setSource('')
              }}
            >
              <option value="">{t.admin.intake.bothKinds}</option>
              <option value="series">{t.nav.series}</option>
              <option value="movie">{t.nav.films}</option>
            </Select>
          </div>

          <div>
            <label htmlFor="intake-source" className="label mb-1.5 block">
              {t.admin.intake.source}
            </label>
            <Select
              id="intake-source"
              value={source}
              aria-describedby="intake-source-hint"
              onChange={(event) => setSource(event.target.value)}
            >
              <option value="">{t.admin.intake.inOrder}</option>
              {offered.map((s) => (
                <option key={s.id} value={s.id} disabled={!on(s.id)}>
                  {on(s.id) ? providerName(s.id) : t.admin.intake.sourceOff(providerName(s.id))}
                </option>
              ))}
            </Select>
            <p id="intake-source-hint" className="mt-1 text-xs text-bone-faint">
              {t.admin.intake.sourceHint}
            </p>
          </div>

          <div>
            <label htmlFor="intake-year" className="label mb-1.5 block">
              {t.admin.intake.year}
            </label>
            <Input
              id="intake-year"
              inputMode="numeric"
              value={year}
              onChange={(event) => setYear(event.target.value.replace(/\D/g, '').slice(0, 4))}
            />
            <p className="mt-1 text-xs text-bone-faint">{t.admin.intake.yearHint}</p>
          </div>

          <div className="flex flex-wrap items-center gap-3 sm:col-span-3">
            <Button type="submit" variant="primary" disabled={!term.trim() || fankaiPage || results.isFetching}>
              {results.isFetching ? (
                <Spinner className="size-4" />
              ) : (
                <Glyph name="discover" className="size-4" />
              )}
              {results.isFetching ? t.admin.intake.searching : t.admin.intake.search}
            </Button>
            <p className="max-w-prose text-xs leading-relaxed text-bone-faint">
              {t.admin.intake.slow}
            </p>
          </div>
        </form>
      </Panel>

      <Panel label={t.admin.intake.found} className="rise" style={{ animationDelay: '100ms' }}>
        <PanelHead
          title={t.admin.intake.found}
          action={
            results.data?.length ? (
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {asked?.source && !isLookup(asked.term)
                  ? t.admin.intake.resultsAt(results.data.length, providerName(asked.source))
                  : t.admin.intake.results(results.data.length)}
              </span>
            ) : null
          }
        />

        {asked === null ? (
          <EmptyState title={t.admin.intake.idle} hint={t.admin.intake.idleHint} />
        ) : results.isPending || results.isFetching ? (
          <p className="flex items-center gap-3 px-5 py-16 text-sm text-bone-dim">
            <Spinner className="size-4" />
            {t.admin.intake.searching}
          </p>
        ) : results.isError ? (
          <p role="alert" className="flex items-center gap-2 px-5 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {t.admin.intake.failed}
          </p>
        ) : results.data.length === 0 ? (
          <EmptyState title={t.admin.intake.empty} hint={t.admin.intake.emptyHint} />
        ) : (
          <ul>
            {results.data.map((found) => {
              const id = keyOf(found)

              return (
                <Result
                  key={id}
                  found={found}
                  stored={found.stored ? undefined : imported[id]}
                  busy={take.isPending && keyOf(take.variables) === id}
                  error={
                    take.isError && keyOf(take.variables) === id
                      ? take.error.message
                      : undefined
                  }
                  onImport={() => take.mutate(found)}
                />
              )
            })}
          </ul>
        )}
      </Panel>
    </div>
  )
}

/**
 * One work a provider has.
 *
 * Shallow on purpose: enough to recognise the right film among six of the same
 * name, and no more — the full fetch happens once, for the one you meant.
 */
function Result({
  found,
  stored,
  busy,
  error,
  onImport,
}: {
  found: Found
  /** The id it was imported under during this visit, if it was. */
  stored: string | undefined
  busy: boolean
  error: string | undefined
  onImport: () => void
}) {
  const { t } = useI18n()

  const ids = [
    found.tmdbId ? `tmdb:${found.tmdbId}` : null,
    found.tvdbId ? `tvdb:${found.tvdbId}` : null,
    found.imdbId,
    found.fankaiId ? `fankai:${found.fankaiId}` : null,
  ].filter(Boolean)

  return (
    <li className="flex gap-4 border-b border-rule p-4 transition-colors duration-150 last:border-0 hover:bg-ink-high">
      <div className="aspect-2/3 w-14 shrink-0 overflow-hidden rounded-card border border-rule bg-ink-high sm:w-20">
        {found.poster ? (
          <Artwork
            url={found.poster}
            role="card"
            alt={t.a11y.poster(found.title)}
            className="size-full object-cover"
          />
        ) : (
          <div className="flex size-full items-center justify-center">
            <Glyph
              name={found.kind === 'series' ? 'tv' : 'film'}
              className="size-5 text-bone-faint"
            />
          </div>
        )}
      </div>

      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <h3 className="text-sm font-medium text-bone">{found.title}</h3>
          {found.year ? (
            <span className="font-mono text-xs text-bone-faint tabular-nums">{found.year}</span>
          ) : null}
          <Glyph
            name={found.kind === 'series' ? 'tv' : 'film'}
            className="size-3.5 text-bone-faint"
            title={found.kind === 'series' ? t.nav.series : t.nav.films}
          />
          {found.isAdult ? <Chip tone="accent">{t.admin.intake.adult}</Chip> : null}
        </div>

        {ids.length ? (
          <p className="mt-1 flex flex-wrap gap-x-3 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
            {ids.map((one) => (
              <span key={one}>{one}</span>
            ))}
          </p>
        ) : null}

        {found.overview ? (
          <p className="mt-1.5 line-clamp-3 max-w-prose text-xs leading-relaxed text-bone-dim">
            {found.overview}
          </p>
        ) : null}

        <div className="mt-3 flex flex-wrap items-center gap-2">
          {found.stored ? (
            <Chip tone="provider">
              <Glyph name="check" className="size-3" />
              {t.admin.intake.stored}
            </Chip>
          ) : stored ? (
            <>
              <Chip tone="manual">
                <Glyph name="check" className="size-3" />
                {t.admin.intake.imported}
              </Chip>
              <Link
                to={`/admin/catalogue/${stored}`}
                className="inline-flex min-h-11 items-center gap-1.5 text-sm text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
              >
                {t.admin.intake.openEntry}
                <Glyph name="chevronRight" className="size-3.5" />
              </Link>
            </>
          ) : fetchable(found) ? (
            <Button size="sm" variant="primary" disabled={busy} onClick={onImport}>
              {busy ? <Spinner className="size-3.5" /> : <Glyph name="download" className="size-3.5" />}
              {busy ? t.admin.intake.importing : t.admin.intake.import}
            </Button>
          ) : (
            <p className="text-xs text-bone-faint">{t.admin.intake.noIdentifier}</p>
          )}
        </div>

        {error ? (
          <p role="alert" className="mt-2 flex items-start gap-1.5 text-xs text-vermillion">
            <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
            {t.admin.intake.importFailed} {error}
          </p>
        ) : null}
      </div>
    </li>
  )
}
