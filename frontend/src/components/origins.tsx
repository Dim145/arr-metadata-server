/**
 * Where a work's values came from, said beside each of them, and the sources
 * it can be synced from.
 *
 * A value one provider gave names it; one several gave says so and names them
 * all, because agreement is what makes a value worth trusting; one a person
 * locked says that instead, whatever a provider would. Skyhook is not counted
 * beside TheTVDB: it republishes it, and two copies of one answer are not two
 * witnesses.
 */

import { useMutation } from '@tanstack/react-query'
import { useState, type ReactNode } from 'react'
import { Link } from 'react-router'

import { Button, Chip, Glyph, Label, Panel, PanelHead, Provenance, Spinner } from './ui'
import { ApiError, api } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n, type Dict } from '../lib/i18n'
import { providerName } from '../lib/labels'
import type { ProvenanceReport, SyncOutcome, SyncSource, ValueSource, WorkProvenance } from '../lib/types'

/** Mirrors `STUDIO_AUTHORITIES` in the merge engine. */
const STUDIO_AUTHORITIES = new Set(['anilist', 'mal'])

/** Lists the merge takes whole, from the first source that has one. */
const WHOLE_LISTS = new Set(['genres', 'keywords'])

/** Each provider once, Skyhook left out beside the TheTVDB it copies. */
export function witnesses(providers: string[]): string[] {
  const unique = [...new Set(providers)]
  return unique.includes('tvdb') ? unique.filter((p) => p !== 'skyhook') : unique
}

function names(providers: string[], locale: string): string {
  return fmt.list(providers.map(providerName), locale)
}

/** The chip beside a field: who gave its value, or that nobody did. */
export function OriginChip({ source, hasValue }: { source?: ValueSource; hasValue: boolean }) {
  const { t } = useI18n()
  const o = t.admin.editor.origin

  if (!source) {
    return hasValue ? (
      <Chip>
        <Glyph name="clock" className="size-3" />
        {o.keptChip}
      </Chip>
    ) : (
      <Chip className="border-dashed text-bone-faint">{o.noneChip}</Chip>
    )
  }

  const who = witnesses([source.from, ...(source.agreed ?? [])])
  if (who.length > 1) {
    return (
      <Chip tone="provider">
        <Glyph name="cloud" className="size-3" />
        {o.multi(who.length)}
      </Chip>
    )
  }
  return <Provenance manual={false} label={providerName(who[0] ?? source.from)} />
}

/**
 * Why the value is the one it is, in a sentence, when there is more to say
 * than the chip does: who else said the same, who said nothing, who said
 * otherwise and lost.
 */
export function originNote(
  name: string,
  source: ValueSource | undefined,
  { hasValue, locked }: { hasValue: boolean; locked: boolean },
  t: Dict,
  locale: string,
): string | null {
  const o = t.admin.editor.origin
  if (locked) return source ? o.underLock(providerName(source.from)) : null
  if (!source) return hasValue ? o.keptNote : null

  if (name === 'studio' && STUDIO_AUTHORITIES.has(source.from)) return o.authority

  const from = providerName(source.from)
  const who = witnesses([source.from, ...(source.agreed ?? [])])
  const passed = witnesses(source.passed ?? [])
  const differed = witnesses(source.differed ?? []).filter((p) => !who.includes(p))

  const said: string[] = []
  if (who.length > 1) said.push(o.same(names(who, locale)))
  if (passed.length) said.push(o.passed(names(passed, locale), passed.length, from))
  if (differed.length) said.push(o.differed(names(differed, locale), differed.length, from))
  if (!said.length && WHOLE_LISTS.has(name)) said.push(o.wholeList)
  return said.length ? said.join(' ') : null
}

/** What the chips mean, once, above the fields. */
export function OriginLegend() {
  const { t } = useI18n()
  const o = t.admin.editor.origin

  const entries: [ReactNode, string][] = [
    [<Provenance key="one" manual={false} label="TMDB" />, o.one],
    [
      <Chip key="many" tone="provider">
        <Glyph name="cloud" className="size-3" />
        {o.multi(3)}
      </Chip>,
      o.many,
    ],
    [<Provenance key="manual" manual label={t.admin.editor.locked} />, o.manual],
    [
      <Chip key="kept">
        <Glyph name="clock" className="size-3" />
        {o.keptChip}
      </Chip>,
      o.kept,
    ],
    [<Chip key="none" className="border-dashed text-bone-faint">{o.noneChip}</Chip>, o.none],
  ]

  return (
    <div className="rise mb-6" style={{ animationDelay: '60ms' }}>
      <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{o.legend}</p>
      <dl className="mt-3 flex flex-wrap items-center gap-x-5 gap-y-2">
        {entries.map(([chip, meaning]) => (
          <div key={meaning} className="flex items-center gap-2">
            <dt>{chip}</dt>
            <dd className="text-xs text-bone-faint">{meaning}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

/**
 * A row in the fields' list for what is not one field — the pictures, the
 * cast, the episodes — laid out like the fields around it, with where it
 * came from and a way to it.
 */
export function SummaryRow({
  label,
  value,
  chip,
  note,
  to,
}: {
  label: string
  value: string
  chip?: ReactNode
  note?: string | null
  /** The panel it is edited in, on this page. */
  to: string
}) {
  const { t } = useI18n()
  return (
    <li data-summary={to} className="px-5 py-3">
      <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-2 sm:flex sm:items-start sm:gap-4">
        <div className="min-w-0 sm:w-44 sm:shrink-0 sm:pt-1">
          <Label>{label}</Label>
        </div>
        <div className="col-span-2 min-w-0 sm:col-span-1 sm:flex-1">
          <p className="text-sm leading-relaxed text-bone-dim">{value}</p>
          {chip ? <div className="mt-2 sm:hidden">{chip}</div> : null}
          {note ? <p className="mt-1 text-xs leading-relaxed text-bone-faint">{note}</p> : null}
        </div>
        <div className="col-start-2 row-start-1 flex shrink-0 flex-wrap items-center justify-end gap-2 sm:col-start-auto sm:row-start-auto">
          {chip ? <span className="hidden sm:contents">{chip}</span> : null}
          <a
            href={`#${to}`}
            className="inline-flex min-h-9 items-center gap-1.5 rounded-full px-3 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
          >
            {t.admin.editor.origin.view}
            <span className="sr-only"> {label}</span>
            <Glyph name="chevronRight" className="size-3.5" />
          </a>
        </div>
      </div>
    </li>
  )
}

/** Images, counted by source: one chip, and the tally beneath it. */
export function imagesSummary(provenance: WorkProvenance, t: Dict, locale: string) {
  const counts = Object.entries(provenance.images ?? {})
    // Pictures added by hand, and those filed under no source, are nobody's.
    .filter(([source]) => source !== 'unknown' && source !== 'manual')
    .sort((a, b) => b[1] - a[1])
  const sources = counts.map(([source]) => source)
  const chip =
    sources.length > 1 ? (
      <Chip tone="provider">
        <Glyph name="cloud" className="size-3" />
        {t.admin.editor.origin.multi(sources.length)}
      </Chip>
    ) : sources[0] ? (
      <Provenance manual={false} label={providerName(sources[0])} />
    ) : null
  const tally = counts.map(([source, n]) => `${providerName(source)} ${fmt.count(n, locale)}`).join(' · ')
  return { chip, note: tally ? t.admin.editor.origin.imagesFrom(tally) : null }
}

/* ── Syncing from chosen sources ──────────────────────────────────────────── */

/**
 * Every provider that could describe the work, ticked to be asked again.
 *
 * Only a work whose provenance is known can be synced a source at a time:
 * the others are handed back what they gave from it. One that has none says
 * so, and offers the full refresh that records it.
 */
export function SourcesPanel({
  itemId,
  report,
  failed,
  onRetry,
  isManual,
  refreshAfter,
  fieldNames,
  onRaw,
  onRefresh,
  refreshing,
  onSynced,
}: {
  itemId: string
  report?: ProvenanceReport
  /** The report could not be loaded. */
  failed: boolean
  onRetry: () => void
  isManual?: boolean
  refreshAfter?: string
  /** The registry's fields: what counts as one in a source's tally. */
  fieldNames: Set<string>
  onRaw: (provider: string) => void
  onRefresh: () => void
  refreshing: boolean
  onSynced: () => void
}) {
  const { t, locale } = useI18n()
  const s = t.admin.editor.sync
  const [chosen, setChosen] = useState<Set<string>>(new Set())

  const sync = useMutation({
    mutationFn: (sources: string[]) => api.post<SyncOutcome>(`/items/${itemId}/sync`, { sources }),
    onSuccess: () => {
      setChosen(new Set())
      onSynced()
    },
    // Written by something else meanwhile: what the page shows is stale too.
    onError: (error) => {
      if (error instanceof ApiError && error.status === 409) onSynced()
    },
  })

  const provenance = report?.provenance
  const sources = report?.sources ?? []
  const traced = provenance !== undefined
  // Asked along, and not ticked themselves: each said once, with the first
  // ticked source that brings it.
  const brought = new Map<string, string>()
  for (const source of sources) {
    if (!chosen.has(source.provider)) continue
    for (const along of source.brings ?? []) {
      if (!chosen.has(along) && !brought.has(along)) brought.set(along, source.provider)
    }
  }
  const busy = sync.isPending || refreshing
  // A work entered by hand has sources only if it was given an id to be
  // asked by, or has answered for before.
  const answerable = sources.some((source) => !source.unavailable || source.fetchedAt)
  // Who numbers the list, as the server takes it: whoever is on record, or
  // TheTVDB. A source brought along that is not it fills the episodes in.
  const recorded = provenance?.episodes
  const numbering = recorded === 'tvdb' || recorded === 'skyhook' ? recorded : 'tvdb'

  const toggle = (provider: string, on: boolean) =>
    setChosen((held) => {
      const next = new Set(held)
      if (on) next.add(provider)
      else next.delete(provider)
      return next
    })

  return (
    <Panel id="sources" className="rise" style={{ animationDelay: '120ms' }}>
      <PanelHead
        title={s.title}
        action={
          refreshAfter ? (
            <span className="label text-right">{s.next(fmt.relative(refreshAfter, locale) ?? '')}</span>
          ) : undefined
        }
      />

      {failed ? (
        <div className="px-5 py-4">
          <p role="alert" className="flex items-start gap-2 text-sm text-vermillion">
            <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
            {s.loadFailed}
          </p>
          <Button size="sm" className="mt-3" onClick={onRetry}>
            <Glyph name="refresh" className="size-4" />
            {s.retry}
          </Button>
        </div>
      ) : report === undefined ? (
        <p className="px-5 py-4 text-sm text-bone-faint">
          <Spinner className="size-4" />
        </p>
      ) : isManual && !answerable ? (
        <p className="px-5 py-4 text-sm text-bone-faint">{t.admin.editor.handEntered}</p>
      ) : (
        <>
          {isManual ? (
            <p className="flex items-start gap-2 border-b border-rule px-5 py-3 text-xs leading-relaxed text-bone-dim">
              <Glyph name="pencil" className="mt-0.5 size-3.5 shrink-0 text-brass" />
              {s.manualNote}
            </p>
          ) : null}
          <ul className="divide-y divide-rule">
            {sources.map((source) => (
              <SourceRow
                key={source.provider}
                source={source}
                provenance={provenance}
                fieldNames={fieldNames}
                checked={chosen.has(source.provider)}
                disabled={!traced || source.unavailable !== undefined || busy}
                onChange={(on) => toggle(source.provider, on)}
                onRaw={() => onRaw(source.provider)}
              />
            ))}
          </ul>

          <div className="border-t border-rule p-5">
            {traced ? (
              <>
                <Button
                  variant="primary"
                  className="w-full justify-center"
                  disabled={chosen.size === 0 || busy}
                  onClick={() => sync.mutate([...chosen])}
                >
                  {sync.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
                  {sync.isPending ? s.syncing : chosen.size ? s.button(chosen.size) : s.choose}
                </Button>

                {[...brought].map(([along, by]) => (
                  <p key={along} className="mt-3 flex items-start gap-2 text-xs leading-relaxed text-bone-dim">
                    <Glyph name={along === 'tvmaze' ? 'clock' : 'list'} className="mt-0.5 size-3.5 shrink-0 text-slate" />
                    {along === 'tvmaze'
                      ? s.brings(providerName(by))
                      : along === numbering
                        ? s.bringsList(providerName(along), providerName(by))
                        : s.bringsAhead(providerName(along), providerName(by))}
                  </p>
                ))}

                <p className="mt-3 border-l-2 border-rule-bright pl-3 text-xs leading-relaxed text-bone-faint">
                  {s.explain}
                </p>
              </>
            ) : (
              <>
                <p className="text-sm leading-relaxed text-bone-dim">{t.admin.editor.origin.untraced}</p>
                <Button className="mt-3 w-full justify-center" disabled={refreshing} onClick={onRefresh}>
                  {refreshing ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
                  {refreshing ? t.admin.editor.refreshing : s.refreshOnce}
                </Button>
              </>
            )}

            <div role="status" aria-live="polite">
              {sync.isSuccess ? (
                <p className="mt-3 flex items-start gap-2 text-sm text-moss">
                  <Glyph name="check" className="mt-0.5 size-4 shrink-0" />
                  <span>
                    {s.done(names(sync.data.answered, locale))}
                    {sync.data.silent.length ? (
                      <span className="mt-1 block text-xs text-bone-dim">
                        {s.silent(names(sync.data.silent, locale), sync.data.silent.length)}
                      </span>
                    ) : null}
                  </span>
                </p>
              ) : null}
            </div>
            {sync.isError ? (
              <p role="alert" className="mt-3 flex items-start gap-2 text-sm text-vermillion">
                <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
                {sync.error instanceof ApiError && sync.error.status === 409
                  ? s.changed
                  : sync.error instanceof ApiError && sync.error.status === 502
                    ? s.noneAnswered
                    : sync.error.message || t.common.actionFailed}
              </p>
            ) : null}
          </div>
        </>
      )}
    </Panel>
  )
}

function SourceRow({
  source,
  provenance,
  fieldNames,
  checked,
  disabled,
  onChange,
  onRaw,
}: {
  source: SyncSource
  provenance?: WorkProvenance
  fieldNames: Set<string>
  checked: boolean
  disabled: boolean
  onChange: (on: boolean) => void
  onRaw: () => void
}) {
  const { t, locale } = useI18n()
  const s = t.admin.editor.sync
  const name = providerName(source.provider)
  const id = `sync-${source.provider}`

  return (
    <li className="flex items-start gap-3 px-5 py-2.5">
      <input
        id={id}
        type="checkbox"
        className="mt-3 size-4 shrink-0 cursor-pointer accent-vermillion disabled:cursor-not-allowed disabled:opacity-40"
        checked={checked}
        disabled={disabled}
        aria-describedby={`${id}-says`}
        onChange={(event) => onChange(event.target.checked)}
      />
      <div className="min-w-0 flex-1 py-1">
        <label
          htmlFor={id}
          className={cn(
            'flex min-h-8 items-center text-sm',
            source.unavailable ? 'text-bone-faint' : 'text-bone',
            disabled ? '' : 'cursor-pointer',
          )}
        >
          <span className="sr-only">{s.ask(name)}</span>
          <span aria-hidden>{name}</span>
        </label>
        <span id={`${id}-says`} className="block text-xs leading-relaxed text-bone-faint">
          {describe(source, provenance, fieldNames, t, locale)}
        </span>
      </div>
      {source.fetchedAt ? (
        <button
          type="button"
          onClick={onRaw}
          className="hit mt-1.5 min-h-8 min-w-11 shrink-0 cursor-pointer rounded-card px-2 font-mono text-[0.6875rem] text-slate transition-colors duration-150 hover:bg-ink-high hover:text-bone"
          aria-label={t.admin.editor.rawOf(name)}
        >
          {'{ }'}
        </button>
      ) : null}
    </li>
  )
}

/** "answered 3 hours ago · 14 fields, 25 images, the episodes". */
function describe(
  source: SyncSource,
  provenance: WorkProvenance | undefined,
  fieldNames: Set<string>,
  t: Dict,
  locale: string,
): string {
  const s = t.admin.editor.sync
  const when = source.fetchedAt ? s.answered(fmt.relative(source.fetchedAt, locale) ?? '') : null
  if (source.unavailable === 'off') return s.off
  if (source.unavailable === 'noId') return s.noId
  if (!when) return s.never
  if (!provenance) return when

  const provider = source.provider
  const fields = Object.entries(provenance.fields ?? {}).filter(([field]) => fieldNames.has(field))
  const gave = fields.filter(([, value]) => value.from === provider).length
  const confirmed = fields.filter(([, value]) => value.agreed?.includes(provider)).length
  const images = provenance.images?.[provider] ?? 0
  const translations = Object.values(provenance.translations ?? {}).filter((p) => p === provider).length
  const ratings = Object.values(provenance.ratings ?? {}).filter((p) => p === provider).length

  const parts: string[] = []
  if (gave) parts.push(s.fields(gave))
  if (images) parts.push(s.images(images))
  if (provenance.episodes === provider) parts.push(s.theEpisodes)
  if (provenance.fields?.credits?.from === provider) parts.push(s.theCredits)
  if (provenance.fields?.relations?.from === provider) parts.push(s.theRelations)
  if (translations) parts.push(s.translations(translations))
  if (ratings) parts.push(s.ratings(ratings))
  if (!parts.length) parts.push(confirmed ? s.confirms(confirmed) : s.nothing)

  return `${when} · ${parts.join(', ')}`
}

/** Where the rules each source follows are laid out. */
export function RulesPanel() {
  const { t } = useI18n()
  const s = t.admin.editor.sync
  return (
    <Panel className="rise" style={{ animationDelay: '160ms' }}>
      <PanelHead title={s.rules} />
      <p className="px-5 py-4 text-sm leading-relaxed text-bone-dim">
        {s.rulesBody}{' '}
        <Link to="/admin/sources" className="text-vermillion underline-offset-4 hover:underline">
          {s.rulesLink}
          <Glyph name="chevronRight" className="ml-0.5 inline size-3.5 align-[-0.15em]" />
        </Link>
      </p>
    </Panel>
  )
}
