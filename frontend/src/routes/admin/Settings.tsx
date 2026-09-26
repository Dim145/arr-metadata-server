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
import { useI18n } from '../../lib/i18n'
import type { ExportStarted, JobsResponse, Settings as Config } from '../../lib/types'
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
          { id: 'settings-runtime', label: t.admin.config.runtime },
          { id: 'settings-policy', label: t.admin.config.policy },
          { id: 'settings-nfo', label: t.admin.config.export },
          { id: 'settings-docs', label: t.admin.config.docs },
          { id: 'settings-maintenance', label: t.admin.config.maintenance },
          { id: 'settings-password', label: t.admin.config.password },
        ]}
      />

      <ServerSettings delay={40} />

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
  const missingPath = run.error instanceof ApiError && run.error.code === 'provider_not_configured'

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

