/**
 * What changed in a work, value by value: its journal.
 *
 * Every change a refresh, a sync or a person made, newest first — the
 * changes one write made together in the order the record lists them —
 * with who made it, when, what the value held and what it took, and why. A
 * source's change made under a lock says so: it reached the record, and not
 * what is served. A value a lock can hold can be gone back to: the field is
 * locked to it through the locks' own route, and the journal calls that a
 * revert. A source's copy is never written back.
 *
 * Filtered by field, by who — a person, a key or a source — and by a period,
 * and handed over whole with the same filters, as a CSV for a spreadsheet or
 * as JSON.
 */

import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import {
  Button,
  ButtonLink,
  Chip,
  EmptyState,
  Glyph,
  Input,
  Panel,
  PanelHead,
  Select,
  Skeleton,
  Spinner,
} from '../../../components/ui'
import { ApiError, api, query, segment } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import * as fmt from '../../../lib/format'
import { useI18n, type Dict } from '../../../lib/i18n'
import { describeIdentity, providerName } from '../../../lib/labels'
import type { ChangeCause, ChangeEntry, ChangeList, FieldRegistry, MediaItem } from '../../../lib/types'

import { fieldLabel } from './FieldRow'
import { MOVED, episodeCode, kindOf, period, placeOf, revertible, shown } from './journal'

/** How many changes a page brings. */
const PAGE = 50

/** The causes a source gives a change, as against a person. */
const BY_A_SOURCE: ChangeCause[] = ['refresh', 'sync', 'fallback']

export function JournalTab({
  work,
  registry,
  onChanged,
}: {
  work: MediaItem
  registry: FieldRegistry
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const j = t.admin.editor.journal
  const queryClient = useQueryClient()

  const [field, setField] = useState('')
  const [who, setWho] = useState('')
  const [from, setFrom] = useState('')
  const [to, setTo] = useState('')
  const bounds = period(from, to)
  const filters = { field, actor: who, since: bounds.since, before: bounds.before }
  const filtered = Boolean(field || who || from || to)

  const journal = useInfiniteQuery({
    queryKey: ['item', work.id, 'changes', field, who, bounds.since, bounds.before],
    queryFn: ({ pageParam }) =>
      api.get<ChangeList>(`/items/${segment(work.id)}/changes${query({ ...filters, cursor: pageParam, limit: PAGE })}`),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next,
  })

  const revert = useMutation({
    mutationFn: (change: ChangeEntry) =>
      api.put(`/items/${segment(work.id)}/overrides`, {
        scope: change.scope,
        field: change.field,
        language: change.language,
        value: change.before ?? null,
        cause: 'revert',
      }),
    onSuccess: () => {
      onChanged()
      void queryClient.invalidateQueries({ queryKey: ['item', work.id, 'changes'] })
      void queryClient.invalidateQueries({ queryKey: ['item', work.id, 'provenance'] })
    },
  })

  const pages = journal.data?.pages ?? []
  const changes = pages.flatMap((page) => page.changes)
  // What the filters can offer: the newest page knows the whole journal's.
  const fields = pages[0]?.fields ?? []
  const everybody = pages[0]?.who ?? []
  const download = (format: 'csv' | 'json') =>
    `/api/v1/items/${encodeURIComponent(work.id)}/changes${query({ ...filters, format })}`

  return (
    <Panel id="journal" label={j.title}>
      <PanelHead
        title={j.title}
        action={
          <span className="flex flex-wrap items-center gap-1">
            {journal.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
            <ButtonLink size="sm" variant="quiet" href={download('csv')} download title={j.downloadHint}>
              <Glyph name="download" className="size-3.5" />
              {j.csv}
            </ButtonLink>
            <ButtonLink size="sm" variant="quiet" href={download('json')} download title={j.downloadHint}>
              <Glyph name="download" className="size-3.5" />
              {j.json}
            </ButtonLink>
          </span>
        }
      />
      <p className="max-w-prose px-5 pt-1 text-sm leading-relaxed text-bone-dim">{j.lead}</p>

      <div className="flex flex-wrap items-end gap-3 border-b border-rule px-5 py-4">
        <div className="min-w-40">
          <label htmlFor="journal-field" className="label mb-1.5 block">
            {j.field}
          </label>
          <Select id="journal-field" value={field} onChange={(event) => setField(event.target.value)}>
            <option value="">{j.allFields}</option>
            {fields.map((name) => (
              <option key={name} value={name}>
                {named(name, 'item', registry, t)}
              </option>
            ))}
          </Select>
        </div>
        <div className="min-w-40">
          <label htmlFor="journal-who" className="label mb-1.5 block">
            {j.who}
          </label>
          <Select id="journal-who" value={who} onChange={(event) => setWho(event.target.value)}>
            <option value="">{j.everybody}</option>
            {everybody.map((name) => (
              <option key={name} value={name}>
                {whoIs(name, t)}
              </option>
            ))}
          </Select>
        </div>
        <div>
          <label htmlFor="journal-from" className="label mb-1.5 block">
            {j.from}
          </label>
          <Input id="journal-from" type="date" value={from} max={to || undefined} onChange={(event) => setFrom(event.target.value)} />
        </div>
        <div>
          <label htmlFor="journal-to" className="label mb-1.5 block">
            {j.to}
          </label>
          <Input id="journal-to" type="date" value={to} min={from || undefined} onChange={(event) => setTo(event.target.value)} />
        </div>
        {filtered ? (
          <Button
            size="sm"
            variant="quiet"
            onClick={() => {
              setField('')
              setWho('')
              setFrom('')
              setTo('')
            }}
          >
            {j.clear}
          </Button>
        ) : null}
      </div>

      {revert.isError ? (
        <p role="alert" className="flex items-center gap-2 border-b border-rule px-5 py-3 text-sm text-vermillion">
          <Glyph name="alert" className="size-4 shrink-0" />
          {revert.error instanceof ApiError ? revert.error.message : t.common.actionFailed}
        </p>
      ) : null}

      {journal.isPending ? (
        <div className="space-y-2 p-5">
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton key={index} className="h-14 w-full" />
          ))}
        </div>
      ) : journal.isError ? (
        <p role="alert" className="flex items-center gap-2 px-5 py-6 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {j.loadFailed}
        </p>
      ) : changes.length === 0 ? (
        <EmptyState title={filtered ? j.emptyFiltered : j.empty} hint={filtered ? undefined : j.emptyHint} />
      ) : (
        <ol className="divide-y divide-rule">
          {changes.map((change) => (
            <Change
              key={change.id}
              change={change}
              registry={registry}
              locale={locale}
              reverting={revert.isPending && revert.variables?.id === change.id}
              onRevert={() => revert.mutate(change)}
            />
          ))}
        </ol>
      )}

      {journal.hasNextPage ? (
        <div className="border-t border-rule px-5 py-3">
          <Button size="sm" disabled={journal.isFetchingNextPage} onClick={() => void journal.fetchNextPage()}>
            {journal.isFetchingNextPage ? <Spinner className="size-4" /> : <Glyph name="chevronDown" className="size-4" />}
            {j.more}
          </Button>
        </div>
      ) : null}
    </Panel>
  )
}

/** One change: where, when, who, why, and what the value held and took. */
function Change({
  change,
  registry,
  locale,
  reverting,
  onRevert,
}: {
  change: ChangeEntry
  registry: FieldRegistry
  locale: string
  reverting: boolean
  onRevert: () => void
}) {
  const { t } = useI18n()
  const j = t.admin.editor.journal
  const words = { yes: t.common.yes, no: t.common.no }
  const place = placeOf(change.scope)
  const where =
    place.kind === 'episode'
      ? episodeCode(place.season, place.episode)
      : place.kind === 'season'
        ? j.season(place.season)
        : place.kind === 'child'
          ? change.scope
          : undefined
  const before = shown(change.before, words)
  const after = shown(change.after, words)
  const bySource = BY_A_SOURCE.includes(change.cause)
  const kind = kindOf(change.scope, change.field)
  const what = kind
    ? change.before === null
      ? j.came[kind]
      : j.went[kind]
    : change.field === MOVED
      ? j.moved
      : undefined

  return (
    <li
      className={cn(
        'grid grid-cols-1 gap-x-4 gap-y-2 px-5 py-3 sm:grid-cols-[8rem_minmax(0,1fr)_auto]',
        // A person's change carries the brass edge a lock carries everywhere.
        bySource ? '' : 'border-l-2 border-brass pl-[calc(1.25rem-2px)]',
      )}
    >
      <p className="font-mono text-xs text-bone-faint tabular-nums sm:pt-0.5">
        <time dateTime={change.at} title={fmt.dateTime(change.at, locale)}>
          {fmt.relative(change.at, locale)}
        </time>
      </p>

      <div className="min-w-0">
        <p className="flex flex-wrap items-baseline gap-x-2 text-sm text-bone">
          {where ? <span className="font-mono text-xs text-bone-dim">{where}</span> : null}
          <span className="font-medium">{named(change.field, change.scope, registry, t)}</span>
          {change.language ? <span className="font-mono text-xs text-bone-faint">@{change.language}</span> : null}
          {what ? <span className="text-xs text-bone-dim">· {what}</span> : null}
        </p>
        {/* What it held taken out, and what it took put in: said so to a
            screen reader as well as struck through. */}
        <p className="mt-1 flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 text-sm leading-relaxed">
          <del className="line-clamp-3 min-w-0 break-words text-bone-faint decoration-bone-faint/60" title={before}>
            {before ?? <i>{j.nothing}</i>}
          </del>
          <Glyph name="chevronRight" className="size-3.5 shrink-0 self-center text-bone-faint" />
          <ins className="line-clamp-3 min-w-0 break-words text-bone no-underline" title={after}>
            {after ?? <i>{j.nothing}</i>}
          </ins>
        </p>
        <p className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-bone-dim">
          <Chip tone={bySource ? 'provider' : 'manual'}>{j.causes[change.cause]}</Chip>
          {change.provider ? <span>{providerName(change.provider)}</span> : null}
          {change.actor ? <span>{j.by(whoIs(change.actor, t))}</span> : bySource ? <span>{j.schedule}</span> : null}
          {change.wasLocked && bySource ? (
            <span className="flex items-center gap-1 text-brass">
              <Glyph name="lock" className="size-3" />
              {j.held}
            </span>
          ) : null}
        </p>
      </div>

      {revertible(change.scope, change.field, registry) ? (
        <div className="flex items-start sm:justify-end">
          <Button size="sm" variant="quiet" disabled={reverting} title={j.revertHint} onClick={onRevert}>
            {reverting ? <Spinner className="size-4" /> : <Glyph name="arrowLeft" className="size-4" />}
            {j.revert}
          </Button>
        </div>
      ) : null}
    </li>
  )
}

/** A field's name in the reader's language, wherever it is; an episode's move, or its coming and going, in words. */
function named(field: string, scope: string, registry: FieldRegistry, t: Dict): string {
  const j = t.admin.editor.journal
  if (field === MOVED) return j.fields.number
  const kind = kindOf(scope, field)
  if (kind) return j.fields[kind]
  const place = placeOf(scope)
  const defs = place.kind === 'season' ? registry.season : place.kind === 'episode' ? registry.episode : registry.item
  const def = defs.find((one) => one.name === field) ?? [...registry.item, ...registry.season, ...registry.episode].find((one) => one.name === field)
  return def ? fieldLabel(def, t) : ((t.labels.fields as Record<string, string>)[field] ?? field)
}

/** Who made a change, as a reader names them: a person by their name, a source by its own. */
function whoIs(label: string, t: Dict): string {
  if (!label.includes(':') && label !== 'anonymous') return providerName(label)
  const { name, role } = describeIdentity(label, t)
  return role ? `${name} · ${role}` : name
}
