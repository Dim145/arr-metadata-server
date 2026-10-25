/**
 * What this server holds, what it has been doing, and what is left to do.
 *
 * What is left to do comes first: a list of errands, each a way into the
 * catalogue filtered to what it counts or into the page that settles it, and
 * nothing at all for a count of nought. The figures are caption — the series
 * and the films, which grow on their own as clients ask for things, the keys,
 * the trail. The locked fields are counted too, and small: they say whether
 * anyone is curating this catalogue or merely running it, but a zero there is
 * no fault, and it was the largest thing on the page, in brass.
 */

import { useQuery } from '@tanstack/react-query'
import { useId, useRef, useState } from 'react'
import { Link } from 'react-router'

import {
  EmptyState,
  Glyph,
  Label,
  Lamp,
  Panel,
  PanelHead,
  PolicyChip,
  Skeleton,
  type GlyphName,
} from '../../components/ui'
import { ApiError, api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useMe } from '../../lib/hooks'
import { useI18n, type Dict } from '../../lib/i18n'
import { providerName } from '../../lib/labels'
import type {
  Errands,
  JobsResponse,
  MediaByWork,
  MediaStatus,
  Settings,
  Stats,
  Health,
  Locks,
  LocksImported,
  UpdateAvailable,
} from '../../lib/types'
import { RunStatus } from './Jobs'

export function Dashboard() {
  const { t } = useI18n()
  const me = useMe()

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
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
          {t.admin.overview.lead}
        </p>
      </header>

      {/* The list of errands is the administrator's: it counts the keys and the
          media, which an editor is not told of. */}
      {me.data?.isAdmin ? <ToDo /> : null}

      {stats.isPending ? (
        <div className="grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-rule bg-rule sm:grid-cols-3 lg:grid-cols-5">
          {Array.from({ length: 5 }, (_, index) => (
            <div key={index} className={cn('bg-ink-raised p-5', index === 4 && 'col-span-2 lg:col-span-1')}>
              <Skeleton className="h-16 w-full" />
            </div>
          ))}
        </div>
      ) : stats.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-status-error">
          <Glyph name="alert" className="size-4" />
          {t.admin.overview.statsFailed}
        </p>
      ) : (
        <div className="rise grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-rule bg-rule sm:grid-cols-3 lg:grid-cols-5">
          <Metric label={t.stats.series} value={stats.data.series} />
          <Metric label={t.stats.films} value={stats.data.movies} />
          <Metric label={t.stats.clients} value={stats.data.clients} />
          <Metric
            label={t.stats.auditEntries}
            value={stats.data.auditEntries}
            hint={t.stats.changesRecorded}
            to="/admin/audit"
          />
          {/* Last, and quiet: the cell that fills the row's end, two wide where
              the row is short of it. */}
          <Metric
            className="col-span-2 lg:col-span-1"
            label={t.stats.lockedFields}
            value={stats.data.overrides}
            hint={t.stats.lockedHint}
            quiet
          />
        </div>
      )}

      <div className="mt-6 grid items-start gap-6 lg:grid-cols-2">
        <RecentRuns />
        {/* What the media kept weigh, and which works weigh most: the
            administrator's, as the media page is. */}
        {me.data?.isAdmin ? <MediaTile /> : null}
        <Surfaces settings={settings.data} />
        <Locking />
        <LocksFile />
        <HealthPanel />
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
  quiet,
  to,
  className,
}: {
  label: string
  /** Absent where the server does not tell this reader how it is run: a dash. */
  value: number | undefined
  hint?: string
  /** A figure that is context rather than news: smaller, and a tier dimmer. */
  quiet?: boolean
  to?: string
  className?: string
}) {
  const { locale } = useI18n()

  return (
    // Every figure starts at the top, so every one in a row sits at the same
    // height — pushed to the bottom, a figure with a caption under it rode
    // higher than its neighbour without one.
    <div className={cn('flex flex-col justify-start gap-3 bg-ink-raised px-5 py-5', className)}>
      {to ? (
        <Link
          to={to}
          className="label inline-flex min-h-11 w-fit items-center gap-1.5 transition-colors duration-150 hover:text-vermillion"
        >
          {label}
          <Glyph name="chevronRight" className="size-3" />
        </Link>
      ) : (
        // As tall as the linked caption beside it, which needs 44px to be a
        // target; otherwise the two figures under them start at different
        // heights.
        <Label className="inline-flex min-h-11 items-center">{label}</Label>
      )}
      <div>
        <p
          className={cn(
            'font-display leading-none tabular-nums',
            quiet ? 'text-3xl text-bone-dim' : 'text-4xl text-bone',
          )}
        >
          {fmt.count(value, locale)}
        </p>
        {hint ? <p className="mt-2 text-xs leading-relaxed text-bone-faint">{hint}</p> : null}
      </div>
    </div>
  )
}

/** One errand: where it leads, how it is said, and how many there are of it. */
interface Errand {
  key: string
  to: string
  glyph: GlyphName
  /** A failure rather than a gap: drawn in the accent. */
  urgent?: boolean
  count: number
  text: (shown: string) => string
}

/** The errands that are true, in the order they are worth doing. */
function errandLines(todo: Errands, k: Dict['admin']['overview'], locale: string): Errand[] {
  const all: Errand[] = [
    {
      key: 'failing',
      // The address the catalogue reads, not the API's own parameter.
      to: '/admin/catalogue?refreshFailed=1',
      glyph: 'alert',
      urgent: true,
      count: todo.refreshFailed,
      text: (shown) => k.failing(shown, todo.refreshFailed),
    },
    {
      key: 'poster',
      to: '/admin/catalogue?missing=poster',
      glyph: 'image',
      count: todo.noPoster,
      text: (shown) => k.todoNoPoster(shown, todo.noPoster),
    },
    {
      key: 'overview',
      to: '/admin/catalogue?missing=overview',
      glyph: 'journal',
      count: todo.noOverview,
      text: (shown) => k.todoNoOverview(shown, todo.noOverview),
    },
    {
      key: 'imdb',
      to: '/admin/catalogue?missing=imdb',
      glyph: 'link',
      count: todo.noImdb,
      text: (shown) => k.todoNoImdb(shown, todo.noImdb),
    },
    {
      key: 'refresh',
      to: '/admin/catalogue?missing=refresh',
      glyph: 'refresh',
      count: todo.neverRefreshed,
      text: (shown) => k.todoNeverRefreshed(shown, todo.neverRefreshed),
    },
    {
      // A lock its source moved under since it was set: kept, or the
      // source's value taken, from the field's own row in the editor.
      key: 'diverging',
      to: '/admin/catalogue?missing=divergingLock',
      glyph: 'lock',
      count: todo.divergingLocks,
      text: (shown) => k.todoDiverging(shown, todo.divergingLocks),
    },
    {
      // Two records of one work, or what looks like it: merged, or said not
      // to be one, from the catalogue narrowed to them.
      key: 'duplicates',
      to: '/admin/catalogue?missing=duplicates',
      glyph: 'copy',
      count: todo.duplicates,
      text: (shown) => k.todoDuplicates(shown, todo.duplicates),
    },
    {
      key: 'keys',
      to: '/admin/clients',
      glyph: 'key',
      count: todo.expiringKeys,
      text: (shown) => k.todoKeys(shown, todo.expiringKeys, todo.keyHorizonDays),
    },
    {
      key: 'media',
      to: '/admin/media',
      glyph: 'download',
      urgent: true,
      count: todo.failedMedia,
      text: (shown) => k.todoMedia(shown, todo.failedMedia),
    },
    {
      // One thing to settle, or none: an archive made now settles it.
      key: 'backup',
      to: '/admin/backups',
      glyph: 'shield',
      urgent: true,
      count: todo.backupOverdue ? 1 : 0,
      text: () => k.todoBackup(fmt.relative(todo.lastBackupAt, locale)),
    },
  ]
  return all.filter((errand) => errand.count > 0)
}

/**
 * What there is to do.
 *
 * A line for each thing that is true — works whose last refresh failed, works
 * with no poster, no synopsis, no IMDb identifier, works never refreshed,
 * works with a lock their source moved under, works that look like another,
 * keys about to lapse, media given up on — each the way into the catalogue
 * filtered to those works, or into the page that settles it. A figure of
 * nought is not a line: a list that is always there, reading zero, is one
 * nobody reads on the day it says something. With nothing left, it says so
 * once, as a thing that is true too.
 */
function ToDo() {
  const { t, locale } = useI18n()
  const k = t.admin.overview

  const todo = useQuery({
    queryKey: ['admin', 'todo'],
    queryFn: () => api.get<Errands>('/admin/todo'),
    refetchInterval: 60_000,
  })

  const lines = todo.data ? errandLines(todo.data, k, locale) : []

  return (
    <Panel label={k.todoTitle} className="rise mb-6">
      <PanelHead title={k.todoTitle} />
      {todo.isPending ? (
        <div className="space-y-2 p-4">
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-4/5" />
        </div>
      ) : todo.isError ? (
        <p className="px-5 py-4 text-sm text-bone-faint">{k.todoLoadFailed}</p>
      ) : lines.length === 0 ? (
        <p className="flex items-center gap-2 px-5 py-4 text-sm text-bone-dim">
          <Glyph name="check" className="size-4 shrink-0 text-status-ok" />
          {k.todoNothing}
        </p>
      ) : (
        <ul className="divide-y divide-rule">
          {lines.map((line) => (
            <li key={line.key}>
              <Link
                to={line.to}
                className="group flex min-h-12 items-center gap-3 px-5 py-2 text-sm transition-colors duration-150 hover:bg-ink-high"
              >
                <Glyph
                  name={line.glyph}
                  className={cn('size-4 shrink-0', line.urgent ? 'text-vermillion' : 'text-bone-faint')}
                />
                <span className="min-w-0 flex-1 text-bone">{line.text(fmt.count(line.count, locale))}</span>
                <Glyph
                  name="chevronRight"
                  className="size-3.5 shrink-0 text-bone-faint transition-colors duration-150 group-hover:text-vermillion"
                />
              </Link>
            </li>
          ))}
        </ul>
      )}
    </Panel>
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
        <p role="alert" className="px-5 py-4 text-sm text-status-error">
          {t.admin.runs.loadFailed}
        </p>
      ) : jobs.data.jobs.length === 0 ? (
        <EmptyState title={t.admin.runs.empty} hint={t.admin.runs.emptyHint} />
      ) : (
        <ul className="divide-y divide-rule">
          {jobs.data.jobs.map((job) => (
            <li key={job.id} className="flex items-center gap-3 px-5 py-2.5">
              <RunStatus status={job.status} />
              <span className="min-w-0 flex-1 truncate">
                <span className="font-mono text-xs text-slate">{job.kind}</span>
                {job.work ? <span className="text-sm text-bone-dim"> · {job.work.title}</span> : null}
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

/**
 * The media kept, in a few figures: how many, what they and their copies
 * weigh, how many were given up on — and the works that weigh most, each the
 * way into its editor. Nothing while no store is configured.
 */
function MediaTile() {
  const { t, locale } = useI18n()
  const m = t.admin.overview.media
  const status = useQuery({
    queryKey: ['media', 'status'],
    queryFn: () => api.get<MediaStatus>('/media/status'),
    staleTime: 60_000,
  })
  const on = status.data !== undefined && status.data.backend !== 'off'
  const heaviest = useQuery({
    queryKey: ['media', 'works', 'space', 'top'],
    queryFn: () => api.get<MediaByWork>(`/media/works${query({ sort: 'space', limit: 3 })}`),
    enabled: on,
    staleTime: 5 * 60_000,
  })
  if (!on || !status.data) return null
  const { counts, variants } = status.data
  const copies = variants.webpBytes + variants.avifBytes + variants.jpegBytes

  return (
    <Panel id="media-tile" className="rise" style={{ animationDelay: '100ms' }}>
      <PanelHead
        title={m.title}
        action={
          <Link
            to="/admin/media"
            className="label -my-2 inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
          >
            {m.open}
          </Link>
        }
      />
      <dl className="grid grid-cols-3 gap-px border-b border-rule bg-rule">
        {(
          [
            [m.kept, fmt.count(counts.stored, locale), false],
            [m.size, fmt.bytes(counts.bytes + copies, locale), false],
            [m.failed, fmt.count(counts.failed, locale), counts.failed > 0],
          ] as const
        ).map(([label, value, bad]) => (
          <div key={label} className="bg-ink-raised px-5 py-3">
            <dt className="label">{label}</dt>
            <dd className={cn('mt-1 font-display text-2xl tabular-nums', bad ? 'text-vermillion' : 'text-bone')}>{value}</dd>
          </div>
        ))}
      </dl>
      {counts.failed > 0 ? (
        // Which works the pictures given up on were for: the catalogue,
        // narrowed to them.
        <p className="px-5 pt-3 text-xs">
          <Link
            to="/admin/catalogue?missing=mediaFailed"
            className="inline-flex min-h-6 items-center text-vermillion underline decoration-vermillion/40 underline-offset-4 transition-colors duration-150 hover:decoration-vermillion"
          >
            {m.failedWorks}
          </Link>
        </p>
      ) : null}
      <p className="px-5 pt-3 text-xs text-bone-faint">{m.copies(fmt.bytes(copies, locale))}</p>
      {heaviest.data?.works.length ? (
        <>
          <p className="label px-5 pt-3">{m.heaviest}</p>
          <ul className="divide-y divide-rule">
            {heaviest.data.works.map((work) => (
              <li key={work.id}>
                <Link
                  to={`/admin/catalogue/${work.id}`}
                  className="flex min-h-11 items-center gap-3 px-5 py-2 text-sm transition-colors duration-150 hover:bg-ink-high"
                >
                  <span className="min-w-0 flex-1 truncate text-bone">{work.title}</span>
                  <span className="shrink-0 font-mono text-xs text-bone-dim tabular-nums">
                    {m.workLine(fmt.count(work.stored, locale), fmt.bytes(work.bytes + work.copiesBytes, locale))}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        </>
      ) : (
        <div className="h-3" />
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
    ['/v4/*', t.admin.overview.tvdbClients, settings?.tvdbPolicy],
    ['graphql.anilist.co', t.admin.overview.anilistClients, settings?.anilistPolicy],
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
      {/* A list of entry points, each with who uses it and how it is guarded —
          not a set of terms and definitions, which is what `<dl>` claimed. */}
      <ul className="divide-y divide-rule">
        {rows.map(([path, who, policy]) => (
          <li key={path} className="flex items-center justify-between gap-4 px-5 py-2.5">
            <div className="min-w-0">
              <p className="truncate font-mono text-body-sm text-bone">{path}</p>
              <p className="mt-0.5 text-xs text-bone-faint">{who}</p>
            </div>
            {policy ? <PolicyChip policy={policy} /> : null}
          </li>
        ))}
      </ul>
    </Panel>
  )
}

/** Where it is kept whether the explanation is folded: in this browser, and nowhere else. */
const LOCKING_KEY = 'ams.dashboard.locking'

/** Unfolded until the reader has folded it: the first visit is the one it is for. */
function lockingOpen(): boolean {
  try {
    return localStorage.getItem(LOCKING_KEY) !== 'folded'
  } catch {
    // Private browsing, or storage disabled: shown, as on a first visit.
    return true
  }
}

/**
 * The one idea an operator has to hold to use the rest of this side.
 *
 * Worth reading once and then in the way, on a page opened every day: so it
 * folds to its title, and stays folded where the reader left it.
 */
function Locking() {
  const { t } = useI18n()
  const [open, setOpen] = useState(lockingOpen)
  const body = useId()

  const lines: [GlyphName, string, string][] = [
    ['cloud', 'text-slate', t.admin.overview.lockingProviders],
    ['lock', 'text-brass', t.admin.overview.lockingYours],
    ['unlock', 'text-bone-faint', t.admin.overview.lockingUnlock],
  ]

  const toggle = () => {
    const next = !open
    setOpen(next)
    try {
      localStorage.setItem(LOCKING_KEY, next ? 'open' : 'folded')
    } catch {
      // Remembered for this visit only.
    }
  }

  return (
    <Panel className="rise lg:col-span-2" style={{ animationDelay: '160ms' }}>
      <h2 className="m-0">
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          aria-controls={body}
          className={cn(
            'flex min-h-12 w-full cursor-pointer items-center justify-between gap-4 px-5 py-3 text-left',
            'transition-colors duration-150 hover:bg-ink-high',
            open && 'border-b border-rule',
          )}
        >
          <span className="label">{t.admin.overview.lockingTitle}</span>
          <Glyph
            name="chevronDown"
            className={cn('size-3.5 shrink-0 text-bone-faint transition-transform duration-200', open && 'rotate-180')}
          />
        </button>
      </h2>
      {open ? (
        <div id={body} className="grid gap-5 px-5 py-5 sm:grid-cols-3">
          {lines.map(([glyph, tone, text]) => (
            <p key={glyph} className="flex gap-3 text-sm leading-relaxed text-bone-dim">
              <Glyph name={glyph} className={cn('mt-0.5 size-4 shrink-0', tone)} />
              <span>{text}</span>
            </p>
          ))}
        </div>
      ) : null}
    </Panel>
  )
}

/* ── The locks, as a file ─────────────────────────────────────────────────── */

/** The most locks sent in one request; the server takes two thousand. */
const LOCKS_PER_REQUEST = 500

/**
 * Every lock as a document: downloaded to keep, or carried to another
 * catalogue and imported there — in parts, a request body being a megabyte
 * at most. The import says what it set, what was already so, and what it
 * could not set.
 */
function LocksFile() {
  const { t } = useI18n()
  const [busy, setBusy] = useState(false)
  const [outcome, setOutcome] = useState<{ tone: 'fine' | 'bad'; lines: string[] }>()
  const picker = useRef<HTMLInputElement>(null)

  const download = async () => {
    setBusy(true)
    try {
      const locks = await api.get<Locks>('/admin/locks')
      const blob = new Blob([JSON.stringify(locks, null, 2)], { type: 'application/json' })
      const url = URL.createObjectURL(blob)
      const link = window.document.createElement('a')
      link.href = url
      link.download = `cinematheque-locks-${locks.exportedAt.slice(0, 10)}.json`
      link.click()
      setTimeout(() => URL.revokeObjectURL(url), 10_000)
    } catch {
      setOutcome({ tone: 'bad', lines: [t.admin.overview.locksExportFailed] })
    } finally {
      setBusy(false)
    }
  }

  const upload = async (file: File | undefined) => {
    if (!file) return
    setBusy(true)
    try {
      const parsed = JSON.parse(await file.text()) as Locks
      if (!Array.isArray(parsed.locks)) throw new Error('not a locks document')
      const total: LocksImported = { applied: 0, unchanged: 0, works: 0, unmatched: [], refused: [] }
      for (let at = 0; at < parsed.locks.length || at === 0; at += LOCKS_PER_REQUEST) {
        const part = await api.post<LocksImported>('/admin/locks', {
          version: parsed.version,
          locks: parsed.locks.slice(at, at + LOCKS_PER_REQUEST),
        })
        total.applied += part.applied
        total.unchanged += part.unchanged
        total.works += part.works
        total.unmatched.push(...part.unmatched)
        total.refused.push(...part.refused)
        if (!parsed.locks.length) break
      }
      const unmatched = [...new Set(total.unmatched)]
      setOutcome({
        tone: total.refused.length || unmatched.length ? 'bad' : 'fine',
        lines: [
          t.admin.overview.locksImported(total.applied, total.works),
          total.unchanged ? t.admin.overview.locksUnchanged(total.unchanged) : undefined,
          unmatched.length ? t.admin.overview.locksUnmatched(unmatched.slice(0, 5)) : undefined,
          total.refused.length ? t.admin.overview.locksRefused(total.refused.length) : undefined,
          ...total.refused.slice(0, 3),
        ].filter((line): line is string => Boolean(line)),
      })
    } catch (error) {
      // What the server said, where it said something; otherwise the file
      // was not a document at all.
      const said =
        error instanceof ApiError
          ? error.status === 413
            ? t.admin.overview.locksTooBig
            : error.message
          : t.admin.overview.locksFailed
      setOutcome({ tone: 'bad', lines: [said] })
    } finally {
      setBusy(false)
      if (picker.current) picker.current.value = ''
    }
  }

  return (
    <Panel className="rise" style={{ animationDelay: '170ms' }} id="locks-file">
      <PanelHead title={t.admin.overview.locksTitle} />
      <div className="p-5">
        <p className="text-sm leading-relaxed text-bone-dim">{t.admin.overview.locksHint}</p>
        <div className="mt-4 flex flex-wrap items-center gap-3">
          <button
            type="button"
            onClick={() => void download()}
            disabled={busy}
            className="inline-flex min-h-11 items-center gap-2 rounded-full border border-rule-bright px-4 text-sm text-bone transition-colors duration-150 hover:border-vermillion hover:text-vermillion disabled:opacity-50"
          >
            <Glyph name="download" className="size-4" />
            {t.admin.overview.locksExport}
          </button>
          <label className="inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-full border border-brass-deep px-4 text-sm text-brass transition-colors duration-150 hover:bg-brass/10 focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-vermillion has-[:disabled]:opacity-50">
            <Glyph name="lock" className="size-4" />
            {busy ? t.admin.overview.locksImporting : t.admin.overview.locksImport}
            <input
              ref={picker}
              type="file"
              accept="application/json,.json"
              className="sr-only"
              disabled={busy}
              onChange={(event) => void upload(event.target.files?.[0])}
            />
          </label>
        </div>
        {outcome ? (
          <ul
            role="status"
            className={cn('mt-4 space-y-1 text-sm', outcome.tone === 'bad' ? 'text-status-error' : 'text-status-ok')}
          >
            {outcome.lines.map((line, index) => (
              <li key={`${index}-${line}`}>{line}</li>
            ))}
          </ul>
        ) : null}
      </div>
    </Panel>
  )
}

/* ── Health ───────────────────────────────────────────────────────────────── */

/** What the server is, holds, keeps and did last — for the administrator. */
function HealthPanel() {
  const { t, locale } = useI18n()
  const health = useQuery({
    queryKey: ['health'],
    queryFn: () => api.get<Health>('/admin/health'),
    refetchInterval: 60_000,
    retry: false,
  })
  const bytes = (n: number) => (n >= 1_048_576 ? `${(n / 1_048_576).toFixed(1)} MB` : `${Math.round(n / 1024)} kB`)
  const uptime = (seconds: number) => {
    const days = Math.floor(seconds / 86_400)
    const hours = Math.floor((seconds % 86_400) / 3600)
    const minutes = Math.floor((seconds % 3600) / 60)
    return days ? `${days} d ${hours} h` : hours ? `${hours} h ${minutes} min` : `${minutes} min`
  }

  return (
    <Panel className="rise lg:col-span-2" style={{ animationDelay: '140ms' }} id="health">
      <PanelHead title={t.admin.overview.health} />
      <div className="p-5">
        <p className="text-sm text-bone-dim">{t.admin.overview.healthHint}</p>
        {health.isPending ? (
          <Skeleton className="mt-4 h-20 w-full" />
        ) : health.isError ? (
          <p role="alert" className="mt-4 text-sm text-status-error">
            {t.admin.overview.healthFailed}
          </p>
        ) : (
          <>
          {health.data.updateAvailable ? <UpdateLine update={health.data.updateAvailable} /> : null}
          {/* The last runs are the panel above's, with its way to all of them;
              said twice on one page they were twice as long to read. */}
          <dl className="mt-4 grid gap-x-6 gap-y-3 text-sm sm:grid-cols-2 lg:grid-cols-4">
            {(
              [
                [t.admin.overview.version, health.data.version],
                [t.admin.overview.uptime, uptime(health.data.uptimeSeconds)],
                [t.admin.overview.database, health.data.database],
                [t.admin.overview.instances, health.data.mode === 'multi' ? t.admin.overview.instancesLine(health.data.instances, health.data.leader ?? null) : t.admin.overview.single],
                [t.admin.overview.refreshFailed, fmt.count(health.data.refreshFailed, locale)],
                [t.admin.overview.cacheOf('items'), t.admin.overview.cacheLine(health.data.itemsCache.entries, bytes(health.data.itemsCache.bytes))],
                [t.admin.overview.cacheOf('searches'), t.admin.overview.cacheLine(health.data.searchesCache.entries, bytes(health.data.searchesCache.bytes))],
                [t.admin.overview.cacheOf('lists'), t.admin.overview.cacheLine(health.data.listsCache.entries, bytes(health.data.listsCache.bytes))],
                [t.admin.overview.sources, health.data.sources.filter((s) => s.on).map((s) => providerName(s.name)).join(' · ') || '—'],
              ] as [string, string][]
            ).map(([label, value]) => (
              <div key={label}>
                <dt className="label">{label}</dt>
                <dd className="mt-0.5 font-mono text-body-sm text-bone tabular-nums">{value}</dd>
              </div>
            ))}
          </dl>
          </>
        )}
      </div>
    </Panel>
  )
}

/**
 * A newer release, said in one line at the top of the health: the version,
 * when it came out, and the way to what changed. Only while the check is on
 * and found one — the server sends nothing otherwise.
 */
function UpdateLine({ update }: { update: UpdateAvailable }) {
  const { t, locale } = useI18n()
  const published = update.publishedAt ? fmt.relative(update.publishedAt, locale) : undefined

  return (
    <p className="mt-4 flex flex-wrap items-center gap-x-3 gap-y-1 rounded-card border border-brass-deep bg-brass/[0.06] px-4 py-1.5 text-sm">
      <Lamp tone="brass">{t.admin.overview.updateOut(update.version)}</Lamp>
      {published && update.publishedAt ? (
        <span className="text-xs text-bone-faint" title={fmt.dateTime(update.publishedAt, locale)}>
          {t.admin.overview.updatePublished(published)}
        </span>
      ) : null}
      <a
        href={update.url}
        target="_blank"
        rel="noreferrer"
        className="ml-auto inline-flex min-h-11 items-center gap-1.5 text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
      >
        {t.admin.overview.updateRead}
        <Glyph name="external" className="size-3.5" />
      </a>
    </p>
  )
}
