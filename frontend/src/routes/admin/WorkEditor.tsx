/**
 * One entry, and everything a person may claim of it.
 *
 * The lock is the product, so it is the loudest thing on the page: a brass edge
 * down the row, a padlock beside the field's name and the word itself in a
 * chip. Anyone who cannot tell brass from slate can still read which values a
 * person decided and which a provider did, because none of it is said in colour
 * alone.
 *
 * Saving is what locks. That is the server's model — an override row is written
 * into a table the refresh path never touches — and the interface refuses to
 * invent a friendlier one, because an operator who thinks a field is locked
 * when it is not will lose their edit to the next sweep.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router'

import { useAdminTitle } from '../../components/AdminShell'
import { ManualChildren } from '../../components/ManualChildren'
import {
  Button,
  ButtonLink,
  Chip,
  Dialog,
  Field,
  FormField,
  Glyph,
  Input,
  Label,
  Panel,
  PanelHead,
  Provenance,
  Select,
  Skeleton,
  Spinner,
  Textarea,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n, type Dict } from '../../lib/i18n'
import { poster } from '../../lib/media'
import type { FieldDef, FieldRegistry, MediaItem, Override, Snapshot } from '../../lib/types'

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

/** Mirrors `TVDB_NUMBERED` in the merge engine: these supply the episode list. */
const NUMBERING = new Set(['tvdb', 'skyhook'])

export function WorkEditor() {
  const { id = '' } = useParams()
  const { t, locale } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const [language, setLanguage] = useState('')
  const [asking, setAsking] = useState<'unlockAll' | 'disable' | 'delete' | null>(null)

  const item = useQuery({
    queryKey: ['item', id, language],
    queryFn: () => api.get<MediaItem>(`/items/${id}${query({ language })}`),
  })
  const registry = useQuery({
    queryKey: ['fields'],
    queryFn: () => api.get<FieldRegistry>('/fields'),
    staleTime: Infinity,
  })
  // Who answered and when. The documents themselves run to megabytes.
  const snapshots = useQuery({
    queryKey: ['item', id, 'snapshots'],
    queryFn: () => api.get<Snapshot[]>(`/items/${id}/snapshots?payload=false`),
  })
  const overrides = useQuery({
    queryKey: ['overrides', id],
    queryFn: () => api.get<Override[]>(`/items/${id}/overrides`),
  })

  useAdminTitle(item.data?.title ?? null)

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', id] })
    void queryClient.invalidateQueries({ queryKey: ['overrides', id] })
    void queryClient.invalidateQueries({ queryKey: ['items'] })
    void queryClient.invalidateQueries({ queryKey: ['stats'] })
  }

  const refresh = useMutation({
    mutationFn: () => api.post<MediaItem>(`/items/${id}/refresh`),
    onSuccess: invalidate,
  })

  const unlockAll = useMutation({
    mutationFn: () => api.delete<{ removed: number }>(`/items/${id}/overrides`),
    onSuccess: () => {
      setAsking(null)
      invalidate()
    },
  })

  const setEnabled = useMutation({
    mutationFn: (enabled: boolean) => api.patch(`/items/${id}`, { isEnabled: enabled }),
    onSuccess: () => {
      setAsking(null)
      invalidate()
    },
  })

  const remove = useMutation({
    mutationFn: () => api.delete(`/items/${id}`),
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

  if (item.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {t.admin.editor.loadFailed}
      </p>
    )
  }

  // A row needs both the value and its definition, so wait for the pair.
  if (!item.data || !registry.data) {
    return <EditorSkeleton />
  }

  const work = item.data
  const locks = new Map(
    (overrides.data ?? []).filter((o) => o.scope === 'item').map((o) => [o.field, o]),
  )
  const sheet = poster(work)

  return (
    <div className="mx-auto max-w-5xl">
      <Link
        to="/admin/catalogue"
        className="label hidden min-h-11 items-center gap-2 transition-colors duration-150 hover:text-vermillion lg:inline-flex"
      >
        <Glyph name="arrowLeft" className="size-3.5" />
        {t.admin.catalogue}
      </Link>

      <header className="rise mb-8 flex gap-5">
        {sheet ? (
          <img
            src={sheet}
            alt={t.a11y.poster(work.title)}
            loading="lazy"
            className="hidden h-42 w-28 shrink-0 rounded-card border border-rule object-cover sm:block"
          />
        ) : null}

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <Chip tone="accent">
              <Glyph name={work.kind === 'series' ? 'tv' : 'film'} className="size-3" />
              {work.kind === 'series' ? t.nav.series : t.nav.films}
            </Chip>
            {work.isManual ? <Provenance manual label={t.work.manualEntry} /> : null}
            {work.status ? <Chip tone="provider">{work.status}</Chip> : null}
            {work.isEnabled ? null : <Chip tone="accent">{t.admin.works.disabled}</Chip>}
            {locks.size > 0 ? (
              <Chip tone="manual">
                <Glyph name="lock" className="size-3" />
                {t.admin.editor.lockCount(locks.size)}
              </Chip>
            ) : null}
          </div>

          <h1 className="mt-3 font-display text-3xl font-medium text-bone sm:text-4xl">
            {work.title}
          </h1>

          {work.overview ? (
            <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
              {work.overview}
            </p>
          ) : null}

          {language ? (
            <p className="mt-2 text-xs text-bone-faint">{t.admin.editor.translationNote}</p>
          ) : null}
        </div>
      </header>

      <div className="rise mb-8 flex flex-wrap items-end gap-2" style={{ animationDelay: '40ms' }}>
        <Button onClick={() => refresh.mutate()} disabled={refresh.isPending}>
          {refresh.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
          {refresh.isPending ? t.admin.editor.refreshing : t.admin.editor.refresh}
        </Button>

        {locks.size > 0 ? (
          <Button variant="danger" onClick={() => setAsking('unlockAll')}>
            <Glyph name="unlock" className="size-4" />
            {t.admin.editor.unlockAll}
          </Button>
        ) : null}

        <ButtonLink
          href={`/api/v1/items/${id}/nfo`}
          target="_blank"
          rel="noreferrer"
          title={t.admin.editor.nfoHint}
        >
          <Glyph name="download" className="size-4" />
          {t.admin.editor.nfo}
        </ButtonLink>

        <Button
          onClick={() => (work.isEnabled ? setAsking('disable') : setEnabled.mutate(true))}
          disabled={setEnabled.isPending}
        >
          <Glyph name="power" className="size-4" />
          {work.isEnabled ? t.admin.works.disable : t.admin.works.enable}
        </Button>

        <Button variant="danger" onClick={() => setAsking('delete')}>
          <Glyph name="trash" className="size-4" />
          {t.common.delete}
        </Button>

        <div className="w-full min-w-40 sm:ml-auto sm:w-auto">
          <label htmlFor="editor-language" className="label mb-1.5 block">
            {t.admin.editor.showIn}
          </label>
          <Select
            id="editor-language"
            value={language}
            onChange={(event) => setLanguage(event.target.value)}
          >
            <option value="">{t.admin.editor.asStored}</option>
            {LANGUAGES.map(([code, label]) => (
              <option key={code} value={code}>
                {label}
              </option>
            ))}
          </Select>
        </div>
      </div>

      {refresh.isError ? (
        <p role="alert" className="mb-6 flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4 shrink-0" />
          {refresh.error.message}
        </p>
      ) : null}

      {work.refreshError ? (
        <p className="mb-6 flex items-start gap-2 rounded-card border border-rule bg-ink-raised px-4 py-3 text-sm text-bone-dim">
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-vermillion" />
          <span>
            {t.admin.editor.refreshFailed}
            <span className="mt-0.5 block font-mono text-xs break-words text-vermillion">
              {work.refreshError}
            </span>
          </span>
        </p>
      ) : null}

      <Panel className="rise" style={{ animationDelay: '80ms' }}>
        <PanelHead
          title={t.admin.editor.fields}
          action={<span className="label hidden sm:inline">{t.admin.editor.fieldsHint}</span>}
        />
        <ul className="divide-y divide-rule">
          {registry.data.item.map((def) => (
            <FieldRow
              key={def.name}
              itemId={id}
              def={def}
              value={(work as unknown as Record<string, unknown>)[def.name]}
              lock={locks.get(def.name)}
              onChanged={invalidate}
            />
          ))}
        </ul>
      </Panel>

      <div className="mt-6">
        <ManualChildren item={work} onChanged={invalidate} />
      </div>

      <div
 className="mt-6 grid gap-6 lg:grid-cols-3">
        <Panel className="rise" style={{ animationDelay: '400ms' }}>
          <PanelHead title={t.work.identifiers} />
          <dl className="divide-y divide-rule">
            {Object.entries(work.externalIds).map(([source, value]) => (
              <Field key={source} label={source}>
                <span className="font-mono text-[0.8125rem] tabular-nums">
                  {Array.isArray(value) ? value.join(', ') : String(value)}
                </span>
              </Field>
            ))}
            <Field label="slug">
              <span className="font-mono text-[0.8125rem] break-all text-bone-dim">
                {work.slug}
              </span>
            </Field>
          </dl>
        </Panel>

        <Panel className="rise" style={{ animationDelay: '440ms' }}>
          <PanelHead title={t.admin.editor.record} />
          <dl className="divide-y divide-rule">
            <Field label={t.admin.editor.created}>{fmt.dateTime(work.createdAt, locale) ?? '—'}</Field>
            <Field label={t.admin.editor.updated}>{fmt.dateTime(work.updatedAt, locale) ?? '—'}</Field>
            <Field label={t.admin.editor.refreshed}>
              {fmt.relative(work.refreshedAt, locale) ?? '—'}
            </Field>
            <Field label={t.admin.editor.nextRefresh}>
              {fmt.relative(work.refreshAfter, locale) ?? '—'}
            </Field>
            {work.seasons?.length ? (
              <Field label={t.admin.editor.content}>
                {t.admin.editor.contentValue(work.seasons.length, work.episodes?.length ?? 0)}
              </Field>
            ) : null}
          </dl>
        </Panel>

        <Sources snapshots={snapshots.data} isManual={work.isManual} />
      </div>

      <Dialog
        open={asking === 'unlockAll'}
        title={t.admin.editor.unlockAllTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={unlockAll.isPending} onClick={() => unlockAll.mutate()}>
              <Glyph name="unlock" className="size-4" />
              {t.admin.editor.unlockAll}
            </Button>
          </>
        }
      >
        {t.admin.editor.unlockAllBody}
      </Dialog>

      <Dialog
        open={asking === 'disable'}
        title={t.admin.works.disableTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={setEnabled.isPending}
              onClick={() => setEnabled.mutate(false)}
            >
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
      </Dialog>
    </div>
  )
}

/* ── One field ────────────────────────────────────────────────────────────── */

function FieldRow({
  itemId,
  def,
  value,
  lock,
  onChanged,
}: {
  itemId: string
  def: FieldDef
  value: unknown
  lock?: Override
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')

  const save = useMutation({
    mutationFn: (parsed: unknown) =>
      api.put(`/items/${itemId}/overrides`, { scope: 'item', field: def.name, value: parsed }),
    onSuccess: () => {
      setEditing(false)
      onChanged()
    },
  })

  const unlock = useMutation({
    mutationFn: () => api.delete(`/items/${itemId}/overrides/item/${def.name}`),
    onSuccess: onChanged,
  })

  const locked = lock !== undefined
  const display = readable(value, t)
  const multiline = def.fieldType === 'longText'
  const inputId = `field-${def.name}`

  const begin = () => {
    setDraft(readable(value, t))
    setEditing(true)
  }

  return (
    <li
      // Names the row after the field it edits: useful when reading the DOM to
      // work out why a lock did not take, and stable for tests to hold on to.
      data-field={def.name}
      className={cn(
        'px-5 py-3 transition-colors duration-150',
        locked ? 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]' : '',
        editing ? '' : 'hover:bg-ink-high',
      )}
    >
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:gap-4">
        <div className="shrink-0 sm:w-44 sm:pt-1">
          <span className="flex items-center gap-1.5">
            {locked ? <Glyph name="lock" className="size-3.5 text-brass" /> : null}
            <Label className={locked ? 'text-brass' : undefined}>{def.label}</Label>
          </span>
          <span className="mt-0.5 block font-mono text-[0.625rem] text-bone-faint">
            {def.fieldType}
          </span>
        </div>

        <div className="min-w-0 flex-1">
          {editing ? (
            <form
              className="flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault()
                save.mutate(parse(draft, def))
              }}
            >
              <FormField
                label={def.label}
                htmlFor={inputId}
                hint={def.fieldType === 'textList' ? t.admin.editor.listHint : undefined}
                error={save.isError ? save.error.message : undefined}
              >
                {multiline ? (
                  <Textarea
                    id={inputId}
                    autoFocus
                    rows={4}
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                  />
                ) : (
                  <Input
                    id={inputId}
                    autoFocus
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                  />
                )}
              </FormField>

              <div className="flex flex-wrap gap-2">
                <Button type="submit" variant="primary" size="sm" disabled={save.isPending}>
                  {save.isPending ? <Spinner className="size-4" /> : <Glyph name="lock" className="size-4" />}
                  {t.admin.editor.saveAndLock}
                </Button>
                <Button type="button" size="sm" onClick={() => setEditing(false)}>
                  {t.common.cancel}
                </Button>
              </div>
            </form>
          ) : (
            <p
              className={cn(
                'text-sm leading-relaxed break-words',
                display ? (locked ? 'text-bone' : 'text-bone-dim') : 'text-bone-faint italic',
              )}
            >
              {display || t.common.notSet}
            </p>
          )}
        </div>

        {editing ? null : (
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            {locked ? (
              <>
                <Provenance
                  manual
                  label={
                    lock?.updatedAt
                      ? t.admin.editor.lockedOn(fmt.relative(lock.updatedAt, locale) ?? '')
                      : t.admin.editor.locked
                  }
                />
                <Button
                  size="sm"
                  onClick={() => unlock.mutate()}
                  disabled={unlock.isPending}
                  title={lock?.updatedBy ? t.admin.editor.lockedBy(lock.updatedBy) : undefined}
                >
                  <Glyph name="unlock" className="size-4" />
                  {t.admin.editor.unlock}
                </Button>
              </>
            ) : (
              <Button size="sm" variant="quiet" onClick={begin}>
                <Glyph name="pencil" className="size-4" />
                {t.common.edit}
              </Button>
            )}
          </div>
        )}
      </div>
    </li>
  )
}

/* ── Sources ──────────────────────────────────────────────────────────────── */

function Sources({ snapshots, isManual }: { snapshots?: Snapshot[]; isManual?: boolean }) {
  const { t, locale } = useI18n()

  const sorted = [...(snapshots ?? [])].sort((a, b) => b.fetchedAt.localeCompare(a.fetchedAt))

  return (
    <Panel className="rise" style={{ animationDelay: '480ms' }}>
      <PanelHead title={t.work.sources} />

      {sorted.length === 0 ? (
        <p className="px-5 py-4 text-sm text-bone-faint">
          {isManual ? t.admin.editor.handEntered : t.admin.editor.noSources}
        </p>
      ) : (
        <dl className="divide-y divide-rule">
          {sorted.map((snapshot) => (
            <Field key={snapshot.provider} label={snapshot.provider}>
              <span title={fmt.dateTime(snapshot.fetchedAt, locale)}>
                {fmt.relative(snapshot.fetchedAt, locale)}
              </span>
            </Field>
          ))}
        </dl>
      )}

      {sorted.some((snapshot) => NUMBERING.has(snapshot.provider)) ? (
        <p className="flex items-start gap-2 border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">
          <Glyph name="cloud" className="mt-0.5 size-3.5 shrink-0 text-slate" />
          {t.work.numbering}
        </p>
      ) : null}
    </Panel>
  )
}

/* ── Values ───────────────────────────────────────────────────────────────── */

/** What a stored value looks like in a box a person types into. */
function readable(value: unknown, t: Dict): string {
  if (value === null || value === undefined) return ''
  if (Array.isArray(value)) return value.join(', ')
  if (typeof value === 'boolean') return value ? t.common.yes : t.common.no
  return String(value)
}

/** Turn what was typed into the JSON shape the field expects. */
function parse(draft: string, def: FieldDef): unknown {
  const trimmed = draft.trim()

  // Clearing the box stores an explicit null, which still counts as an edit and
  // still locks the field: "this work has no network" is a decision too.
  if (trimmed === '') return null

  switch (def.fieldType) {
    case 'integer': {
      const parsed = Number.parseInt(trimmed, 10)
      return Number.isNaN(parsed) ? trimmed : parsed
    }
    case 'float': {
      const parsed = Number.parseFloat(trimmed)
      return Number.isNaN(parsed) ? trimmed : parsed
    }
    case 'boolean':
      return /^(true|yes|oui|1|on)$/i.test(trimmed)
    case 'textList':
      return trimmed
        .split(',')
        .map((part) => part.trim())
        .filter(Boolean)
    default:
      return trimmed
  }
}

function EditorSkeleton() {
  return (
    <div className="mx-auto max-w-5xl space-y-6">
      <div className="flex gap-5">
        <Skeleton className="hidden h-42 w-28 sm:block" />
        <div className="flex-1 space-y-3">
          <Skeleton className="h-5 w-40" />
          <Skeleton className="h-10 w-2/3" />
          <Skeleton className="h-16 w-full" />
        </div>
      </div>
      <Skeleton className="h-96 w-full" />
    </div>
  )
}
