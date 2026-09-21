/**
 * What this server holds, and what it has been doing.
 *
 * The figures are the subject and everything else is caption. The locked-field
 * count gets the largest of them because it is the only number here that says
 * whether anyone is curating this catalogue or merely running it — the rest
 * grow on their own as clients ask for things.
 */

import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'

import {
  Chip,
  EmptyState,
  Glyph,
  Label,
  Panel,
  PanelHead,
  Skeleton,
  type GlyphName,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { JobsResponse, Settings, Stats } from '../../lib/types'
import { RunStatus } from './Jobs'

export function Dashboard() {
  const { t } = useI18n()

  const stats = useQuery({ queryKey: ['stats'], queryFn: () => api.get<Stats>('/stats') })
  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/settings'),
    staleTime: 5 * 60_000,
  })

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl leading-tight font-medium text-bone sm:text-4xl">
          {t.admin.dashboard}
        </h1>
        <p className="mt-2 text-sm text-bone-dim">{t.brand.tagline}</p>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
          {t.admin.overview.lead}
        </p>
      </header>

      {stats.isPending ? (
        <div className="grid gap-px overflow-hidden rounded-panel border border-rule bg-rule sm:grid-cols-2 lg:grid-cols-4">
          {Array.from({ length: 5 }, (_, index) => (
            <div key={index} className="bg-ink-raised p-5">
              <Skeleton className="h-20 w-full" />
            </div>
          ))}
        </div>
      ) : stats.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {t.admin.overview.statsFailed}
        </p>
      ) : (
        <div className="rise grid gap-px overflow-hidden rounded-panel border border-rule bg-rule sm:grid-cols-2 lg:grid-cols-4">
          <Metric
            className="sm:col-span-2 lg:row-span-2"
            label={t.stats.lockedFields}
            value={stats.data.overrides}
            hint={t.stats.lockedHint}
            big
          />
          <Metric label={t.stats.series} value={stats.data.series} />
          <Metric label={t.stats.films} value={stats.data.movies} />
          <Metric label={t.stats.clients} value={stats.data.clients} />
          <Metric
            label={t.stats.auditEntries}
            value={stats.data.auditEntries}
            hint={t.stats.changesRecorded}
            to="/admin/audit"
          />
        </div>
      )}

      <div className="mt-6 grid items-start gap-6 lg:grid-cols-2">
        <RecentRuns />
        <Surfaces settings={settings.data} />
        <Locking />
      </div>
    </div>
  )
}

/**
 * A figure, with its caption.
 *
 * `to` makes the caption the way into the section the figure counts, which is
 * how the audit trail and the job history stay reachable on a phone: the bottom
 * bar holds five places and neither of them earns one.
 */
function Metric({
  label,
  value,
  hint,
  big,
  to,
  className,
}: {
  label: string
  value: number
  hint?: string
  big?: boolean
  to?: string
  className?: string
}) {
  const { locale } = useI18n()

  return (
    <div
      className={cn(
        'flex flex-col gap-4 bg-ink-raised px-5 py-5',
        // The hero cell is two rows tall; centring keeps the figure at the eye
        // rather than stranding it at the bottom of a void.
        big ? 'justify-center' : 'justify-between',
        className,
      )}
    >
      {to ? (
        <Link
          to={to}
          className="label inline-flex min-h-11 w-fit items-center gap-1.5 transition-colors duration-150 hover:text-vermillion"
        >
          {label}
          <Glyph name="chevronRight" className="size-3" />
        </Link>
      ) : (
        <Label>{label}</Label>
      )}
      <div>
        <p
          className={cn(
            'font-display leading-none tabular-nums',
            big ? 'text-6xl text-vermillion sm:text-7xl' : 'text-4xl text-bone',
          )}
        >
          {fmt.count(value, locale)}
        </p>
        {hint ? <p className="mt-2 text-xs leading-relaxed text-bone-faint">{hint}</p> : null}
      </div>
    </div>
  )
}

/** The last handful of runs; the rest are a tap away. */
function RecentRuns() {
  const { t, locale } = useI18n()

  const jobs = useQuery({
    queryKey: ['jobs', 'recent'],
    queryFn: () => api.get<JobsResponse>(`/jobs${query({ limit: 6 })}`),
    refetchInterval: 30_000,
  })

  return (
    <Panel className="rise" style={{ animationDelay: '80ms' }}>
      <PanelHead
        title={t.admin.overview.recentRuns}
        action={
          <Link
            to="/admin/jobs"
            className="label -my-2 inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
          >
            {t.admin.overview.allRuns}
          </Link>
        }
      />

      {jobs.isPending ? (
        <div className="space-y-2 p-4">
          {Array.from({ length: 3 }, (_, index) => (
            <Skeleton key={index} className="h-8 w-full" />
          ))}
        </div>
      ) : jobs.isError ? (
        <p role="alert" className="px-5 py-4 text-sm text-vermillion">
          {t.admin.runs.loadFailed}
        </p>
      ) : jobs.data.jobs.length === 0 ? (
        <EmptyState title={t.admin.runs.empty} hint={t.admin.runs.emptyHint} />
      ) : (
        <ul className="divide-y divide-rule">
          {jobs.data.jobs.map((job) => (
            <li key={job.id} className="flex items-center gap-3 px-5 py-2.5">
              <RunStatus status={job.status} />
              <span className="min-w-0 flex-1 truncate font-mono text-xs text-slate">
                {job.kind}
              </span>
              <span
                className="shrink-0 font-mono text-xs text-bone-faint tabular-nums"
                title={fmt.dateTime(job.createdAt, locale)}
              >
                {fmt.relative(job.createdAt, locale)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  )
}

/** Which door each client comes through, and what stands at it. */
function Surfaces({ settings }: { settings?: Settings }) {
  const { t } = useI18n()

  const rows: [string, string, string | undefined][] = [
    ['/v1/tvdb/*', 'Sonarr', settings?.arrPolicy],
    ['/v1/movie/*', 'Radarr', settings?.arrPolicy],
    ['/3/*', t.admin.overview.tmdbClients, settings?.tmdbPolicy],
    ['/api/v1/*', t.admin.overview.thisUi, settings?.nativePolicy],
  ]

  return (
    <Panel className="rise" style={{ animationDelay: '120ms' }}>
      <PanelHead
        title={t.admin.overview.surfaces}
        action={
          <Link
            to="/admin/settings"
            className="label -my-2 inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
          >
            {t.admin.overview.configure}
          </Link>
        }
      />
      <dl className="divide-y divide-rule">
        {rows.map(([path, who, policy]) => (
          <div key={path} className="flex items-center justify-between gap-4 px-5 py-2.5">
            <div className="min-w-0">
              <dt className="truncate font-mono text-[0.8125rem] text-bone">{path}</dt>
              <dd className="mt-0.5 text-xs text-bone-faint">{who}</dd>
            </div>
            {policy ? <PolicyChip policy={policy} /> : null}
          </div>
        ))}
      </dl>
    </Panel>
  )
}

/**
 * A policy, in a word.
 *
 * `open` is the one worth the accent: it means anyone who can reach the port is
 * served, which on a machine with a port forward is the whole internet.
 */
export function PolicyChip({ policy }: { policy: string }) {
  return (
    <Chip tone={policy === 'open' ? 'accent' : policy === 'allowlist' ? 'provider' : 'neutral'}>
      <Glyph name={policy === 'open' ? 'alert' : policy === 'allowlist' ? 'globe' : 'key'} className="size-3" />
      {policy}
    </Chip>
  )
}

/** The one idea an operator has to hold to use the rest of this side. */
function Locking() {
  const { t } = useI18n()

  const lines: [GlyphName, string, string][] = [
    ['cloud', 'text-slate', t.admin.overview.lockingProviders],
    ['lock', 'text-brass', t.admin.overview.lockingYours],
    ['unlock', 'text-bone-faint', t.admin.overview.lockingUnlock],
  ]

  return (
    <Panel className="rise lg:col-span-2" style={{ animationDelay: '160ms' }}>
      <PanelHead title={t.admin.overview.lockingTitle} />
      <div className="grid gap-5 px-5 py-5 sm:grid-cols-3">
        {lines.map(([glyph, tone, text]) => (
          <p key={glyph} className="flex gap-3 text-sm leading-relaxed text-bone-dim">
            <Glyph name={glyph} className={cn('mt-0.5 size-4 shrink-0', tone)} />
            <span>{text}</span>
          </p>
        ))}
      </div>
    </Panel>
  )
}
