/**
 * The import rules: what is brought into the catalogue on a schedule, and
 * into which selection — TMDB's most popular works of a recent stretch, or a
 * list another server publishes, followed.
 *
 * One row a rule: what it imports, where it puts it, when it runs next, how
 * its last run went, whether it is on; and the form that writes one, which
 * says what its schedule names and what its source would bring in now
 * before anything is kept. Each run is a run of the task `import.rules`,
 * on the tasks page.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useId, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  IconButton,
  Input,
  Panel,
  PanelHead,
  Segmented,
  Select,
  Skeleton,
  Spinner,
  Toggle,
} from '../../components/ui'
import { ApiError, api } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useSettled } from '../../lib/debounce'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { languageName } from '../../lib/labels'
import { TEXT_LANGUAGES } from '../../lib/prefs'
import type {
  CuratedLists,
  ImportPreview,
  ImportRule,
  ImportRuleRequest,
  MediaKind,
  ScheduleMoments,
} from '../../lib/types'
import { RunStatus, describeDetail } from './Jobs'

/** The stretches offered, in days: the server takes any from 1 to 3650. */
const STRETCHES = [7, 30, 90, 180, 365]

/** The common moments, as a cron expression each. */
const PRESETS: { id: string; cron: string }[] = [
  { id: 'nightly', cron: '0 3 * * *' },
  { id: 'mondays', cron: '0 5 * * 1' },
  { id: 'sundays', cron: '0 4 * * 0' },
  { id: 'monthly', cron: '0 4 1 * *' },
]

/** A rule as the form holds it: text where it types, choices where it picks. */
interface Form {
  name: string
  source: 'tmdb' | 'feed'
  kind: '' | MediaKind
  days: number
  limit: string
  originalLanguage: string
  url: string
  listId: string
  listMode: 'add' | 'replace'
  cron: string
  language: string
  enableWorks: boolean
  isEnabled: boolean
}

const BLANK: Form = {
  name: '',
  source: 'tmdb',
  kind: 'movie',
  days: 90,
  limit: '20',
  originalLanguage: '',
  url: '',
  listId: '',
  listMode: 'replace',
  cron: '0 5 * * 1',
  language: '',
  enableWorks: true,
  isEnabled: true,
}

function formOf(rule: ImportRule): Form {
  return {
    name: rule.name,
    source: rule.source,
    kind: rule.kind ?? '',
    days: rule.days ?? 90,
    limit: String(rule.limit ?? 20),
    originalLanguage: rule.originalLanguage ?? '',
    url: rule.url ?? '',
    listId: rule.listId ?? '',
    listMode: rule.listMode,
    cron: rule.cron,
    language: rule.language ?? '',
    enableWorks: rule.enableWorks,
    isEnabled: rule.isEnabled,
  }
}

function requestOf(form: Form): ImportRuleRequest {
  const tmdb = form.source === 'tmdb'
  return {
    name: form.name.trim(),
    source: form.source,
    kind: form.kind || undefined,
    days: tmdb ? form.days : undefined,
    limit: tmdb ? Number(form.limit) || undefined : undefined,
    originalLanguage: tmdb ? form.originalLanguage.trim() || undefined : undefined,
    url: tmdb ? undefined : form.url.trim(),
    listId: form.listId || undefined,
    listMode: form.listMode,
    cron: form.cron.trim(),
    language: form.language || undefined,
    enableWorks: form.enableWorks,
    isEnabled: form.isEnabled,
  }
}

/** The host a followed list is at, which is what a row says of it. */
function hostOf(url: string | undefined): string {
  if (!url) return ''
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

export function ImportRules() {
  const { t, locale } = useI18n()
  const r = t.admin.rules
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState<ImportRule | 'new' | null>(null)
  const [asking, setAsking] = useState<ImportRule | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const rules = useQuery({
    queryKey: ['import-rules'],
    queryFn: () => api.get<ImportRule[]>('/import/rules'),
    // Asked again while one of them runs, so its row says when it is done.
    refetchInterval: (query) => (query.state.data?.some((rule) => rule.lastRun?.status === 'running') ? 4_000 : false),
  })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['import-rules'] })
    void queryClient.invalidateQueries({ queryKey: ['tasks'] })
  }

  const toggle = useMutation({
    mutationFn: (rule: ImportRule) =>
      api.put<ImportRule>(`/import/rules/${encodeURIComponent(rule.id)}`, {
        ...requestOf(formOf(rule)),
        // Its address as it was shown: the key kept stays.
        url: rule.source === 'feed' ? rule.url : undefined,
        isEnabled: !rule.isEnabled,
      }),
    onSuccess: invalidate,
  })
  const run = useMutation({
    mutationFn: (rule: ImportRule) => api.post<{ jobId: string }>(`/import/rules/${encodeURIComponent(rule.id)}/run`),
    onMutate: () => setNotice(null),
    onSuccess: () => {
      setNotice(r.started)
      invalidate()
    },
  })
  const remove = useMutation({
    mutationFn: (rule: ImportRule) => api.delete(`/import/rules/${encodeURIComponent(rule.id)}`),
    onSuccess: () => {
      setAsking(null)
      invalidate()
    },
  })

  const describe = (rule: ImportRule) =>
    rule.source === 'tmdb'
      ? r.tmdbLine(
          rule.limit ?? 20,
          rule.kind === 'series' ? t.nav.series.toLowerCase() : t.nav.films.toLowerCase(),
          r.stretch[String(rule.days ?? 90)] ?? r.days(rule.days ?? 90),
        )
      : r.feedLine(hostOf(rule.url))

  return (
    <Panel id="import-rules" label={r.title} className="rise mb-6 overflow-hidden scroll-mt-24" style={{ animationDelay: '140ms' }}>
      <PanelHead
        title={r.title}
        action={
          <Button size="sm" variant="primary" onClick={() => setEditing('new')}>
            <Glyph name="plus" className="size-4" />
            {r.add}
          </Button>
        }
      />
      <p className="px-5 pt-3 text-sm leading-relaxed text-bone-dim">{r.lead}</p>
      <p role="status" className="px-5 pt-2 text-sm text-moss">
        {notice ?? ''}
      </p>
      {run.isError ? (
        <p role="alert" className="px-5 pt-2 text-sm text-vermillion">
          {run.error instanceof ApiError ? run.error.message : t.common.actionFailed}
        </p>
      ) : null}

      {rules.isPending ? (
        <div className="space-y-2 p-5">
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-12 w-full" />
        </div>
      ) : rules.isError ? (
        <p role="alert" className="flex items-center gap-2 px-5 py-6 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {r.loadFailed}
        </p>
      ) : rules.data.length === 0 ? (
        <EmptyState title={r.none} hint={r.noneHint} />
      ) : (
        <ul className="mt-2 divide-y divide-rule border-t border-rule">
          {rules.data.map((rule) => (
            <li key={rule.id} data-rule={rule.id} className="grid gap-3 px-5 py-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
              <div className="min-w-0 space-y-1">
                <p className="flex flex-wrap items-center gap-2">
                  <span className={cn('text-sm font-medium', rule.isEnabled ? 'text-bone' : 'text-bone-faint')}>{rule.name}</span>
                  {rule.isEnabled ? null : <Chip>{r.off}</Chip>}
                </p>
                <p className="text-sm text-bone-dim">
                  {describe(rule)}
                  {rule.listName ? (
                    <>
                      {' → '}
                      <span className="text-bone">{rule.listName}</span>
                      <span className="text-bone-faint"> · {r.placements[rule.listMode]}</span>
                    </>
                  ) : null}
                </p>
                <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-bone-faint">
                  <span className="font-mono">{rule.cron} UTC</span>
                  {rule.nextAt ? (
                    <span title={fmt.dateTime(rule.nextAt, locale)}>{r.next(fmt.relative(rule.nextAt, locale) ?? '')}</span>
                  ) : null}
                  {rule.lastRun ? (
                    <span className="flex items-center gap-2">
                      <RunStatus status={rule.lastRun.status} />
                      <span title={fmt.dateTime(rule.lastRun.createdAt, locale)}>{fmt.relative(rule.lastRun.createdAt, locale)}</span>
                      <span className="text-bone-dim">{describeDetail(rule.lastRun.error ?? rule.lastRun.detail, t, locale)}</span>
                    </span>
                  ) : (
                    <span>{r.never}</span>
                  )}
                </p>
              </div>
              <div className="flex flex-wrap items-center gap-1">
                <Toggle
                  checked={rule.isEnabled}
                  label={`${r.isEnabled}: ${rule.name}`}
                  disabled={toggle.isPending}
                  onChange={() => toggle.mutate(rule)}
                />
                <Button size="sm" disabled={run.isPending} aria-label={`${r.runNow}: ${rule.name}`} onClick={() => run.mutate(rule)}>
                  {run.isPending && run.variables?.id === rule.id ? <Spinner className="size-4" /> : <Glyph name="play" className="size-4" />}
                  {r.runNow}
                </Button>
                <IconButton glyph="pencil" label={r.edit(rule.name)} onClick={() => setEditing(rule)} />
                <IconButton glyph="trash" tone="danger" label={`${r.delete}: ${rule.name}`} onClick={() => setAsking(rule)} />
              </div>
            </li>
          ))}
        </ul>
      )}
      <p className="border-t border-rule px-5 py-3 text-xs text-bone-faint">
        {r.runsOn}{' '}
        <Link to="/admin/jobs#task-import.rules" className="text-vermillion underline-offset-4 hover:underline">
          {t.admin.jobs}
        </Link>
      </p>

      {editing ? (
        <RuleForm
          key={editing === 'new' ? 'new' : editing.id}
          rule={editing === 'new' ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            invalidate()
          }}
        />
      ) : null}

      <Dialog
        open={asking !== null}
        title={r.deleteTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={remove.isPending} onClick={() => asking && remove.mutate(asking)}>
              <Glyph name="trash" className="size-4" />
              {r.delete}
            </Button>
          </>
        }
      >
        {asking ? r.deleteBody(asking.name) : null}
        {remove.isError ? (
          <p role="alert" className="mt-3 text-sm text-vermillion">
            {remove.error.message}
          </p>
        ) : null}
      </Dialog>
    </Panel>
  )
}

/* ── The form ─────────────────────────────────────────────────────────────── */

function RuleForm({ rule, onClose, onSaved }: { rule: ImportRule | null; onClose: () => void; onSaved: () => void }) {
  const { t, locale } = useI18n()
  const r = t.admin.rules
  const ids = useId()
  const [form, setForm] = useState<Form>(rule ? formOf(rule) : BLANK)
  const set = <K extends keyof Form>(key: K, value: Form[K]) => setForm((now) => ({ ...now, [key]: value }))
  const [preview, setPreview] = useState<ImportPreview | null>(null)

  const selections = useQuery({
    queryKey: ['admin-lists'],
    queryFn: () => api.get<CuratedLists>('/lists'),
  })
  const manual = (selections.data?.lists ?? []).filter((list) => list.mode === 'manual')

  const cron = useSettled(form.cron.trim(), 350)
  const moments = useQuery({
    queryKey: ['schedule-preview', { kind: 'cron', cron }],
    queryFn: () => api.post<ScheduleMoments>('/tasks/schedules/preview', { kind: 'cron', cron }),
    enabled: cron.length > 0,
    retry: false,
    staleTime: 30_000,
  })

  const save = useMutation({
    mutationFn: () =>
      rule
        ? api.put<ImportRule>(`/import/rules/${encodeURIComponent(rule.id)}`, requestOf(form))
        : api.post<ImportRule>('/import/rules', requestOf(form)),
    onSuccess: onSaved,
  })
  const look = useMutation({
    mutationFn: () =>
      api.post<ImportPreview>(
        `/import/rules/preview${rule ? `?rule=${encodeURIComponent(rule.id)}` : ''}`,
        requestOf(form),
      ),
    onSuccess: setPreview,
  })

  return (
    <Dialog
      open
      size="lg"
      title={rule ? r.edit(rule.name) : r.create}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t.common.cancel}</Button>
          <Button variant="primary" disabled={save.isPending || !form.name.trim()} onClick={() => save.mutate()}>
            {save.isPending ? <Spinner className="size-4" /> : <Glyph name="check" className="size-4" />}
            {save.isPending ? r.saving : r.save}
          </Button>
        </>
      }
    >
      <form
        className="space-y-5"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate()
        }}
      >
        <FormField label={r.name} htmlFor={`${ids}-name`}>
          <Input id={`${ids}-name`} required maxLength={120} value={form.name} onChange={(event) => set('name', event.target.value)} />
        </FormField>

        <div className="space-y-2">
          <p className="label">{r.source}</p>
          <Segmented<'tmdb' | 'feed'>
            label={r.source}
            value={form.source}
            onChange={(next) => set('source', next)}
            options={[
              { value: 'tmdb', label: r.sources.tmdb },
              { value: 'feed', label: r.sources.feed },
            ]}
          />
        </div>

        {form.source === 'tmdb' ? (
          <div className="grid gap-4 sm:grid-cols-2">
            <FormField label={r.kind} htmlFor={`${ids}-kind`}>
              <Select id={`${ids}-kind`} value={form.kind || 'movie'} onChange={(event) => set('kind', event.target.value as MediaKind)}>
                <option value="movie">{t.nav.films}</option>
                <option value="series">{t.nav.series}</option>
              </Select>
            </FormField>
            <FormField label={r.stretchLabel} htmlFor={`${ids}-days`}>
              <Select id={`${ids}-days`} value={String(form.days)} onChange={(event) => set('days', Number(event.target.value))}>
                {(STRETCHES.includes(form.days) ? STRETCHES : [...STRETCHES, form.days].sort((a, b) => a - b)).map((days) => (
                  <option key={days} value={days}>
                    {r.stretch[String(days)] ?? r.days(days)}
                  </option>
                ))}
              </Select>
            </FormField>
            <FormField label={r.limit} htmlFor={`${ids}-limit`} hint={r.limitHint}>
              <Input
                id={`${ids}-limit`}
                type="number"
                inputMode="numeric"
                min={1}
                max={500}
                value={form.limit}
                onChange={(event) => set('limit', event.target.value)}
              />
            </FormField>
            <FormField label={r.originalLanguage} htmlFor={`${ids}-original`} hint={r.originalLanguageHint}>
              <Select id={`${ids}-original`} value={form.originalLanguage} onChange={(event) => set('originalLanguage', event.target.value)}>
                <option value="">{r.anyLanguage}</option>
                {TEXT_LANGUAGES.filter((code) => !code.includes('-')).map((code) => (
                  <option key={code} value={code}>
                    {languageName(code, locale) ?? code}
                  </option>
                ))}
              </Select>
            </FormField>
          </div>
        ) : (
          <div className="grid gap-4">
            <FormField label={r.url} htmlFor={`${ids}-url`} hint={r.urlHint}>
              <Input
                id={`${ids}-url`}
                type="url"
                inputMode="url"
                spellCheck={false}
                className="font-mono text-xs"
                value={form.url}
                onChange={(event) => set('url', event.target.value)}
              />
            </FormField>
            {rule?.url?.includes('=hidden') ? <p className="text-xs text-bone-faint">{r.keyKept}</p> : null}
            <div className="max-w-sm">
              <FormField label={r.feedKind} htmlFor={`${ids}-feed-kind`} hint={r.feedKindHint}>
                <Select id={`${ids}-feed-kind`} value={form.kind} onChange={(event) => set('kind', event.target.value as '' | MediaKind)}>
                  <option value="">{r.notSaid}</option>
                  <option value="movie">{t.nav.films}</option>
                  <option value="series">{t.nav.series}</option>
                </Select>
              </FormField>
            </div>
          </div>
        )}

        <div className="grid gap-4 sm:grid-cols-2">
          <FormField label={r.list} htmlFor={`${ids}-list`} hint={manual.length ? undefined : r.noLists}>
            <Select id={`${ids}-list`} value={form.listId} onChange={(event) => set('listId', event.target.value)}>
              <option value="">{r.noList}</option>
              {manual.map((list) => (
                <option key={list.id} value={list.id}>
                  {list.name}
                </option>
              ))}
            </Select>
          </FormField>
          {form.listId ? (
            <div className="space-y-1.5">
              <p className="label">{r.placement}</p>
              <Segmented<'add' | 'replace'>
                label={r.placement}
                value={form.listMode}
                onChange={(next) => set('listMode', next)}
                options={[
                  { value: 'replace', label: r.placements.replace },
                  { value: 'add', label: r.placements.add },
                ]}
              />
              <p className="text-xs text-bone-faint">{r.placementHints[form.listMode]}</p>
            </div>
          ) : null}
        </div>

        <div className="space-y-3">
          <FormField label={r.when} htmlFor={`${ids}-cron`} hint={t.admin.tasks.when.expressionHint}>
            <Input
              id={`${ids}-cron`}
              className="font-mono"
              spellCheck={false}
              autoComplete="off"
              value={form.cron}
              onChange={(event) => set('cron', event.target.value)}
            />
          </FormField>
          <div className="flex flex-wrap gap-2">
            {PRESETS.map((preset) => (
              <Button key={preset.id} size="sm" variant="quiet" aria-pressed={form.cron === preset.cron} onClick={() => set('cron', preset.cron)}>
                {t.admin.tasks.when.preset[preset.id]}
              </Button>
            ))}
          </div>
          <div aria-live="polite" className="text-sm">
            {moments.isError ? (
              <p role="alert" className="text-vermillion">
                {moments.error instanceof ApiError ? moments.error.message : t.common.error}
              </p>
            ) : moments.data ? (
              <p className="text-bone-dim">
                {r.nextRuns(moments.data.next.map((at) => fmt.dateTime(at, locale) ?? at).join(' · '))}
              </p>
            ) : null}
          </div>
        </div>

        <div className="max-w-sm">
          <FormField label={t.admin.wizard.language} htmlFor={`${ids}-language`} hint={t.admin.wizard.languageHint}>
            <Select id={`${ids}-language`} value={form.language} onChange={(event) => set('language', event.target.value)}>
              <option value="">{t.admin.wizard.catalogueOnly}</option>
              {TEXT_LANGUAGES.map((code) => (
                <option key={code} value={code}>
                  {languageName(code, locale) ?? code}
                </option>
              ))}
            </Select>
          </FormField>
        </div>

        <ul className="divide-y divide-rule rounded-card border border-rule">
          {(
            [
              ['enableWorks', t.admin.wizard.enabled, t.admin.wizard.enabledHint],
              ['isEnabled', r.isEnabled, r.isEnabledHint],
            ] as const
          ).map(([key, label, hint]) => (
            <li key={key} className="flex items-start gap-4 px-4 py-3">
              <div className="min-w-0 flex-1">
                <label htmlFor={`${ids}-${key}`} className="block text-sm text-bone">
                  {label}
                </label>
                <p className="mt-0.5 text-xs leading-relaxed text-bone-faint">{hint}</p>
              </div>
              <Toggle id={`${ids}-${key}`} checked={form[key]} label={label} onChange={(next) => set(key, next)} />
            </li>
          ))}
        </ul>

        <div className="space-y-3 border-t border-rule pt-4">
          <Button disabled={look.isPending} onClick={() => look.mutate()}>
            {look.isPending ? <Spinner className="size-4" /> : <Glyph name="search" className="size-4" />}
            {look.isPending ? r.previewing : r.previewNow}
          </Button>
          {look.isError ? (
            <p role="alert" className="text-sm text-vermillion">
              {look.error instanceof ApiError ? look.error.message : t.common.error}
            </p>
          ) : null}
          {preview ? (
            <div role="status" className="space-y-2">
              <p className="text-sm text-bone">
                {r.previewSays(preview.lines.length, preview.counts.candidate, preview.counts.held)}
              </p>
              <ol className="max-h-56 divide-y divide-rule overflow-y-auto rounded-card border border-rule text-sm">
                {preview.lines.slice(0, 50).map((line) => (
                  <li key={line.line} className="flex items-center justify-between gap-3 px-3 py-1.5">
                    <span className="min-w-0 truncate text-bone">
                      {line.work?.title ?? line.title ?? <span className="font-mono text-xs text-bone-dim">{line.text}</span>}
                      {line.year ? <span className="ml-1.5 font-mono text-xs text-bone-faint">{line.year}</span> : null}
                    </span>
                    <Chip tone={line.status === 'candidate' ? 'provider' : line.status === 'held' ? 'neutral' : 'accent'}>
                      {t.admin.wizard.counts[line.status]}
                    </Chip>
                  </li>
                ))}
              </ol>
            </div>
          ) : null}
        </div>

        {save.isError ? (
          <p role="alert" className="text-sm text-vermillion">
            {save.error instanceof ApiError ? save.error.message : t.common.actionFailed}
          </p>
        ) : null}
      </form>
    </Dialog>
  )
}
