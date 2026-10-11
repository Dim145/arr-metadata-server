/**
 * What this server does, and how it was started.
 *
 * Two kinds of fact, and the page is ordered by which one an operator came for.
 * The settings come first and are live: the server keeps them in a table it
 * re-reads on every request, so a change applies to the next one. Below them
 * sit the things that really were read once from the environment — the version,
 * the database, whether a TMDB key exists, what stands at each door — and those
 * stay read-only, because a form that appeared to change them would be lying.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  ButtonLink,
  Chip,
  Field,
  Glyph,
  Input,
  Label,
  OnThisPage,
  Panel,
  PanelHead,
  Skeleton,
  Spinner,
} from '../../components/ui'
import { type GroupId, ServerSettings, groupOf, useRegistry } from '../../components/ScopeSettings'
import { ApiError, api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useI18n, type Dict } from '../../lib/i18n'
import type {
  EffectiveSetting,
  ExportStarted,
  JobsResponse,
  Settings as Config,
  WebhookEvents,
  WebhookTest,
} from '../../lib/types'
import { PolicyChip } from './Dashboard'

export function Settings() {
  const { t } = useI18n()

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Config>('/settings'),
    staleTime: 5 * 60_000,
  })
  const registry = useRegistry()

  if (settings.isPending) {
    return (
      <div className="mx-auto max-w-4xl space-y-6">
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (settings.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {t.admin.config.loadFailed}
      </p>
    )
  }

  const config = settings.data
  // The groups the server's settings fall in, for the index: the same
  // grouping the panels below are drawn by.
  const groups: GroupId[] = (['answering', 'providers', 'sources', 'refresh', 'adult', 'other'] as GroupId[]).filter(
    (group) => (registry.data ?? []).some((def) => def.scopes.includes('server') && groupOf(def.key) === group),
  )

  return (
    <div className="mx-auto max-w-4xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {t.admin.settings}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
          {t.admin.config.lead}
        </p>
      </header>

      {config.authDisabled ? (
        <p
          role="alert"
          className="rise mb-6 flex items-start gap-3 rounded-panel border border-vermillion-deep bg-vermillion/[0.06] px-5 py-4 text-sm leading-relaxed text-bone"
        >
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-vermillion" />
          <span>
            <strong className="font-medium">{t.admin.config.authOffTitle}</strong>{' '}
            {t.admin.config.authOffBody}
          </span>
        </p>
      ) : null}

      <OnThisPage
        label={t.admin.inPage}
        entries={[
          ...groups.map((group) => ({ id: `settings-${group}`, label: t.settings.groups[group] })),
          { id: 'settings-webhooks', label: t.settings.groups.webhooks },
          { id: 'settings-runtime', label: t.admin.config.runtime },
          { id: 'settings-policy', label: t.admin.config.policy },
          { id: 'settings-nfo', label: t.admin.config.export },
          { id: 'settings-docs', label: t.admin.config.docs },
          { id: 'settings-maintenance', label: t.admin.config.maintenance },
          { id: 'settings-password', label: t.admin.config.password },
        ]}
      />

      <ServerSettings delay={40} />

      <Webhooks delay={240} />

      <div className="mt-6 grid gap-6 lg:grid-cols-2">
        <Panel id="settings-runtime" className="rise" style={{ animationDelay: '260ms' }}>
          <PanelHead title={t.admin.config.runtime} />
          <dl className="divide-y divide-rule">
            <Field label={t.admin.config.version}>
              <span className="font-mono text-[0.8125rem] tabular-nums">{config.version}</span>
            </Field>
            <Field label={t.admin.config.database}>
              <span className="font-mono text-[0.8125rem]">{config.database}</span>
            </Field>
            <Field label={t.admin.config.publicUrl}>
              <span className="font-mono text-[0.8125rem] break-all">
                {config.publicUrl ?? '—'}
              </span>
            </Field>
            <Field label={t.admin.config.tmdb}>
              {config.tmdbConfigured ? (
                <Chip>
                  <Glyph name="check" className="size-3" />
                  {t.admin.config.tmdbReady}
                </Chip>
              ) : (
                <Chip tone="accent">
                  <Glyph name="alert" className="size-3" />
                  {t.admin.config.tmdbMissing}
                </Chip>
              )}
            </Field>
          </dl>
          <p className="border-t border-rule px-5 py-4 text-xs leading-relaxed text-bone-faint">
            {t.admin.config.runtimeHint}
          </p>
        </Panel>

        <Panel id="settings-policy" className="rise" style={{ animationDelay: '300ms' }}>
          <PanelHead title={t.admin.config.policy} />
          <dl className="divide-y divide-rule">
            {(
              [
                ['/api/v1/*', config.nativePolicy],
                ['/3/*', config.tmdbPolicy],
                ['/v4/*', config.tvdbPolicy],
                ['graphql.anilist.co', config.anilistPolicy],
                ['/v1/*', config.arrPolicy],
              ] as const
            ).map(([path, policy]) => (
              <div key={path} className="flex items-center justify-between gap-4 px-5 py-2.5">
                <dt className="font-mono text-[0.8125rem] text-bone">{path}</dt>
                <dd>
                  <PolicyChip policy={policy} />
                </dd>
              </div>
            ))}
          </dl>
          <p className="border-t border-rule px-5 py-4 text-xs leading-relaxed text-bone-faint">
            {t.admin.config.policyHint}
          </p>
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-rule px-5 py-3 text-sm text-bone-dim">
            {t.admin.config.accessMoved}
            <Link
              to="/admin/access"
              className="inline-flex min-h-11 items-center gap-1.5 text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
            >
              {t.admin.accessPage}
              <Glyph name="chevronRight" className="size-3.5" />
            </Link>
          </p>
        </Panel>
      </div>

      <Panel id="settings-nfo" className="rise mt-6" style={{ animationDelay: '340ms' }}>
        <PanelHead
          title={t.admin.config.export}
          action={
            <Link
              to="/admin/jobs"
              className="label -my-2 inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
            >
              {t.admin.jobs}
            </Link>
          }
        />
        <NfoExport />
      </Panel>

      <Panel id="settings-docs" className="rise mt-6" style={{ animationDelay: '380ms' }}>
        <PanelHead title={t.admin.config.docs} />
        <div className="flex flex-wrap items-center justify-between gap-4 p-5">
          <div className="min-w-0">
            <p className="text-sm text-bone">{t.admin.config.docsTitle}</p>
            <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">
              {t.admin.config.docsBody}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            {/* A way somewhere else, not the action this page asks for: the
                gradient is kept for one button a screen. */}
            <ButtonLink href="/api/docs" target="_blank" rel="noreferrer" variant="ghost">
              <Glyph name="external" className="size-4" />
              {t.admin.config.open}
            </ButtonLink>
            <ButtonLink href="/api/openapi.json" target="_blank" rel="noreferrer">
              openapi.json
            </ButtonLink>
          </div>
        </div>
      </Panel>

      <Panel id="settings-maintenance" className="rise mt-6" style={{ animationDelay: '420ms' }}>
        <PanelHead title={t.admin.config.maintenance} />
        <ClearCache />
      </Panel>

      <Panel id="settings-password" className="rise mt-6 mb-4" style={{ animationDelay: '460ms' }}>
        <PanelHead title={t.admin.config.password} />
        <p className="flex flex-wrap items-center gap-x-3 gap-y-2 p-5 text-sm text-bone-dim">
          {t.admin.config.passwordMoved}
          <Link
            to="/admin/account"
            className="inline-flex min-h-11 items-center gap-1.5 text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
          >
            {t.admin.account}
            <Glyph name="chevronRight" className="size-3.5" />
          </Link>
        </p>
      </Panel>
    </div>
  )
}

/** Write a `.nfo` document for everything, for a library Plex reads. */
/** `N works, N episodes, N failed`, as the server words an export's run. */
function exportCounts(detail?: string) {
  const match = detail?.match(/^(\d+) works, (\d+) episodes, (\d+) failed$/)
  return match ? { works: Number(match[1]), episodes: Number(match[2]), failed: Number(match[3]) } : undefined
}

/**
 * The export, written in the background: a library's artwork is thousands of
 * downloads. A run seen during this visit is followed until it ends — the one
 * started here, or one another tab started — and what it wrote is read off its
 * record. One that ended before the visit is the jobs page's to tell.
 */
function NfoExport() {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const run = useMutation({
    mutationFn: () => api.post<ExportStarted>('/export/nfo'),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['jobs'] })
    },
  })

  const latest = useQuery({
    queryKey: ['jobs', 'export.nfo'],
    queryFn: () => api.get<JobsResponse>(`/jobs${query({ kind: 'export.nfo', limit: 1 })}`),
    refetchInterval: (state) => (state.state.data?.jobs[0]?.status === 'running' ? 3000 : false),
    // Asked again on coming back to the tab, whatever the app's default: an
    // export another tab started is otherwise never seen here.
    refetchOnWindowFocus: 'always',
  })
  const job = latest.data?.jobs[0]
  const [followed, setFollowed] = useState<string>()
  // When this page was opened: a run started after it is one of this visit's,
  // even when it has ended by the time this tab asks.
  const [opened] = useState(() => Date.now())
  const started = run.data?.jobId
  useEffect(() => {
    if (started) setFollowed(started)
    else if (job && (job.status === 'running' || Date.parse(job.createdAt) >= opened)) setFollowed(job.id)
  }, [started, job, opened])
  const mine = job && job.id === followed ? job : undefined
  const counts = mine?.status === 'succeeded' ? exportCounts(mine.detail) : undefined
  const writing = run.isPending || job?.status === 'running'

  // The code alone. On this route `provider_not_configured` has exactly one
  // cause — no export path is set — and matching on the message text as well
  // only ever mislabelled something else as that.
  const missingPath =
    run.error instanceof ApiError &&
    (run.error.code === 'export_not_configured' || run.error.code === 'provider_not_configured')

  return (
    <div className="flex flex-wrap items-center justify-between gap-4 p-5">
      <div className="min-w-0">
        <p className="text-sm text-bone">{t.admin.config.exportTitle}</p>
        <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">
          {t.admin.config.exportBody}
        </p>

        {writing ? (
          <p role="status" className="mt-2 max-w-prose text-xs leading-relaxed text-bone-dim">
            {t.admin.config.exportBackground}{' '}
            <Link to="/admin/jobs" className="text-vermillion underline-offset-2 hover:underline">
              {t.admin.config.exportJobs}
            </Link>
          </p>
        ) : counts ? (
          <p
            role="status"
            className={cn(
              'mt-2 font-mono text-xs',
              // Green says "this worked". A run that wrote nothing and failed on
              // every work did not, whatever status code carried the summary.
              counts.works === 0 && counts.failed > 0 ? 'text-vermillion' : 'text-moss',
            )}
          >
            {t.admin.config.exportDone(counts.works, counts.episodes)}
            {counts.failed > 0 ? ` · ${t.admin.config.exportFailed(counts.failed)}` : ''}
            {run.data ? (
              <span className="mt-0.5 block break-all text-bone-faint">{run.data.root}</span>
            ) : null}
          </p>
        ) : mine?.status === 'failed' ? (
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {mine.error}
          </p>
        ) : null}

        {run.isError ? (
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {missingPath ? t.admin.config.exportNoPath : run.error.message}
          </p>
        ) : null}
      </div>

      <Button variant="primary" onClick={() => run.mutate()} disabled={writing}>
        {writing ? <Spinner className="size-4" /> : <Glyph name="download" className="size-4" />}
        {writing ? t.admin.config.exportRunning : t.admin.config.exportRun}
      </Button>
    </div>
  )
}

function ClearCache() {
  const { t } = useI18n()
  const clear = useMutation({ mutationFn: () => api.post('/cache/clear') })

  return (
    <div className="flex flex-wrap items-center justify-between gap-4 p-5">
      <div className="min-w-0">
        <p className="text-sm text-bone">{t.admin.config.cacheTitle}</p>
        <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">
          {t.admin.config.cacheBody}
        </p>

        {/* Without this the button simply goes back to saying "Clear", which
            reads as "done" — the one thing it must not say when it is not. */}
        {clear.isError ? (
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {clear.error.message}
          </p>
        ) : null}
      </div>
      <Button onClick={() => clear.mutate()} disabled={clear.isPending}>
        {clear.isPending ? <Spinner className="size-4" /> : null}
        {clear.isSuccess ? t.admin.config.cacheCleared : t.admin.config.cacheClear}
      </Button>
    </div>
  )
}

/* ── The webhook ──────────────────────────────────────────────────────────── */

/** The server's own settings, as the rows above read them: one query, shared. */
const SERVER_VALUES = ['settings', 'scope', 'server', '-'] as const

/** `*`: every event, those the server grows later included. */
const EVERY = '*'

/** The names a stored list holds, as written: trimmed, the empty ones dropped. */
function namesIn(value: string): string[] {
  return value
    .split(',')
    .map((name) => name.trim())
    .filter(Boolean)
}

/**
 * One server setting, written as it changes; the rows above refetch with it.
 * What the write answers — the server's settings as they stand after it — is
 * taken at once: a box ticked before the refetch lands would otherwise be
 * worked out from the list as it was, and undo the one ticked just before.
 * The boxes wait while a write is under way, so no two answers race.
 */
function useServerSetting(key: string) {
  const queryClient = useQueryClient()
  const save = useMutation({
    mutationFn: (value: string) => api.put<EffectiveSetting[]>('/settings/server/-', { key, value }),
    onSuccess: (written) => {
      queryClient.setQueryData([...SERVER_VALUES], written)
      void queryClient.invalidateQueries({ queryKey: [...SERVER_VALUES] })
    },
  })
  // The server answers `{key} {why}`; the key is the field's own name here.
  const message = save.error instanceof ApiError ? save.error.message : save.error?.message
  const error = message?.startsWith(`${key} `) ? message.slice(key.length + 1) : message
  return { save, error }
}

/**
 * Where what happens is posted, and which of it.
 *
 * The events are ticked rather than typed: the server names every one it can
 * tell of — the trail's actions and the facts nobody did — and the setting
 * keeps them as it always has, as names separated by commas. A name it holds
 * that this server does not know is kept as it is, and said so. The test
 * goes to the address the server holds, so it waits for the field to be
 * saved.
 */
function Webhooks({ delay }: { delay: number }) {
  const { t } = useI18n()
  const w = t.admin.config.webhooks

  const values = useQuery({
    queryKey: [...SERVER_VALUES],
    queryFn: () => api.get<EffectiveSetting[]>('/settings/server/-'),
  })
  const known = useQuery({
    queryKey: ['webhooks', 'events'],
    queryFn: () => api.get<WebhookEvents>('/admin/webhooks/events'),
    // What the server can tell of only changes when the binary does.
    staleTime: 60 * 60_000,
  })

  const value = (key: string) => values.data?.find((setting) => setting.key === key)?.value

  return (
    <Panel
      id="settings-webhooks"
      label={t.settings.groups.webhooks}
      className="rise mt-6"
      style={{ animationDelay: `${delay}ms` }}
    >
      <PanelHead title={t.settings.groups.webhooks} />
      <p className="border-b border-rule px-5 py-4 text-sm leading-relaxed text-bone-dim">{w.lead}</p>
      {values.isPending || known.isPending ? (
        <div className="space-y-3 p-5">
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-40 w-full" />
        </div>
      ) : values.isError || known.isError ? (
        <p role="alert" className="flex items-center gap-2 px-5 py-4 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {values.isError ? t.settings.loadFailed : w.loadFailed}
        </p>
      ) : (
        <>
          <WebhookAddress saved={value('webhooks.url') ?? ''} />
          <WebhookEventList
            value={value('webhooks.events') ?? known.data.defaults.join(',')}
            known={known.data}
          />
        </>
      )}
    </Panel>
  )
}

/** What a test came to, in a sentence, and whether it went well. */
function tested(result: WebhookTest, w: Dict['admin']['config']['webhooks']) {
  if (result.refused === 'not_web') return { fine: false, text: w.notWeb }
  if (result.refused === 'internal') return { fine: false, text: w.internal }
  if (result.status !== undefined) {
    return result.delivered
      ? { fine: true, text: w.delivered(result.status, result.millis) }
      : { fine: false, text: w.refused(result.status, result.millis) }
  }
  return { fine: false, text: w.unreached(result.millis) }
}

/** The address, saved as the field is left, and the test of it. */
function WebhookAddress({ saved }: { saved: string }) {
  const { t } = useI18n()
  const w = t.admin.config.webhooks
  const key = 'webhooks.url'
  const id = 'setting-server-webhooks-url'
  const names: Record<string, string | undefined> = t.settings.names
  const hints: Record<string, string | undefined> = t.settings.hints

  const [draft, setDraft] = useState(saved)
  // The stored value moves under the field when a write is refused, or when
  // another tab saved one.
  useEffect(() => setDraft(saved), [saved])

  const { save, error } = useServerSetting(key)
  const test = useMutation({ mutationFn: () => api.post<WebhookTest>('/admin/webhooks/test') })

  const unsaved = draft.trim() !== saved
  const outcome = test.data ? tested(test.data, w) : undefined

  return (
    <div className="border-b border-rule px-5 py-4">
      <label htmlFor={id} className="block text-sm font-medium text-bone">
        {names[key] ?? key}
      </label>
      <p className="mt-0.5 font-mono text-[0.6875rem] text-bone-faint">{key}</p>
      {hints[key] ? <p className="mt-1.5 max-w-prose text-xs leading-relaxed text-bone-faint">{hints[key]}</p> : null}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Input
          id={id}
          type="url"
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          className="min-w-0 flex-1 basis-64"
          value={draft}
          disabled={save.isPending}
          onChange={(event) => {
            setDraft(event.target.value)
            if (save.isSuccess || save.isError) save.reset()
            if (test.data || test.isError) test.reset()
          }}
          onBlur={() => {
            if (draft.trim() !== saved) save.mutate(draft.trim())
          }}
          // Enter is how a form is submitted, and this field is not in one.
          onKeyDown={(event) => {
            if (event.key === 'Enter') event.currentTarget.blur()
          }}
        />
        <Button
          onClick={() => test.mutate()}
          disabled={!saved || unsaved || save.isPending || test.isPending}
          aria-describedby={`${id}-test`}
        >
          {test.isPending ? <Spinner className="size-4" /> : <Glyph name="share" className="size-4" />}
          {test.isPending ? w.testing : w.test}
        </Button>
        {save.isPending ? <Spinner className="size-3.5 text-bone-faint" /> : null}
        {save.isSuccess ? (
          <span className="flex items-center gap-1 text-xs text-moss">
            <Glyph name="check" className="size-3.5" />
            {t.settings.saved}
          </span>
        ) : null}
      </div>

      <p id={`${id}-test`} className="mt-2 max-w-prose text-xs leading-relaxed text-bone-faint">
        {saved && unsaved ? w.saveFirst : w.testHint}
      </p>
      {/* Always there, so what appears in it is read out. */}
      <p role="status" className={cn('mt-1 text-xs', outcome?.fine ? 'text-moss' : 'text-vermillion')}>
        {outcome ? outcome.text : ''}
        {test.data?.error ? (
          <span className="mt-0.5 block font-mono break-words text-bone-faint">{test.data.error}</span>
        ) : null}
      </p>
      {test.isError ? (
        <p role="alert" className="mt-1 text-xs text-vermillion">
          {test.error.message}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="mt-2 flex items-start gap-1.5 text-xs text-vermillion">
          <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
          {error}
        </p>
      ) : null}
    </div>
  )
}

/**
 * The events, ticked: `*` above the rest, the facts, then the trail's actions
 * folded away — forty-odd of them. A box changed is saved at once, as a switch
 * is; while a write is under way the boxes wait, so two changes never race.
 */
function WebhookEventList({ value, known }: { value: string; known: WebhookEvents }) {
  const { t } = useI18n()
  const w = t.admin.config.webhooks
  const key = 'webhooks.events'
  const says: Record<string, string | undefined> = t.admin.trail.says
  const hints: Record<string, string | undefined> = t.settings.hints

  const { save, error } = useServerSetting(key)

  const names = namesIn(value)
  const offered = [...known.facts, ...known.actions]
  const unknown = names.filter((name) => name !== EVERY && !offered.includes(name))
  // What was just ticked is shown at once: the boxes follow the stored
  // value, which only comes back with the write, and a tick that waited for
  // it read as a tick that did not take.
  const [pending, setPending] = useState<Set<string> | null>(null)
  const ticked = pending ?? new Set(names)
  const every = ticked.has(EVERY)
  const actionsTicked = known.actions.filter((name) => every || ticked.has(name)).length

  // In the order they are offered; a name this server does not know is kept
  // where it was, at the end.
  const settled = { onSettled: () => setPending(null) }
  const write = (next: Set<string>) => {
    setPending(next)
    save.mutate([...offered.filter((name) => next.has(name)), ...unknown].join(','), settled)
  }
  const toggle = (name: string, on: boolean) => {
    const next = new Set(ticked)
    if (on) next.add(name)
    else next.delete(name)
    write(next)
  }
  // Every event off is every one named: what was posted goes on being posted,
  // and each can then be unticked.
  const toggleEvery = (on: boolean) => {
    if (!on) {
      write(new Set(offered))
      return
    }
    setPending(new Set([EVERY]))
    save.mutate(EVERY, settled)
  }

  const box = (name: string, label: string, hint?: string) => (
    <li key={name}>
      <label className="flex cursor-pointer items-start gap-3 py-1.5 has-[:disabled]:cursor-not-allowed">
        <input
          type="checkbox"
          className="mt-1 size-4 shrink-0 cursor-pointer accent-vermillion disabled:cursor-not-allowed disabled:opacity-60"
          checked={every || ticked.has(name)}
          disabled={every || save.isPending}
          onChange={(event) => toggle(name, event.target.checked)}
        />
        <span className="min-w-0">
          <span className="block text-sm text-bone">{label}</span>
          <span className="block font-mono text-[0.6875rem] text-bone-faint">{name}</span>
          {hint ? <span className="mt-0.5 block text-xs leading-relaxed text-bone-faint">{hint}</span> : null}
        </span>
      </label>
    </li>
  )

  return (
    <fieldset className="min-w-0 px-5 py-4">
      <legend className="float-left w-full text-sm font-medium text-bone">{w.events}</legend>
      <div className="clear-both">
        <p className="mt-0.5 font-mono text-[0.6875rem] text-bone-faint">{key}</p>
        {hints[key] ? <p className="mt-1.5 max-w-prose text-xs leading-relaxed text-bone-faint">{hints[key]}</p> : null}

        <div className="mt-3 flex flex-wrap items-center gap-3">
          <label className="flex cursor-pointer items-center gap-3 has-[:disabled]:cursor-not-allowed">
            <input
              type="checkbox"
              className="size-4 shrink-0 cursor-pointer accent-vermillion disabled:cursor-not-allowed"
              checked={every}
              disabled={save.isPending}
              onChange={(event) => toggleEvery(event.target.checked)}
            />
            <span className="text-sm text-bone">{w.every}</span>
            <span className="font-mono text-[0.6875rem] text-bone-faint">{EVERY}</span>
          </label>
          {save.isPending ? <Spinner className="size-3.5 text-bone-faint" /> : null}
          {save.isSuccess ? (
            <span className="flex items-center gap-1 text-xs text-moss">
              <Glyph name="check" className="size-3.5" />
              {t.settings.saved}
            </span>
          ) : null}
        </div>

        <fieldset className="mt-4 min-w-0">
          <legend className="label">{w.facts}</legend>
          <ul className="mt-1.5">
            {known.facts.map((name) => box(name, w.fact[name] ?? name, w.factHint[name]))}
          </ul>
        </fieldset>

        <details className="group mt-4">
          <summary className="label flex min-h-11 cursor-pointer list-none items-center gap-2 transition-colors duration-150 hover:text-bone [&::-webkit-details-marker]:hidden">
            <Glyph name="chevronRight" className="size-3.5 transition-transform duration-150 group-open:rotate-90" />
            {w.actions}
            <span className="font-mono tabular-nums normal-case">
              {actionsTicked}/{known.actions.length}
            </span>
          </summary>
          <fieldset className="min-w-0">
            <legend className="sr-only">{w.actions}</legend>
            <ul className="mt-1 grid gap-x-6 sm:grid-cols-2">
              {known.actions.map((name) => box(name, says[name] ?? name))}
            </ul>
          </fieldset>
        </details>

        {unknown.length ? (
          <p className="mt-3 max-w-prose text-xs leading-relaxed text-bone-faint">
            {w.unknown(unknown.join(', '))}
          </p>
        ) : null}
        {error ? (
          <p role="alert" className="mt-2 flex items-start gap-1.5 text-xs text-vermillion">
            <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
            {error}
          </p>
        ) : null}
      </div>
    </fieldset>
  )
}

