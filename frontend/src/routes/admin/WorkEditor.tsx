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
import { useEffect, useMemo, useRef, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router'

import { useAdminTitle } from '../../components/AdminShell'
import { ManualChildren } from '../../components/ManualChildren'
import { Artwork } from '../../components/media'
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
  OnThisPage,
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
import { providerName, statusLabel } from '../../lib/labels'
import { useI18n, type Dict } from '../../lib/i18n'
import { episodeCode, episodesOf, poster, seasonName, seasonNumbers } from '../../lib/media'
import type { Episode, FieldDef, FieldRegistry, MediaItem, Override, Snapshot } from '../../lib/types'

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
    // The catalogue's own pages read the same work under another key.
    void queryClient.invalidateQueries({ queryKey: ['work', id] })
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

  // The registry too, and not only the work: it is asked for once and cached
  // forever, so a failure there never retries. Without this the page waits on
  // `registry.data` that is never coming and shimmers for as long as it is open.
  if (item.isError || registry.isError) {
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
  // Every lock, the seasons' and episodes' too: unlocking everything lifts
  // them all, and counting only the work's own let one confirmation undo
  // thirty episode edits it never mentioned.
  const allLocks = overrides.data?.length ?? 0
  const deeperLocks = allLocks - locks.size
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
          <Artwork
            url={sheet}
            role="card"
            alt={t.a11y.poster(work.title)}
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
            {work.status ? <Chip tone="provider">{statusLabel(work.status, t)}</Chip> : null}
            {work.isEnabled ? null : <Chip tone="accent">{t.admin.works.disabled}</Chip>}
            {allLocks > 0 ? (
              <Chip tone="manual">
                <Glyph name="lock" className="size-3" />
                {t.admin.editor.lockCount(allLocks)}
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

      {/* Two to a row on a phone: five buttons one under another, then the
          language, were a whole screen before the first field. */}
      <div className="rise mb-8 grid grid-cols-2 gap-2 sm:flex sm:flex-wrap sm:items-end" style={{ animationDelay: '40ms' }}>
        <Button onClick={() => refresh.mutate()} disabled={refresh.isPending}>
          {refresh.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
          {refresh.isPending ? t.admin.editor.refreshing : t.admin.editor.refresh}
        </Button>

        {allLocks > 0 ? (
          <Button variant="danger" onClick={() => setAsking('unlockAll')}>
            <Glyph name="unlock" className="size-4" />
            {t.admin.editor.unlockAll}
          </Button>
        ) : null}

        <Link
          to={`/work/${id}`}
          className="inline-flex min-h-11 items-center gap-2 rounded-full border border-rule-bright px-5 text-sm font-medium text-bone transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high"
        >
          <Glyph name="reel" className="size-4" />
          {t.admin.editor.publicPage}
        </Link>

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

        <div className="col-span-2 w-full min-w-40 sm:ml-auto sm:w-auto">
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

      <OnThisPage
        label={t.admin.inPage}
        entries={[
          { id: 'fields', label: t.admin.editor.fields },
          { id: 'credits', label: t.admin.editor.children.credits },
          { id: 'titles', label: t.admin.editor.children.titles },
          { id: 'artwork', label: t.admin.editor.children.artwork },
          ...(work.kind === 'series'
            ? [
                { id: 'seasons', label: t.admin.editor.children.seasons },
                { id: 'episodes', label: t.admin.editor.children.episodes },
                ...(seasonNumbers(work).length ? [{ id: 'season-fields', label: t.admin.editor.seasons }] : []),
              ]
            : []),
          { id: 'identifiers', label: t.work.identifiers },
          { id: 'record', label: t.admin.editor.record },
          { id: 'sources', label: t.work.sources },
        ]}
      />

      <Panel id="fields" className="rise" style={{ animationDelay: '80ms' }}>
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

      {work.kind === 'series' && seasonNumbers(work).length ? (
        <div className="mt-6">
          <SeasonsEditor
            work={work}
            registry={registry.data}
            overrides={overrides.data ?? []}
            onChanged={invalidate}
          />
        </div>
      ) : null}

      <div
 className="mt-6 grid gap-6 lg:grid-cols-3">
        <Panel id="identifiers" className="rise" style={{ animationDelay: '400ms' }}>
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

        <Panel id="record" className="rise" style={{ animationDelay: '440ms' }}>
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

        <Sources itemId={id} snapshots={snapshots.data} isManual={work.isManual} />
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
        {deeperLocks > 0 ? (
          <p className="mt-3 text-sm text-brass">{t.admin.editor.unlockAllDeeper(deeperLocks)}</p>
        ) : null}
        {/* Without this the dialog stays open with a re-enabled button and no
            reason, which reads as "press it again". */}
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
  scope = 'item',
  def,
  value,
  lock,
  onChanged,
}: {
  itemId: string
  /** `item`, `season:3` or `episode:3x7` — what the override addresses. */
  scope?: string
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
      api.put(`/items/${itemId}/overrides`, { scope, field: def.name, value: parsed }),
    onSuccess: () => {
      setEditing(false)
      onChanged()
    },
  })

  const unlock = useMutation({
    mutationFn: () =>
      api.delete(`/items/${itemId}/overrides/${encodeURIComponent(scope)}/${def.name}`),
    onSuccess: onChanged,
  })

  const locked = lock !== undefined
  const display = readable(value, t)
  const multiline = def.fieldType === 'longText'
  const inputId = `field-${scope.replace(/\W/g, '-')}-${def.name}`

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
      {/* On a phone the buttons sit on the label's line, and the value runs
          the width under both: a row was four lines tall, and twenty-eight of
          them made a page nobody could scan. */}
      <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-2 sm:flex sm:items-start sm:gap-4">
        <div className="min-w-0 sm:w-44 sm:shrink-0 sm:pt-1">
          <span className="flex items-center gap-1.5">
            {locked ? <Glyph name="lock" className="size-3.5 text-brass" /> : null}
            <Label className={locked ? 'text-brass' : undefined}>{fieldLabel(def, t)}</Label>
          </span>
          {/* What shape the value takes, in words — "Date · AAAA-MM-JJ" tells
              somebody what to type, where "date" and "timeOfDay" were the
              names of an enum. */}
          <span className="mt-0.5 block font-mono text-[0.6875rem] text-bone-faint">
            {(t.labels.fieldTypes as Record<string, string>)[def.fieldType] ?? def.fieldType}
          </span>
        </div>

        <div className="col-span-2 min-w-0 sm:col-span-1 sm:flex-1">
          {/* A lock that did not lift, with nothing said, is this screen's own
              failure mode running backwards: somebody believing a field is one
              thing while the server holds another. The save path already
              reports itself, inside the form below. */}
          {unlock.isError ? (
            <p role="alert" className="mb-2 text-xs text-vermillion">
              {unlock.error.message || t.common.actionFailed}
            </p>
          ) : null}

          {editing ? (
            <form
              className="flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault()
                save.mutate(parse(draft, def))
              }}
            >
              <FormField
                label={fieldLabel(def, t)}
                htmlFor={inputId}
                hint={
                  def.fieldType === 'textList'
                    ? t.admin.editor.listHint
                    : def.fieldType === 'dateTime'
                      ? t.admin.editor.dateTimeHint
                      : undefined
                }
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
              {/* A picture's address is the picture, with the address kept
                  for whoever needs it: a hundred-character URL said nothing
                  a thumbnail does not. */}
              {display && def.name === 'image' && /^https?:\/\//.test(display) ? (
                <span className="flex items-center gap-3">
                  <Artwork url={display} role="still" alt="" className="h-14 w-24 shrink-0 rounded-card border border-rule object-cover" />
                  <span className="min-w-0 truncate font-mono text-xs" title={display}>
                    {display}
                  </span>
                </span>
              ) : (
                display || t.common.notSet
              )}
            </p>
          )}
        </div>

        {editing ? null : (
          <div className="col-start-2 row-start-1 flex shrink-0 flex-wrap items-center justify-end gap-2 sm:col-start-auto sm:row-start-auto">
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

/* ── Seasons and episodes ─────────────────────────────────────────────────── */

/**
 * The fields of one season and of each of its episodes, locked the same way
 * the work's own are.
 *
 * One season at a time, and one episode open at a time: a series of a
 * thousand episodes drawn as a thousand forms is not a page anybody can use.
 * Arriving from a season or an episode's public page opens straight onto it.
 */
function SeasonsEditor({
  work,
  registry,
  overrides,
  onChanged,
}: {
  work: MediaItem
  registry: FieldRegistry
  overrides: Override[]
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const [params] = useSearchParams()
  const numbers = seasonNumbers(work)

  // Absent is not zero: `Number(null)` opened every series with specials on
  // its specials.
  const asked = params.has('season') ? Number(params.get('season')) : Number.NaN
  const [season, setSeason] = useState<number>(
    numbers.includes(asked) ? asked : (numbers.find((n) => n > 0) ?? numbers[0] ?? 1),
  )
  const [open, setOpen] = useState<number | null>(
    numbers.includes(asked) && params.get('episode') ? Number(params.get('episode')) : null,
  )

  const episodes = episodesOf(work, season)
  const meta = work.seasons?.find((s) => s.seasonNumber === season)
  const lockOf = (scope: string, field: string) =>
    overrides.find((o) => o.scope === scope && o.field === field)
  const lockCount = (scope: string) => overrides.filter((o) => o.scope === scope).length

  return (
    <Panel id="season-fields" className="rise" style={{ animationDelay: '120ms' }}>
      <PanelHead
        title={t.admin.editor.seasons}
        action={
          <div className="flex items-center gap-2">
            {/* Out of sight on a phone, where the select says which season it
                is on its own — but still its name for a screen reader. */}
            <label htmlFor="editor-season" className="label sr-only sm:not-sr-only">
              {t.admin.editor.season}
            </label>
            <Select
              id="editor-season"
              value={String(season)}
              onChange={(event) => {
                setSeason(Number(event.target.value))
                setOpen(null)
              }}
              className="w-auto min-w-36"
            >
              {numbers.map((n) => (
                <option key={n} value={n}>
                  {seasonName(work.seasons?.find((s) => s.seasonNumber === n)?.title, n, t.work.season)}
                </option>
              ))}
            </Select>
          </div>
        }
      />

      {meta ? (
        <ul className="divide-y divide-rule border-b border-rule">
          {registry.season.map((def) => (
            <FieldRow
              key={`season:${season}:${def.name}`}
              itemId={work.id}
              scope={`season:${season}`}
              def={def}
              value={(meta as unknown as Record<string, unknown>)[def.name]}
              lock={lockOf(`season:${season}`, def.name)}
              onChanged={onChanged}
            />
          ))}
        </ul>
      ) : null}

      <ol className="divide-y divide-rule">
        {episodes.map((episode) => {
          const scope = `episode:${episode.seasonNumber}x${episode.episodeNumber}`
          const locks = lockCount(scope)
          const expanded = open === episode.episodeNumber

          return (
            <EpisodeFields
              key={episode.id}
              episode={episode}
              expanded={expanded}
              locks={locks}
              onToggle={() => setOpen(expanded ? null : episode.episodeNumber)}
              locale={locale}
            >
              <ul className="divide-y divide-rule border-t border-rule bg-ink/40">
                {registry.episode.map((def) => (
                  <FieldRow
                    key={`${scope}:${def.name}`}
                    itemId={work.id}
                    scope={scope}
                    def={def}
                    value={(episode as unknown as Record<string, unknown>)[def.name]}
                    lock={lockOf(scope, def.name)}
                    onChanged={onChanged}
                  />
                ))}
              </ul>
            </EpisodeFields>
          )
        })}
      </ol>
    </Panel>
  )
}

function EpisodeFields({
  episode,
  expanded,
  locks,
  onToggle,
  locale,
  children,
}: {
  episode: Episode
  expanded: boolean
  locks: number
  onToggle: () => void
  locale: string
  children: React.ReactNode
}) {
  const { t } = useI18n()
  const ref = useRef<HTMLLIElement>(null)

  // Opened from an episode's public page: bring it into view once.
  useEffect(() => {
    if (expanded) ref.current?.scrollIntoView({ block: 'nearest' })
    // Only on first open; later toggles are the reader's own doing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const panel = `episode-fields-${episode.seasonNumber}x${episode.episodeNumber}`

  return (
    <li ref={ref}>
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={panel}
        onClick={onToggle}
        className="flex w-full cursor-pointer items-center gap-3 px-5 py-3 text-left transition-colors duration-150 hover:bg-ink-high"
      >
        <Glyph name={expanded ? 'chevronDown' : 'chevronRight'} className="size-3.5 text-bone-faint" />
        <span className="font-mono text-xs text-bone-faint tabular-nums">{episodeCode(episode)}</span>
        <span className="min-w-0 flex-1 truncate text-sm text-bone">{episode.title || '—'}</span>
        <span className="hidden font-mono text-[0.6875rem] text-bone-faint tabular-nums sm:inline">
          {fmt.shortDate(episode.airDate, locale) ?? ''}
        </span>
        {locks ? (
          <Chip tone="manual">
            <Glyph name="lock" className="size-3" />
            {locks}
            <span className="sr-only"> {t.admin.editor.lockCount(locks)}</span>
          </Chip>
        ) : null}
      </button>
      {expanded ? <div id={panel}>{children}</div> : null}
    </li>
  )
}

/* ── Sources ──────────────────────────────────────────────────────────────── */

function Sources({
  itemId,
  snapshots,
  isManual,
}: {
  itemId: string
  snapshots?: Snapshot[]
  isManual?: boolean
}) {
  const { t, locale } = useI18n()
  const [viewing, setViewing] = useState<string | null>(null)

  const sorted = [...(snapshots ?? [])].sort((a, b) => b.fetchedAt.localeCompare(a.fetchedAt))

  return (
    <Panel id="sources" className="rise" style={{ animationDelay: '480ms' }}>
      <PanelHead title={t.work.sources} />

      {sorted.length === 0 ? (
        <p className="px-5 py-4 text-sm text-bone-faint">
          {isManual ? t.admin.editor.handEntered : t.admin.editor.noSources}
        </p>
      ) : (
        <dl className="divide-y divide-rule">
          {sorted.map((snapshot) => (
            <Field key={snapshot.provider} label={providerName(snapshot.provider)}>
              <span className="inline-flex items-center gap-2">
                <span title={fmt.dateTime(snapshot.fetchedAt, locale)}>
                  {fmt.relative(snapshot.fetchedAt, locale)}
                </span>
                <button
                  type="button"
                  onClick={() => setViewing(snapshot.provider)}
                  className="hit min-h-8 min-w-11 cursor-pointer rounded-card px-2 font-mono text-[0.6875rem] text-slate transition-colors duration-150 hover:bg-ink-high hover:text-bone"
                  aria-label={t.admin.editor.rawOf(providerName(snapshot.provider))}
                >
                  {'{ }'}
                </button>
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

      <RawSnapshot itemId={itemId} provider={viewing} onClose={() => setViewing(null)} />
    </Panel>
  )
}

/**
 * What one provider actually answered, verbatim.
 *
 * For the question the merged record cannot answer: did the provider say
 * this, or did this server get it wrong? Fetched only when asked for — a long
 * series' documents run to megabytes.
 */
function RawSnapshot({
  itemId,
  provider,
  onClose,
}: {
  itemId: string
  provider: string | null
  onClose: () => void
}) {
  const { t } = useI18n()
  const [copied, setCopied] = useState<'no' | 'yes' | 'failed'>('no')

  const raw = useQuery({
    queryKey: ['item', itemId, 'snapshot', provider],
    queryFn: () => api.get<Snapshot[]>(`/items/${itemId}/snapshots${query({ provider: provider ?? '' })}`),
    enabled: provider !== null,
    staleTime: 60_000,
  })

  // A provider's answer can run to megabytes: laid out once, not on every
  // render the copy button's state causes.
  const payload = raw.data?.[0]?.payload
  const text = useMemo(() => (payload === undefined ? '' : JSON.stringify(payload, null, 2)), [payload])

  const copy = async () => {
    try {
      // Absent altogether over plain HTTP, which is how a server on a home
      // network is usually reached: that is a refusal like any other.
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
        // A tab stop and a name: it always scrolls, and a keyboard had no way
        // into it otherwise.
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
    case 'dateTime': {
      // Typed by hand, in UTC: the seconds and the zone filled in, so that
      // "2009-03-22T21:00" is the instant the server expects.
      const partial = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2})(:\d{2})?$/.exec(trimmed)
      return partial ? `${partial[1]}${partial[2] ?? ':00'}Z` : trimmed
    }
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

/**
 * A field's name in the reader's language.
 *
 * The registry comes from the server, in English, so a French editor listed
 * "SORT TITLE" and "RUNTIME (MINUTES)". Its own label is kept as the fallback
 * for a field added to the server before it is added here.
 */
function fieldLabel(def: FieldDef, t: Dict): string {
  return (t.labels.fields as Record<string, string>)[def.name] ?? def.label
}
