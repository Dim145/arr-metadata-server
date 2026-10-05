/**
 * The work as the rest of the world knows it: the identifiers it goes by on
 * the sites it was assembled from, the marks each gave it, the works a
 * provider files beside it, the other ways its episodes are numbered, and
 * what TMDB recommends next to it.
 *
 * Only the identifiers are edited here. The rest is what the sources said,
 * shown so that nobody has to open the public page to know it is there.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import { ExternalLink } from '../../../components/elsewhere'
import { Artwork } from '../../../components/media'
import { Button, Field, FormField, Glyph, Input, Panel, PanelHead, Skeleton, Spinner } from '../../../components/ui'
import { ApiError, api } from '../../../lib/api'
import * as fmt from '../../../lib/format'
import { useOrders } from '../../../lib/hooks'
import { useI18n } from '../../../lib/i18n'
import { providerName } from '../../../lib/labels'
import { identifierLink, relationLink } from '../../../lib/links'
import { ratingsOf, seasonNumbers } from '../../../lib/media'
import type { ExternalIds, MediaItem, Override, Suggestion, Suggestions as SuggestionsResponse } from '../../../lib/types'

import { Count, Empty } from './shared'

export function ElsewhereTab({ work, overrides, onChanged }: { work: MediaItem; overrides: Override[]; onChanged: () => void }) {
  const { t } = useI18n()
  const lock = overrides.find((o) => o.scope === 'item' && o.field === 'externalIds')

  return (
    <div className="grid grid-cols-1 gap-6 lg:grid-cols-3 lg:items-start">
      <Panel id="identifiers" label={t.work.identifiers} className="scroll-mt-28 lg:scroll-mt-16">
        <PanelHead title={t.work.identifiers} />
        <IdentifiersEditor work={work} lock={lock} onChanged={onChanged} />
      </Panel>
      <Ratings work={work} />
      {work.isManual ? null : <Suggestions work={work} />}
      <Related work={work} />
      {work.kind === 'series' ? <Orders work={work} /> : null}
    </div>
  )
}

/* ── The identifiers, edited ─────────────────────────────────────────────── */

/** The sources a work can be given an identifier for, in the order shown. */
const ID_SOURCES: { key: keyof ExternalIds; list?: boolean }[] = [
  { key: 'tmdb' },
  { key: 'tvdb' },
  { key: 'imdb' },
  { key: 'tvmaze' },
  { key: 'tvrage' },
  { key: 'trakt' },
  { key: 'fankai' },
  { key: 'mal', list: true },
  { key: 'anilist', list: true },
]

/** What the boxes say for a set of identifiers. */
function idDrafts(ids: ExternalIds): Record<string, string> {
  const drafts: Record<string, string> = {}
  for (const { key } of ID_SOURCES) {
    const value = ids[key]
    drafts[key] = value === undefined ? '' : Array.isArray(value) ? value.join(', ') : String(value)
  }
  return drafts
}

/** The identifiers the boxes hold, in the shape the server locks. */
function idsOf(drafts: Record<string, string>): { ids: ExternalIds; error?: string } {
  const ids: ExternalIds = {}
  for (const { key, list } of ID_SOURCES) {
    const text = (drafts[key] ?? '').trim()
    if (!text) continue
    if (key === 'imdb') {
      ids.imdb = text
      continue
    }
    // Whole numbers and nothing after them: `parseInt` read "81189x" as 81189
    // and "1e5" as 1, and locked what nobody had typed.
    const parts = text.split(',').map((part) => part.trim())
    if (parts.some((part) => !/^\d+$/.test(part))) return { ids, error: key }
    const numbers = parts.map(Number)
    if (numbers.some((n) => !Number.isSafeInteger(n))) return { ids, error: key }
    if (list) {
      ;(ids as Record<string, unknown>)[key] = numbers
    } else {
      if (numbers.length !== 1) return { ids, error: key }
      ;(ids as Record<string, unknown>)[key] = numbers[0]
    }
  }
  return { ids }
}

function IdentifiersEditor({ work, lock, onChanged }: { work: MediaItem; lock?: Override; onChanged: () => void }) {
  const { t, locale } = useI18n()
  const e = t.admin.editor
  const [editing, setEditing] = useState(false)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [invalid, setInvalid] = useState<string | null>(null)

  const save = useMutation({
    mutationFn: (ids: ExternalIds) => api.put(`/items/${work.id}/overrides`, { scope: 'item', field: 'externalIds', value: ids }),
    onSuccess: () => {
      setEditing(false)
      onChanged()
    },
  })
  const unlock = useMutation({
    mutationFn: () => api.delete(`/items/${work.id}/overrides/item/externalIds`),
    onSuccess: onChanged,
  })

  const held = Object.entries(work.externalIds).filter(([, value]) => value !== undefined && value !== null && !(Array.isArray(value) && !value.length))

  return (
    <div className={lock ? 'border-l-2 border-brass bg-brass/[0.05]' : undefined}>
      {lock ? (
        <p className="flex items-center gap-2 px-5 pt-4 text-xs text-brass">
          <Glyph name="lock" className="size-3.5" />
          {e.identifiersLocked(fmt.relative(lock.updatedAt, locale) ?? '')}
        </p>
      ) : null}
      {editing ? (
        <form
          className="flex flex-col gap-3 p-5"
          onSubmit={(event) => {
            event.preventDefault()
            const { ids, error } = idsOf(drafts)
            if (error) {
              setInvalid(error)
              return
            }
            setInvalid(null)
            save.mutate(ids)
          }}
          onKeyDown={(event) => {
            if (event.key === 'Escape') setEditing(false)
          }}
        >
          {ID_SOURCES.map(({ key, list }) => (
            <FormField key={key} label={providerName(key)} htmlFor={`id-${key}`} hint={list ? e.identifiersListHint : undefined} error={invalid === key ? e.identifiersInvalid : undefined}>
              <Input
                id={`id-${key}`}
                value={drafts[key] ?? ''}
                inputMode={key === 'imdb' ? 'text' : 'numeric'}
                onChange={(event) => setDrafts((held) => ({ ...held, [key]: event.target.value }))}
              />
            </FormField>
          ))}
          {save.isError ? (
            <p role="alert" className="text-sm text-vermillion">
              {save.error instanceof ApiError ? save.error.message : t.common.actionFailed}
            </p>
          ) : null}
          <div className="flex flex-wrap gap-2">
            <Button type="submit" variant="primary" size="sm" disabled={save.isPending}>
              {save.isPending ? <Spinner className="size-4" /> : <Glyph name="lock" className="size-4" />}
              {e.saveAndLock}
            </Button>
            <Button type="button" size="sm" onClick={() => setEditing(false)}>
              {t.common.cancel}
            </Button>
          </div>
        </form>
      ) : (
        <>
          <dl className="divide-y divide-rule">
            {held.length ? (
              held.map(([source, value]) => {
                const values = Array.isArray(value) ? value : [value as string | number]
                return (
                  <Field key={source} label={providerName(source)}>
                    <span className="inline-flex flex-wrap items-baseline justify-end gap-x-2 gap-y-1 font-mono text-[0.8125rem] tabular-nums">
                      {values.map((one) => {
                        const href = identifierLink(source, one, work.kind)
                        return href ? (
                          <ExternalLink key={one} href={href}>
                            {one}
                          </ExternalLink>
                        ) : (
                          <span key={one}>{one}</span>
                        )
                      })}
                    </span>
                  </Field>
                )
              })
            ) : (
              <p className="px-5 py-3 text-sm text-bone-faint">{e.identifiersNone}</p>
            )}
            <Field label="slug">
              <span className="font-mono text-[0.8125rem] break-all text-bone-dim">{work.slug}</span>
            </Field>
          </dl>
          <div className="p-5 pt-3">
            <p className="text-xs leading-relaxed text-bone-faint">{e.identifiersHint}</p>
            <div className="mt-3 flex flex-wrap gap-2">
              <Button
                size="sm"
                onClick={() => {
                  setDrafts(idDrafts(work.externalIds))
                  setInvalid(null)
                  setEditing(true)
                }}
              >
                <Glyph name="pencil" className="size-4" />
                {t.common.edit}
              </Button>
              {lock ? (
                <Button size="sm" variant="quiet" disabled={unlock.isPending} onClick={() => unlock.mutate()}>
                  <Glyph name="unlock" className="size-4" />
                  {e.unlock}
                </Button>
              ) : null}
            </div>
            {unlock.isError ? (
              <p role="alert" className="mt-2 text-sm text-vermillion">
                {unlock.error instanceof ApiError ? unlock.error.message : t.common.actionFailed}
              </p>
            ) : null}
          </div>
        </>
      )}
    </div>
  )
}

/* ── What the sources make of it ─────────────────────────────────────────── */

/**
 * Every mark out of ten a source gave, the one the clients are handed first.
 * One figure hides how far apart the audiences are, and this is the page
 * where the whole of what the sources said is laid out.
 */
function Ratings({ work }: { work: MediaItem }) {
  const { t, locale } = useI18n()
  const x = t.admin.editor.elsewhereTab
  const ratings = ratingsOf(work.ratings)

  return (
    <Panel id="ratings" label={t.work.ratings} className="scroll-mt-28 lg:scroll-mt-16">
      <PanelHead title={t.work.ratings} action={<Count>{ratings.length}</Count>} />
      {ratings.length ? (
        <ul className="divide-y divide-rule">
          {ratings.map((rating, index) => {
            const value = rating.value ?? 0
            return (
              <li key={rating.source} className="px-5 py-3">
                <div className="flex items-baseline justify-between gap-3">
                  <span className="label flex items-center gap-2">
                    {providerName(rating.source)}
                    {index === 0 ? <span className="font-mono text-[0.625rem] tracking-[0.08em] text-brass normal-case">· {x.headline}</span> : null}
                  </span>
                  <span className="font-display text-lg leading-none text-bone tabular-nums">
                    {fmt.score(value, locale)}
                    <span className="sr-only"> {t.work.outOfTen}</span>
                  </span>
                </div>
                <div aria-hidden className="mt-2 h-1 overflow-hidden rounded-full bg-rule">
                  <div className="h-full rounded-full bg-gradient-to-r from-vermillion to-brass" style={{ width: `${value * 10}%` }} />
                </div>
                {rating.votes ? (
                  <p className="mt-1.5 font-mono text-[0.6875rem] text-bone-faint tabular-nums">{t.work.votes(fmt.count(rating.votes, locale), rating.votes)}</p>
                ) : null}
              </li>
            )
          })}
        </ul>
      ) : (
        <Empty>{x.noRatings}</Empty>
      )}
      <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{x.ratingsHint}</p>
    </Panel>
  )
}

/** The works a provider files beside this one, nearest first. */
function Related({ work }: { work: MediaItem }) {
  const { t } = useI18n()
  const x = t.admin.editor.elsewhereTab
  const relations = work.relations ?? []

  return (
    <Panel id="related" label={t.work.related} className="scroll-mt-28 lg:scroll-mt-16">
      <PanelHead title={t.work.related} action={<Count>{relations.length}</Count>} />
      {relations.length ? (
        <ul className="divide-y divide-rule">
          {relations.map((relation) => {
            const kind = t.work.relation[relation.relationType] ?? t.work.relation.OTHER
            const format = relation.format ? (t.work.format[relation.format] ?? relation.format) : undefined
            const caption = [kind, relation.year, format, relation.workId ? undefined : t.work.onSite(providerName(relation.source))].filter(Boolean).join(' · ')
            const face = (
              <span className="aspect-2/3 w-9 shrink-0 overflow-hidden rounded-[4px] border border-rule bg-ink-high">
                {relation.image ? <Artwork url={relation.image} role="thumb" alt="" className="size-full object-cover" /> : null}
              </span>
            )
            return (
              <li key={relation.id} className="flex items-center gap-3 px-5 py-2">
                {face}
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-bone">{relation.title}</span>
                  <span className="block truncate text-xs text-bone-faint">{caption}</span>
                </span>
                {relation.workId ? (
                  <Link to={`/admin/catalogue/${relation.workId}`} className="hit inline-flex min-h-8 shrink-0 items-center gap-1 rounded-full px-2 text-xs text-vermillion hover:bg-vermillion/10">
                    {t.admin.editor.suggestionsHeld}
                    <Glyph name="chevronRight" className="size-3" />
                  </Link>
                ) : (
                  <ExternalLink href={relationLink(relation)} className="shrink-0 text-xs text-bone-dim">
                    {providerName(relation.source)}
                  </ExternalLink>
                )}
              </li>
            )
          })}
        </ul>
      ) : (
        <Empty>{x.noRelated}</Empty>
      )}
    </Panel>
  )
}

/** The other orders TheTVDB numbers the episodes in, each a way to the season sheet in it. */
function Orders({ work }: { work: MediaItem }) {
  const { t } = useI18n()
  const x = t.admin.editor.elsewhereTab
  const orders = useOrders(work.id)
  const list = orders.data?.orders ?? []
  const first = seasonNumbers(work).find((n) => n > 0) ?? seasonNumbers(work)[0]

  return (
    <Panel id="orders" label={x.orders} className="scroll-mt-28 lg:scroll-mt-16">
      <PanelHead title={x.orders} action={<Count>{list.length}</Count>} />
      {orders.isPending ? (
        <div className="p-5">
          <Skeleton className="h-10 w-full" />
        </div>
      ) : list.length ? (
        <ul className="divide-y divide-rule">
          {list.map((order) => {
            const seasons = new Set(order.episodes.map((episode) => episode.seasonNumber))
            const target = seasons.has(first ?? -1) ? first : [...seasons].sort((a, b) => a - b)[0]
            return (
              <li key={order.kind} className="flex items-center gap-3 px-5 py-2.5">
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-medium text-bone">{t.season.order[order.kind] ?? order.kind}</span>
                  <span className="block font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                    {t.admin.editor.contentValue(seasons.size, order.episodes.length)}
                  </span>
                </span>
                {target !== undefined ? (
                  <Link
                    to={`/work/${work.id}/season/${target}?order=${encodeURIComponent(order.kind)}`}
                    className="hit inline-flex min-h-8 shrink-0 items-center gap-1 rounded-full px-2 text-xs text-vermillion hover:bg-vermillion/10"
                  >
                    {x.seeOrder}
                    <Glyph name="chevronRight" className="size-3" />
                  </Link>
                ) : null}
              </li>
            )
          })}
        </ul>
      ) : (
        <Empty>{x.noOrders}</Empty>
      )}
      <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{x.ordersHint}</p>
    </Panel>
  )
}

/* ── What TMDB suggests ───────────────────────────────────────────────────── */

/**
 * What TMDB recommends beside the work: a click imports one, and one the
 * catalogue already holds opens instead. Absent where no TMDB key lets it
 * be asked.
 */
function Suggestions({ work }: { work: MediaItem }) {
  const { t } = useI18n()
  const queryClient = useQueryClient()
  const [imported, setImported] = useState<Record<number, string>>({})

  const suggestions = useQuery({
    queryKey: ['suggestions', work.id],
    queryFn: () => api.get<SuggestionsResponse>(`/items/${work.id}/suggestions`),
    retry: false,
    staleTime: 60 * 60_000,
  })

  const take = useMutation({
    mutationFn: (suggestion: Suggestion) => api.post<MediaItem>('/discover/import', { kind: suggestion.kind, tmdbId: suggestion.tmdbId }),
    onSuccess: (item, suggestion) => {
      setImported((held) => ({ ...held, [suggestion.tmdbId]: item.id }))
      void queryClient.invalidateQueries({ queryKey: ['items'] })
      void queryClient.invalidateQueries({ queryKey: ['stats'] })
    },
  })

  if (suggestions.isError && suggestions.error instanceof ApiError && suggestions.error.status === 503) {
    return null
  }
  const list = suggestions.data?.suggestions ?? []

  return (
    <Panel id="suggestions" label={t.admin.editor.suggestions} className="scroll-mt-28 lg:scroll-mt-16">
      <PanelHead title={t.admin.editor.suggestions} />
      <div className="p-5">
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{t.admin.editor.suggestionsHint}</p>
        {suggestions.isPending ? (
          <Skeleton className="mt-4 h-24 w-full" />
        ) : suggestions.isError ? (
          <p role="alert" className="mt-4 text-sm text-vermillion">
            {t.admin.editor.suggestionsFailed}
          </p>
        ) : list.length === 0 ? (
          <p className="mt-4 text-sm text-bone-faint">{t.admin.editor.suggestionsNone}</p>
        ) : (
          <ul className="mt-4 divide-y divide-rule">
            {list.map((suggestion) => {
              const held = suggestion.held ?? imported[suggestion.tmdbId]
              const busy = take.isPending && take.variables?.tmdbId === suggestion.tmdbId
              return (
                <li key={suggestion.tmdbId} className="flex items-center gap-3 py-2">
                  {suggestion.poster ? (
                    <img src={suggestion.poster} alt="" width={32} height={48} loading="lazy" className="h-12 w-8 shrink-0 rounded-sm object-cover" />
                  ) : (
                    <span className="h-12 w-8 shrink-0 rounded-sm bg-ink-high" />
                  )}
                  <span className="min-w-0 flex-1">
                    <span className="block text-sm break-words text-bone">
                      {suggestion.title}
                      {suggestion.year ? <span className="text-bone-faint"> · {suggestion.year}</span> : null}
                    </span>
                    {suggestion.score ? <span className="font-mono text-xs text-bone-faint tabular-nums">{suggestion.score.toFixed(1)}</span> : null}
                  </span>
                  {held ? (
                    <Link to={`/admin/catalogue/${held}`} className="text-sm text-vermillion underline-offset-4 hover:underline">
                      {t.admin.editor.suggestionsHeld}
                    </Link>
                  ) : (
                    <Button size="sm" disabled={busy} onClick={() => take.mutate(suggestion)}>
                      {busy ? t.admin.editor.suggestionsImporting : t.admin.editor.suggestionsImport}
                    </Button>
                  )}
                </li>
              )
            })}
          </ul>
        )}
        {take.isError ? (
          <p role="alert" className="mt-3 text-sm text-vermillion">
            {take.error.message}
          </p>
        ) : null}
      </div>
    </Panel>
  )
}
