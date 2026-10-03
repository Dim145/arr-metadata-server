/**
 * The record itself: every field a person may claim, grouped the way a
 * catalogue card groups them — what the work is, what it is about, when and
 * where it was shown, how it is classed, where it leads — and beside them who
 * gave each value and the sources it can be asked of again.
 *
 * The fields of the other kind of work — a film's release dates on a series —
 * are kept rather than hidden, folded under their own head: a source does
 * sometimes give one, and a lock on one has to be findable.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useMemo, useState } from 'react'
import { useNavigate } from 'react-router'

import { OriginChip, OriginLegend, RulesPanel, SourcesPanel, SummaryRow, imagesSummary, witnesses } from '../../../components/origins'
import {
  Button,
  Dialog,
  Field,
  Glyph,
  Panel,
  PanelHead,
  Provenance,
  Segmented,
  Select,
  Skeleton,
  Toggle,
} from '../../../components/ui'
import { api, query } from '../../../lib/api'
import * as fmt from '../../../lib/format'
import { useI18n } from '../../../lib/i18n'
import { providerName } from '../../../lib/labels'
import { seasonNumbers } from '../../../lib/media'
import type { FieldDef, FieldRegistry, MediaItem, MediaKind, Override, ProvenanceReport, Snapshot, SyncSource, WorkProvenance } from '../../../lib/types'

import { FieldRow, hasValue } from './FieldRow'
import { Count, Empty, LockBadge } from './shared'

/** Worth offering without asking the server which translations it holds. */
const LANGUAGES = [
  ['en', 'English'],
  ['fr', 'Français'],
  ['de', 'Deutsch'],
  ['es', 'Español'],
  ['it', 'Italiano'],
  ['pt', 'Português'],
  ['nl', 'Nederlands'],
  ['ja', '日本語'],
  ['ko', '한국어'],
  ['zh', '中文'],
  ['ru', 'Русский'],
] as const

/** Fields edited elsewhere than in a row: the identifiers on their own tab, the poster and the background among the artwork. */
export const PANEL_FIELDS = new Set(['externalIds', 'primaryPoster', 'primaryFanart'])

/** How the fields are grouped, and which belong to one kind of work only. */
const GROUPS: { id: 'identity' | 'synopsis' | 'broadcast' | 'release' | 'classification' | 'links'; fields: string[]; only?: MediaKind }[] = [
  { id: 'identity', fields: ['title', 'originalTitle', 'sortTitle', 'titleQualifier', 'year', 'status', 'isAdult', 'slug'] },
  { id: 'synopsis', fields: ['overview'] },
  { id: 'broadcast', only: 'series', fields: ['firstAired', 'lastAired', 'airTime', 'network', 'studio', 'runtime'] },
  { id: 'release', only: 'movie', fields: ['inCinemas', 'digitalRelease', 'physicalRelease', 'studio', 'runtime', 'collectionTmdbId'] },
  { id: 'classification', fields: ['genres', 'keywords', 'contentRating', 'contentRatingCountry', 'originalLanguage', 'originalCountry'] },
  { id: 'links', fields: ['homepage', 'trailerYoutubeId', 'themeMusic'] },
]

type Filter = 'all' | 'locked' | 'empty'

export function RecordTab({
  work,
  registry,
  overrides,
  report,
  reportFailed,
  onRetryReport,
  language,
  onLanguage,
  refreshing,
  onRefresh,
  onChanged,
}: {
  work: MediaItem
  registry: FieldRegistry
  overrides: Override[]
  report?: ProvenanceReport
  reportFailed: boolean
  onRetryReport: () => void
  language: string
  onLanguage: (next: string) => void
  refreshing: boolean
  onRefresh: () => void
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const e = t.admin.editor
  const [filter, setFilter] = useState<Filter>('all')
  const [otherOpen, setOtherOpen] = useState(false)
  const [viewing, setViewing] = useState<string | null>(null)

  const locks = useMemo(() => new Map(overrides.filter((o) => o.scope === 'item').map((o) => [o.field, o])), [overrides])
  const provenance = report?.provenance
  const traced = provenance !== undefined
  const fieldNames = useMemo(() => new Set(registry.item.map((def) => def.name)), [registry])
  const byName = useMemo(() => new Map(registry.item.map((def) => [def.name, def])), [registry])
  const record = work as unknown as Record<string, unknown>

  // Every group's fields, in the registry's definitions; then whatever the
  // registry has that no group names, which is the other kind's and the new.
  const placed = new Set<string>()
  const groups = GROUPS.filter((group) => !group.only || group.only === work.kind).map((group) => {
    const defs = group.fields.map((name) => byName.get(name)).filter((def): def is FieldDef => def !== undefined)
    defs.forEach((def) => placed.add(def.name))
    return { id: group.id, defs }
  })
  const other = registry.item.filter((def) => !placed.has(def.name) && !PANEL_FIELDS.has(def.name))
  // Unfolded of itself when one of them says something.
  const otherSpeaks = other.some((def) => hasValue(def, record[def.name]) || locks.has(def.name))

  const matches = (def: FieldDef) =>
    filter === 'all' || (filter === 'locked' ? locks.has(def.name) : !hasValue(def, record[def.name]))
  const counts = {
    all: registry.item.filter((def) => !PANEL_FIELDS.has(def.name)).length,
    locked: registry.item.filter((def) => !PANEL_FIELDS.has(def.name) && locks.has(def.name)).length,
    empty: registry.item.filter((def) => !PANEL_FIELDS.has(def.name) && !hasValue(def, record[def.name])).length,
  }

  const rows = (defs: FieldDef[]) =>
    defs.filter(matches).map((def) => (
      <FieldRow
        key={def.name}
        itemId={work.id}
        def={def}
        value={record[def.name]}
        lock={locks.get(def.name)}
        origin={provenance?.fields?.[def.name]}
        // The slug is made here, from the title and the year.
        traced={traced && def.name !== 'slug'}
        onChanged={onChanged}
      />
    ))

  const shownGroups = groups.map((group) => ({ ...group, rows: rows(group.defs) })).filter((group) => group.rows.length)
  const otherRows = rows(other)
  const nothing = shownGroups.length === 0 && otherRows.length === 0

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-3">
        <Segmented<Filter>
          label={e.filter.label}
          value={filter}
          onChange={setFilter}
          options={[
            { value: 'all', label: <Choice label={e.filter.all} n={counts.all} /> },
            { value: 'locked', label: <Choice label={e.filter.locked} n={counts.locked} /> },
            { value: 'empty', label: <Choice label={e.filter.empty} n={counts.empty} /> },
          ]}
        />
        <div className="flex items-center gap-3">
          <label htmlFor="editor-language" className="label">
            {e.showIn}
          </label>
          <Select id="editor-language" value={language} onChange={(event) => onLanguage(event.target.value)} className="w-auto min-w-36">
            <option value="">{e.asStored}</option>
            {LANGUAGES.map(([code, label]) => (
              <option key={code} value={code}>
                {label}
              </option>
            ))}
          </Select>
        </div>
      </div>
      {language ? <p className="mb-4 text-xs text-bone-faint">{e.translationNote}</p> : null}

      {traced ? <OriginLegend /> : null}

      <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:items-start">
        <div id="fields" className="scroll-mt-28 space-y-5 lg:scroll-mt-16">
          {shownGroups.map((group) => (
            <Panel key={group.id} id={`group-${group.id}`} label={e.groups[group.id]}>
              <PanelHead
                title={
                  <span className="flex items-center gap-2">
                    {e.groups[group.id]}
                    <LockBadge n={group.defs.filter((def) => locks.has(def.name)).length} />
                  </span>
                }
                action={<Count>{e.fieldCount(group.rows.length)}</Count>}
              />
              <ul className="divide-y divide-rule">{group.rows}</ul>
            </Panel>
          ))}

          {otherRows.length ? (
            <Panel id="group-other" label={e.groups.other}>
              <PanelHead
                title={
                  <span className="flex items-center gap-2">
                    {e.groups.other}
                    <LockBadge n={other.filter((def) => locks.has(def.name)).length} />
                  </span>
                }
                action={
                  <Button size="sm" variant="quiet" aria-expanded={otherOpen || otherSpeaks} onClick={() => setOtherOpen((held) => !held)}>
                    <Glyph name={otherOpen || otherSpeaks ? 'chevronUp' : 'chevronDown'} className="size-4" />
                    {otherOpen || otherSpeaks ? e.hide : `${e.show} · ${e.fieldCount(otherRows.length)}`}
                  </Button>
                }
              />
              {otherOpen || otherSpeaks ? (
                <>
                  <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">{e.groups.otherHint}</p>
                  <ul className="mt-2 divide-y divide-rule border-t border-rule">{otherRows}</ul>
                </>
              ) : null}
            </Panel>
          ) : null}

          {nothing ? (
            <Panel>
              <Empty>{e.filter.none}</Empty>
            </Panel>
          ) : null}

          {provenance && filter === 'all' ? (
            <Panel id="group-also" label={e.groups.also}>
              <PanelHead title={e.groups.also} />
              <ul className="divide-y divide-rule">
                <Summaries work={work} provenance={provenance} sources={report?.sources ?? []} />
              </ul>
            </Panel>
          ) : null}
        </div>

        {/* Held in view beside the fields, and scrolled on its own when it is
            taller than the window. */}
        <div className="space-y-6 lg:sticky lg:top-16 lg:max-h-[calc(100dvh-5rem)] lg:overflow-y-auto lg:overscroll-contain lg:rounded-panel">
          <SourcesPanel
            itemId={work.id}
            report={report}
            failed={reportFailed}
            onRetry={onRetryReport}
            isManual={work.isManual}
            refreshAfter={work.refreshAfter}
            fieldNames={fieldNames}
            onRaw={setViewing}
            onRefresh={onRefresh}
            refreshing={refreshing}
            onSynced={onChanged}
          />

          <Panel id="record" label={e.record}>
            <PanelHead title={e.record} />
            <dl className="divide-y divide-rule">
              <Field label={e.created}>{fmt.dateTime(work.createdAt, locale) ?? '—'}</Field>
              <Field label={e.updated}>{fmt.dateTime(work.updatedAt, locale) ?? '—'}</Field>
              <Field label={e.refreshed}>{fmt.relative(work.refreshedAt, locale) ?? '—'}</Field>
              <Field label={e.nextRefresh}>{fmt.relative(work.refreshAfter, locale) ?? '—'}</Field>
              {work.seasons?.length ? (
                <Field label={e.content}>{e.contentValue(work.seasons.length, work.episodes?.length ?? 0)}</Field>
              ) : null}
            </dl>
          </Panel>

          <Maintenance work={work} allLocks={overrides.length} deeperLocks={overrides.length - locks.size} onChanged={onChanged} />

          <RulesPanel />
        </div>
      </div>

      <RawSnapshot itemId={work.id} provider={viewing} onClose={() => setViewing(null)} />
    </div>
  )
}

function Choice({ label, n }: { label: string; n: number }) {
  return (
    <span className="inline-flex items-center gap-1.5">
      {label}
      <Count>{n}</Count>
    </span>
  )
}

/* ── Keeping the record ───────────────────────────────────────────────────── */

/**
 * What is done to the record as a whole: whether the clients are served it,
 * and the two things that cannot be taken back lightly — every lock lifted at
 * once, and the record removed.
 */
function Maintenance({
  work,
  allLocks,
  deeperLocks,
  onChanged,
}: {
  work: MediaItem
  allLocks: number
  deeperLocks: number
  onChanged: () => void
}) {
  const { t } = useI18n()
  const e = t.admin.editor
  const m = e.maintenance
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [asking, setAsking] = useState<'unlockAll' | 'disable' | 'delete' | null>(null)

  const unlockAll = useMutation({
    mutationFn: () => api.delete<{ removed: number }>(`/items/${work.id}/overrides`),
    onSuccess: () => {
      setAsking(null)
      onChanged()
    },
  })

  const setEnabled = useMutation({
    mutationFn: (enabled: boolean) => api.patch(`/items/${work.id}`, { isEnabled: enabled }),
    onSuccess: () => {
      setAsking(null)
      onChanged()
    },
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/items/${work.id}`),
    onSuccess: () => {
      setAsking(null)
      // Only the lists: this entry's own queries are deliberately left alone,
      // because either invalidating or dropping them makes the observer that is
      // still mounted ask the server for a work it has just deleted.
      void queryClient.invalidateQueries({ queryKey: ['items'] })
      void queryClient.invalidateQueries({ queryKey: ['stats'] })
      navigate('/admin/catalogue', { replace: true })
    },
  })

  return (
    <Panel label={m.title}>
      <PanelHead title={m.title} />
      <div className="space-y-3 p-5">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm text-bone">{m.served}</p>
            <p className="text-xs text-bone-faint">{work.isEnabled ? m.servedHint : m.notServed}</p>
          </div>
          <Toggle
            checked={work.isEnabled}
            label={m.served}
            disabled={setEnabled.isPending}
            onChange={(next) => (next ? setEnabled.mutate(true) : setAsking('disable'))}
          />
        </div>
        {setEnabled.isError ? (
          <p role="alert" className="text-xs text-vermillion">
            {setEnabled.error.message || t.common.actionFailed}
          </p>
        ) : null}

        {allLocks > 0 ? (
          <Button size="sm" className="w-full justify-start" onClick={() => setAsking('unlockAll')}>
            <Glyph name="unlock" className="size-4" />
            {e.unlockAll}
            <Count className="ml-auto">{allLocks}</Count>
          </Button>
        ) : null}

        <Button size="sm" variant="danger" className="w-full justify-start" onClick={() => setAsking('delete')}>
          <Glyph name="trash" className="size-4" />
          {t.common.delete}
        </Button>
      </div>

      <Dialog
        open={asking === 'unlockAll'}
        title={e.unlockAllTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={unlockAll.isPending} onClick={() => unlockAll.mutate()}>
              <Glyph name="unlock" className="size-4" />
              {e.unlockAll}
            </Button>
          </>
        }
      >
        {e.unlockAllBody}
        {deeperLocks > 0 ? <p className="mt-3 text-sm text-brass">{e.unlockAllDeeper(deeperLocks)}</p> : null}
        {unlockAll.isError ? (
          <p role="alert" className="mt-3 text-sm text-vermillion">
            {unlockAll.error.message || t.common.actionFailed}
          </p>
        ) : null}
      </Dialog>

      <Dialog
        open={asking === 'disable'}
        title={t.admin.works.disableTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={setEnabled.isPending} onClick={() => setEnabled.mutate(false)}>
              {t.admin.works.disable}
            </Button>
          </>
        }
      >
        {t.admin.works.disableBody(work.title)}
      </Dialog>

      <Dialog
        open={asking === 'delete'}
        title={t.admin.works.deleteTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={remove.isPending} onClick={() => remove.mutate()}>
              <Glyph name="trash" className="size-4" />
              {t.common.delete}
            </Button>
          </>
        }
      >
        {t.admin.works.deleteBody(work.title)}
        {remove.isError ? (
          <p role="alert" className="mt-3 text-sm text-vermillion">
            {remove.error.message || t.common.actionFailed}
          </p>
        ) : null}
      </Dialog>
    </Panel>
  )
}

/* ── What is not one field ────────────────────────────────────────────────── */

/**
 * The lists the other tabs hold, said beside the fields with where they came
 * from: the pictures, gathered from every source; the cast, one source's
 * whole; the episodes, one source's list with the others filling it in.
 */
function Summaries({ work, provenance, sources }: { work: MediaItem; provenance: WorkProvenance; sources: SyncSource[] }) {
  const { t, locale } = useI18n()
  const o = t.admin.editor.origin
  const images = imagesSummary(provenance, t, locale)
  const spine = provenance.episodes
  const pictures =
    (work.images?.length ?? 0) + (work.seasons ?? []).reduce((sum, season) => sum + (season.images?.length ?? 0), 0)

  const answered = new Set(sources.filter((source) => source.fetchedAt).map((source) => source.provider))
  const fillers = witnesses(['tmdb', 'tvdb', 'skyhook'].filter((p) => p !== spine && answered.has(p)))

  return (
    <>
      {pictures ? <SummaryRow label={o.images} value={o.imageCount(pictures)} chip={images.chip} note={images.note} to="artwork" /> : null}
      {work.credits?.length ? (
        <SummaryRow
          label={o.credits}
          value={o.creditCount(work.credits.length)}
          chip={<OriginChip source={provenance.fields?.credits} hasValue />}
          note={o.creditsFrom}
          to="credits"
        />
      ) : null}
      {work.kind === 'series' && work.episodes?.length ? (
        <SummaryRow
          label={o.episodes}
          value={t.admin.editor.contentValue(work.seasons?.length ?? 0, work.episodes.length)}
          chip={spine ? <Provenance manual={false} label={providerName(spine)} /> : null}
          note={
            spine
              ? [
                  o.episodesFrom(providerName(spine)),
                  answered.has('tvmaze') && spine !== 'tvmaze' ? o.episodesTimes : null,
                  fillers.length ? o.episodesFill(fmt.list(fillers.map(providerName), locale)) : null,
                ]
                  .filter(Boolean)
                  .join(' ')
              : null
          }
          to={seasonNumbers(work).length ? 'season-fields' : 'episodes'}
        />
      ) : null}
    </>
  )
}

/* ── What a provider actually said ────────────────────────────────────────── */

/**
 * What one provider answered, verbatim.
 *
 * For the question the merged record cannot answer: did the provider say
 * this, or did this server get it wrong? Fetched only when asked for — a long
 * series' documents run to megabytes.
 */
function RawSnapshot({ itemId, provider, onClose }: { itemId: string; provider: string | null; onClose: () => void }) {
  const { t } = useI18n()
  const [copied, setCopied] = useState<'no' | 'yes' | 'failed'>('no')

  const raw = useQuery({
    queryKey: ['item', itemId, 'snapshot', provider],
    queryFn: () => api.get<Snapshot[]>(`/items/${itemId}/snapshots${query({ provider: provider ?? '' })}`),
    enabled: provider !== null,
    staleTime: 60_000,
  })

  const payload = raw.data?.[0]?.payload
  const text = useMemo(() => (payload === undefined ? '' : JSON.stringify(payload, null, 2)), [payload])

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied('yes')
    } catch {
      setCopied('failed')
    }
  }

  return (
    <Dialog
      open={provider !== null}
      title={provider ? t.admin.editor.rawOf(providerName(provider)) : ''}
      onClose={() => {
        setCopied('no')
        onClose()
      }}
      footer={
        <>
          <span role="status" className="mr-auto text-xs text-bone-faint">
            {copied === 'failed' ? t.admin.editor.copyFailed : ''}
          </span>
          <Button disabled={!text} onClick={() => void copy()}>
            <Glyph name={copied === 'yes' ? 'check' : 'copy'} className="size-4" />
            {copied === 'yes' ? t.admin.editor.copied : t.admin.editor.copy}
          </Button>
          <Button onClick={onClose}>{t.nav.close}</Button>
        </>
      }
    >
      {raw.isPending ? (
        <Skeleton className="h-64 w-full" />
      ) : raw.isError ? (
        <p role="alert" className="text-sm text-vermillion">
          {t.admin.editor.rawFailed}
        </p>
      ) : (
        <pre
          tabIndex={0}
          role="region"
          aria-label={provider ? t.admin.editor.rawOf(providerName(provider)) : undefined}
          className="max-h-[60dvh] overflow-auto rounded-card border border-rule bg-ink p-3 font-mono text-[0.6875rem] leading-relaxed text-bone-dim"
        >
          {text}
        </pre>
      )}
    </Dialog>
  )
}
