/**
 * The record beside a work's page: its facts, its dates and ratings by
 * country, its marks out of ten, its identifiers, and where it is watched.
 */

import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'

import { ExternalLink } from '../../components/elsewhere'
import { Field, Genre, Glyph, Panel, PanelHead, Select } from '../../components/ui'
import { ApiError, api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import { genrePath } from '../../lib/facets'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { languageName, listedGenres, providerName, statusLabel } from '../../lib/labels'
import { identifierLink } from '../../lib/links'
import { ratingsOf } from '../../lib/media'
import {
  REGION_KEY,
  certificationIn,
  cinemaDayIn,
  countryCode,
  countryName,
  otherCountries,
  releasesIn,
} from '../../lib/releases'
import type { CountryRelease, MediaItem, WhereToWatch as WhereToWatchData } from '../../lib/types'
import { Fold } from './fold'

/**
 * Where a section starts when the page is led there by its own index: below
 * the bar at the top, and below the row of sections a phone keeps under it.
 */
export const SECTION_MARGIN = 'scroll-mt-32 lg:scroll-mt-20'

export function Record({ item, region }: { item: MediaItem; region: string }) {
  const { t, locale } = useI18n()
  // A film's day in the reader's country's cinemas, where it is known.
  const local = item.kind === 'movie' ? cinemaDayIn(item, region) : undefined
  const rated = ratedIn(item, region)
  const companies = (item.productionCompanies ?? []).filter((c) => c.trim())
  // A list that only says the studio again says nothing.
  const onlyStudio =
    companies.length === 1 && companies[0]!.toLocaleLowerCase(locale) === item.studio?.toLocaleLowerCase(locale)
  const countries = (item.productionCountries ?? []).map((code) => countryName(code, locale))

  const rows: [string, string | undefined, string?][] = [
    [t.work.status, statusLabel(item.status, t)],
    [
      item.kind === 'series' ? t.work.firstAired : t.work.released,
      fmt.longDate(item.kind === 'series' ? item.firstAired : (local ?? item.inCinemas), locale),
      local ? countryName(region, locale) : undefined,
    ],
    [t.work.lastAired, item.kind === 'series' ? fmt.longDate(item.lastAired, locale) : undefined],
    [t.work.runtime, fmt.runtime(item.runtime, locale)],
    [t.work.network, item.network],
    [t.work.studio, item.studio],
    [t.work.companies, onlyStudio || !companies.length ? undefined : fmt.list(companies, locale)],
    [t.work.countries, countries.length ? fmt.list(countries, locale) : undefined],
    [t.work.certification, rated?.certification, rated?.country ? countryName(rated.country, locale) : undefined],
    [t.work.originalLanguage, languageName(item.originalLanguage, locale)],
  ]

  const present = rows.filter((row): row is [string, string, string?] => Boolean(row[1]))

  return (
    <Panel id="details" className={SECTION_MARGIN}>
      <PanelHead title={t.work.details} />
      <dl className="divide-y divide-rule">
        {present.map(([label, value, hint]) => (
          <Field key={label} label={label} hint={hint}>
            {value}
          </Field>
        ))}
      </dl>

      {item.genres.length ? (
        <div className="flex flex-wrap gap-1.5 border-t border-rule p-4">
          {listedGenres(item.genres).map((genre) => (
            <Genre key={genre} name={genre} to={genrePath(genre)} />
          ))}
        </div>
      ) : null}
    </Panel>
  )
}

/**
 * The rating a reader is shown, and the country it is of: their own
 * country's, else the record's own — another country's, said so.
 */
export function ratedIn(item: MediaItem, region: string): { certification: string; country?: string } | undefined {
  const own = certificationIn(item, region)
  if (own) return { certification: own, country: region }
  const kept = item.contentRating?.trim()
  return kept ? { certification: kept, country: countryCode(item.contentRatingCountry) } : undefined
}

/**
 * When the work came out in each country, and the rating it carries there:
 * the reader's country, then every other a press away — a film shown in
 * forty countries is forty lines nobody came for.
 */
export function Releases({ item, region }: { item: MediaItem; region: string }) {
  const { t, locale } = useI18n()
  const [all, setAll] = useState(false)
  if (!item.releases?.length) return null

  const name = countryName(region, locale)
  const own = releasesIn(item, region).filter((r) => r.date)
  const rating = certificationIn(item, region)
  const others = otherCountries(item, region, locale)
  const kind = (release: CountryRelease) =>
    release.releaseType ? (t.work.releaseKind[release.releaseType] ?? release.releaseType) : t.work.released

  const title = item.kind === 'movie' ? t.work.releases : t.work.certifications

  return (
    <Panel label={title}>
      <PanelHead title={title} />
      <p className="label px-5 pt-3.5">{name}</p>
      {rating || own.length ? (
        <dl className="divide-y divide-rule">
          {rating ? <Field label={t.work.certification}>{rating}</Field> : null}
          {own.map((release, index) => (
            <Field key={`${release.releaseType}-${release.date}-${index}`} label={kind(release)} hint={release.note}>
              {fmt.longDate(release.date, locale)}
            </Field>
          ))}
        </dl>
      ) : (
        <p className="px-5 py-2.5 text-sm text-bone-faint">
          {item.kind === 'movie' ? t.work.noneIn(name) : t.work.unratedIn(name)}
        </p>
      )}
      {others.length ? (
        <div className="border-t border-rule px-5 py-2">
          <button
            type="button"
            aria-expanded={all}
            aria-controls={`releases-${item.id}`}
            onClick={() => setAll(!all)}
            className="inline-flex min-h-11 cursor-pointer items-center gap-1.5 text-sm text-bone-dim transition-colors duration-150 hover:text-bone"
          >
            <Glyph name="chevronRight" className={cn('size-3.5 transition-transform duration-150 motion-reduce:transition-none', all && 'rotate-90')} />
            {all ? t.work.fewerCountries : t.work.otherCountries(others.length)}
          </button>
          {all ? (
            <ul id={`releases-${item.id}`} className="divide-y divide-rule pb-2">
              {others.map((code) => {
                const rated = certificationIn(item, code)
                const days = releasesIn(item, code).filter((r) => r.date)
                return (
                  <li key={code} className="py-2.5">
                    <div className="flex items-baseline justify-between gap-3">
                      <span className="text-sm text-bone">{countryName(code, locale)}</span>
                      {rated ? <span className="font-mono text-xs text-bone-dim">{rated}</span> : null}
                    </div>
                    {days.length ? (
                      <p className="mt-0.5 text-xs leading-relaxed text-bone-faint">
                        {days.map((release) => `${kind(release)} ${fmt.shortDate(release.date, locale)}`).join(' · ')}
                      </p>
                    ) : null}
                  </li>
                )
              })}
            </ul>
          ) : null}
        </div>
      ) : null}
    </Panel>
  )
}

/**
 * Every mark out of ten a source gave, the one on the plate first.
 *
 * One figure hides how far apart the audiences are: an anime can sit a point
 * higher on MyAnimeList than on TMDB, from a hundred times the votes. Left out
 * when the plate already shows the only one there is.
 */
export function Ratings({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const ratings = ratingsOf(item.ratings)

  if (ratings.length < 2) return null

  return (
    <Panel>
      <PanelHead title={t.work.ratings} />
      <ul className="divide-y divide-rule">
        {ratings.map((rating) => {
          const value = rating.value ?? 0

          return (
            <li key={rating.source} className="px-5 py-3">
              <div className="flex items-baseline justify-between gap-3">
                <span className="label">{providerName(rating.source)}</span>
                <span className="font-display text-lg leading-none text-bone tabular-nums">
                  {fmt.score(value, locale)}
                  <span className="sr-only"> {t.work.outOfTen}</span>
                </span>
              </div>
              <div aria-hidden className="mt-2 h-1 overflow-hidden rounded-full bg-rule">
                <div
                  className="h-full rounded-full bg-gradient-to-r from-vermillion to-brass"
                  style={{ width: `${value * 10}%` }}
                />
              </div>
              {rating.votes ? (
                <p className="mt-1.5 font-mono text-caption text-bone-faint tabular-nums">
                  {t.work.votes(fmt.count(rating.votes, locale), rating.votes)}
                </p>
              ) : null}
            </li>
          )
        })}
      </ul>
    </Panel>
  )
}

/** How many identifiers stand open before the rest are folded. */
const SHOWN_IDENTIFIERS = 3

/**
 * Every identifier, each a way to the work on the site that issued it.
 *
 * MyAnimeList and AniList give one id per season or cour, so a long series
 * carries a dozen: the first — the entry that stands for the whole — is shown,
 * and the rest wait behind a count rather than filling the panel. The sites
 * past the first three are folded the same way, and found by the browser's
 * search all the same (see `Fold`).
 */
export function Identifiers({ item }: { item: MediaItem }) {
  const { t } = useI18n()

  const entries = Object.entries(item.externalIds).filter(([, value]) =>
    Array.isArray(value) ? value.length : value !== undefined && value !== null,
  )

  if (!entries.length) return null

  const row = ([source, value]: (typeof entries)[number]) => (
    <Field key={source} label={providerName(source)}>
      <IdentifierValues source={source} values={Array.isArray(value) ? value : [value]} kind={item.kind} />
    </Field>
  )
  const rest = entries.slice(SHOWN_IDENTIFIERS)

  return (
    <Panel>
      <PanelHead title={t.work.identifiers} />
      <dl className="divide-y divide-rule">{entries.slice(0, SHOWN_IDENTIFIERS).map(row)}</dl>
      {rest.length ? (
        <Fold label={t.work.allIdentifiers(entries.length)} className="border-t border-rule px-5 py-2.5">
          <dl className="divide-y divide-rule border-t border-rule">{rest.map(row)}</dl>
        </Fold>
      ) : null}
    </Panel>
  )
}

function IdentifierValues({
  source,
  values,
  kind,
}: {
  source: string
  values: (string | number)[]
  kind: MediaItem['kind']
}) {
  const { t } = useI18n()
  const [all, setAll] = useState(false)
  const shown = all ? values : values.slice(0, 1)
  const hidden = values.length - shown.length

  return (
    <span className="inline-flex flex-wrap items-baseline justify-end gap-x-2 gap-y-1 font-mono text-body-sm tabular-nums">
      {shown.map((value) => {
        const href = identifierLink(source, value, kind)
        return href ? (
          <ExternalLink key={value} href={href}>
            {value}
          </ExternalLink>
        ) : (
          <span key={value}>{value}</span>
        )
      })}
      {values.length > 1 ? (
        <button
          type="button"
          aria-expanded={all}
          onClick={() => setAll(!all)}
          className="min-h-6 cursor-pointer text-xs text-bone-faint underline decoration-dotted underline-offset-2 hover:text-bone"
        >
          {all ? t.work.fewerIds : t.work.moreIds(hidden)}
        </button>
      ) : null}
    </span>
  )
}

/* ── Where to watch ───────────────────────────────────────────────────────── */

/**
 * How many services a group shows before the rest wait behind a count: a
 * film for rent is on twenty, and the panel was a column of logos.
 */
const FOLDED_PROVIDERS = 4

/**
 * The services carrying a work in a country, as the panel asks for them —
 * read by the page too, which offers the way there only where there is one.
 * The same query, so it is asked once.
 */
export function useWatch(id: string | undefined, region: string | undefined) {
  return useQuery({
    queryKey: ['watch', id, region ?? ''],
    queryFn: () => api.get<WhereToWatchData>(`/items/${id}/watch${query({ region })}`),
    enabled: Boolean(id),
    retry: false,
    staleTime: 60 * 60_000,
  })
}

/** Whether a country's answer lists any service at all. */
export function watchable(data: WhereToWatchData | undefined): boolean {
  return Boolean(data && WATCH_GROUPS.some((group) => data[group].length))
}

const WATCH_GROUPS = ['flatrate', 'free', 'ads', 'rent', 'buy'] as const

/**
 * The services carrying the work in a country, as TMDB lists them from
 * JustWatch — named beside the data, as TMDB's terms ask. The country is
 * the reader's own setting, then the language's; any other TMDB knows of
 * can be picked. Absent altogether where there is no TMDB key, or nothing.
 */
export function WhereToWatch({
  item,
  picked,
  onPick,
}: {
  item: MediaItem
  /** The country the reader picked here, if they did. */
  picked: string | undefined
  onPick: (code: string | undefined) => void
}) {
  const { t, locale } = useI18n()
  const region = picked
  // The groups opened out past their first four, by name.
  const [opened, setOpened] = useState<ReadonlySet<string>>(() => new Set())
  const toggle = (group: string) =>
    setOpened((was) => {
      const next = new Set(was)
      if (!next.delete(group)) next.add(group)
      return next
    })
  // The country picked last time, kept in this browser alone: a reader abroad
  // picks it once, not on every page.
  const pick = (code: string) => {
    onPick(code)
    try {
      localStorage.setItem(REGION_KEY, code)
    } catch {
      // A browser that keeps nothing still shows the country picked.
    }
  }

  const watch = useWatch(item.id, region)
  // A remembered country the server no longer takes is forgotten, and the
  // panel asked for again without it, rather than staying away for good.
  useEffect(() => {
    if (region && watch.error instanceof ApiError && watch.error.status === 400) {
      try {
        localStorage.removeItem(REGION_KEY)
      } catch {
        // Nothing kept, nothing to forget.
      }
      onPick(undefined)
    }
  }, [region, watch.error, onPick])
  if (!watch.data) return null
  const data = watch.data
  const groups = WATCH_GROUPS.filter((group) => data[group].length)
  if (!groups.length && data.regions.length === 0) return null

  // A country's name where the engine knows them; its code where it does not.
  const names = (() => {
    try {
      return new Intl.DisplayNames(locale, { type: 'region' })
    } catch {
      return undefined
    }
  })()
  const place = (code: string) => {
    try {
      return names?.of(code) ?? code
    } catch {
      return code
    }
  }

  return (
    // Where the bar along the foot of a phone leads with "Watch".
    <Panel id="watch" className={SECTION_MARGIN}>
      <PanelHead
        title={t.work.watch}
        action={
          data.regions.length > 1 ? (
            <Select
              aria-label={t.work.watchRegion}
              value={data.region}
              onChange={(event) => pick(event.target.value)}
              className="min-h-9 w-auto py-0 text-xs"
            >
              {data.regions.map((code) => (
                <option key={code} value={code}>
                  {place(code)}
                </option>
              ))}
            </Select>
          ) : null
        }
      />
      <div className="space-y-4 p-5">
        {groups.length ? (
          groups.map((group) => {
            // The first few, and the rest a press away; a group of four or
            // fewer is whole as it is.
            const providers = data[group]
            const open = opened.has(group)
            const shown = open || providers.length <= FOLDED_PROVIDERS ? providers : providers.slice(0, FOLDED_PROVIDERS)
            return (
              <div key={group}>
                <span className="label">{t.work.watchGroups[group]}</span>
                <ul className="mt-2 flex flex-wrap gap-2">
                  {shown.map((provider) => (
                    <li
                      key={provider.id}
                      className="flex items-center gap-2 rounded-full border border-rule py-1 pr-3 pl-1 text-sm text-bone"
                    >
                      {provider.logo ? (
                        <img
                          src={provider.logo}
                          alt=""
                          width={24}
                          height={24}
                          loading="lazy"
                          className="size-6 rounded-full"
                        />
                      ) : (
                        <span className="ml-2" />
                      )}
                      {provider.name}
                    </li>
                  ))}
                  {providers.length > FOLDED_PROVIDERS ? (
                    <li className="flex">
                      <button
                        type="button"
                        aria-expanded={open}
                        onClick={() => toggle(group)}
                        className="hit inline-flex min-h-8 cursor-pointer items-center rounded-full border border-dashed border-rule-bright px-3 text-xs text-bone-dim transition-colors duration-150 hover:border-bone-faint hover:text-bone"
                      >
                        {open ? t.work.fewerProviders : t.work.moreProviders(providers.length - shown.length)}
                      </button>
                    </li>
                  ) : null}
                </ul>
              </div>
            )
          })
        ) : (
          <p className="text-sm text-bone-dim">{t.work.watchNone(place(data.region))}</p>
        )}
        <p className="text-xs text-bone-faint">
          {place(data.region)} · {t.work.watchCredit}
          {data.link ? (
            <>
              {' · '}
              <ExternalLink href={data.link} className="text-bone-dim">
                {t.work.watchLink}
              </ExternalLink>
            </>
          ) : null}
        </p>
      </div>
    </Panel>
  )
}
