/**
 * What this server does, and how it was started.
 *
 * Two kinds of fact, and the page is ordered by which one an operator came for.
 * The settings come first and are live: the server keeps them in a table it
 * re-reads on every request, so a change applies to the next one. They can be
 * searched — by key, by title, by the sentence that explains them — and each
 * says whether it is other than it began, and offers the way back. Last, in a
 * section of their own, sit the things that really were read once from the
 * environment — the version, the database, whether a TMDB key exists, what
 * stands at each door — and those stay read-only, because a form that
 * appeared to change them would be lying.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  ButtonLink,
  Chip,
  EmptyState,
  Field,
  Glyph,
  Input,
  Label,
  OnThisPage,
  Panel,
  PanelHead,
  PolicyChip,
  SectionTitle,
  Skeleton,
  Spinner,
} from '../../components/ui'
import {
  type GroupId,
  ServerSettings,
  groupOf,
  isChanged,
  settingMatches,
  useRegistry,
  useServerValues,
} from '../../components/ScopeSettings'
import { KometaExport } from '../../components/Exports'
import { ApiError, api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import type { ExportStarted, JobsResponse, Settings as Config } from '../../lib/types'

/** The groups of settings the page draws, in the order it draws them. */
const SETTING_GROUPS: GroupId[] = ['answering', 'providers', 'sources', 'refresh', 'adult', 'other']

export function Settings() {
  const { t } = useI18n()
  const c = t.admin.config
  // What was typed to find a setting by, and whether only the changed ones are wanted.
  const [term, setTerm] = useState('')
  const [changedOnly, setChangedOnly] = useState(false)

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Config>('/settings'),
    staleTime: 5 * 60_000,
  })
  const registry = useRegistry()
  const values = useServerValues()

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
  // The settings the panels draw, those that answer to the search, and those
  // among them that are other than they began.
  const here = (registry.data ?? []).filter(
    (def) => def.scopes.includes('server') && SETTING_GROUPS.includes(groupOf(def.key)),
  )
  const stateOf = (key: string) => values.data?.find((value) => value.key === key)
  const changed = here.filter((def) => isChanged(def, stateOf(def.key)))
  const shown = here.filter(
    (def) => settingMatches(def, t, term) && (!changedOnly || isChanged(def, stateOf(def.key))),
  )
  const searching = term.trim() !== '' || changedOnly
  // The groups with a setting left, for the index: the same grouping the
  // panels below are drawn by.
  const groups = SETTING_GROUPS.filter((group) => shown.some((def) => groupOf(def.key) === group))

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

      <div role="search" aria-label={c.search} className="rise mb-4">
        <div className="flex flex-wrap items-center gap-3">
          <div className="relative min-w-60 flex-1">
            <Glyph
              name="search"
              className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
            />
            <Input
              type="search"
              aria-label={c.search}
              placeholder={c.search}
              autoComplete="off"
              spellCheck={false}
              value={term}
              onChange={(event) => setTerm(event.target.value)}
              className="pl-9"
            />
          </div>
          {/* Brass, as a hand's doing is everywhere on this side. Offered
              while there is something changed to look at, or the filter is on
              and has to be got out of. */}
          {changed.length || changedOnly ? (
            <button
              type="button"
              aria-pressed={changedOnly}
              onClick={() => setChangedOnly((was) => !was)}
              className={cn(
                'inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-full border px-4 text-sm font-medium transition-colors duration-150',
                changedOnly
                  ? 'border-brass-deep bg-brass/10 text-brass'
                  : 'border-rule-bright text-bone-dim hover:border-brass-deep hover:text-brass',
              )}
            >
              <Glyph name="pencil" className="size-4" />
              {c.changedOnly}
              <span className="font-mono text-xs tabular-nums">{changed.length}</span>
            </button>
          ) : null}
        </div>
        {/* Said, and read out as it changes: how much of the page is left. */}
        <p role="status" className="mt-2 min-h-4 font-mono text-xs text-bone-faint tabular-nums">
          {searching ? c.showing(shown.length, here.length) : ''}
        </p>
      </div>

      <OnThisPage
        label={t.admin.inPage}
        entries={[
          ...groups.map((group) => ({ id: `settings-${group}`, label: t.settings.groups[group] })),
          { id: 'settings-webhooks', label: t.settings.groups.webhooks },
          { id: 'settings-nfo', label: c.export },
          { id: 'settings-docs', label: c.docs },
          { id: 'settings-maintenance', label: c.maintenance },
          { id: 'settings-password', label: c.password },
          { id: 'settings-startup', label: c.startup },
        ]}
      />

      {searching && shown.length === 0 ? (
        <EmptyState
          title={term.trim() ? c.searchNone(term.trim()) : c.changedNone}
          hint={term.trim() ? c.searchNoneHint : undefined}
          action={
            <Button
              size="sm"
              onClick={() => {
                setTerm('')
                setChangedOnly(false)
              }}
            >
              {c.searchClear}
            </Button>
          }
        />
      ) : null}

      <ServerSettings delay={40} filter={term} changedOnly={changedOnly} />

      <Webhooks delay={240} />

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
        <KometaExport />
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

      {/* Apart from the settings, and after everything an operator does here:
          what was read from the environment once, at startup. Nothing in it
          can be changed from this page. */}
      <section id="settings-startup" aria-label={c.startup} className="mt-12 scroll-mt-28 lg:scroll-mt-16">
        <SectionTitle>{c.startup}</SectionTitle>
        <p className="-mt-2 mb-6 max-w-prose text-sm leading-relaxed text-bone-dim">{c.startupLead}</p>
        <div className="grid gap-6 lg:grid-cols-2">
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
      </section>
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

/**
 * Where the webhook went: a page of its own, with as many destinations as
 * are wanted — each with its events, a test and a log. The address and the
 * events this panel held became the destination “Webhook” there, and are no
 * settings any more; the panel stays, so whoever looks for it here is led
 * on.
 */
function Webhooks({ delay }: { delay: number }) {
  const { t } = useI18n()
  const w = t.admin.config.webhooks

  return (
    <Panel
      id="settings-webhooks"
      label={t.settings.groups.webhooks}
      className="rise mt-6"
      style={{ animationDelay: `${delay}ms` }}
    >
      <PanelHead title={t.settings.groups.webhooks} />
      <div className="flex flex-wrap items-center justify-between gap-4 px-5 py-4">
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{w.moved}</p>
        <Link
          to="/admin/notifications"
          className="inline-flex min-h-11 shrink-0 items-center gap-2 rounded-full border border-rule-bright px-5 text-sm font-medium text-bone transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high"
        >
          <Glyph name="bell" className="size-4" />
          {w.open}
        </Link>
      </div>
    </Panel>
  )
}

