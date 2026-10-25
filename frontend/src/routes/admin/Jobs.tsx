/**
 * The tasks: what the server does on its own, and what it did.
 *
 * Above, one card per task — what it is for, when it runs, how its last run
 * went, when the next one comes — with the button to run it now or again,
 * to cancel the run under way where it can stop, and to read its log; and,
 * for the tasks that run on a cadence of their own, when they run: their
 * own cadence, a window of the day, or a cron expression. Below, the
 * history: one row per run rather than per work, a page at a time and under
 * the day each ran on, filtered by task, outcome, and whether the schedule
 * or a person started it — with the failures one press away at its head,
 * since they are what it is opened for.
 *
 * The cards are followed as they go: the server's stream says each change,
 * whichever instance made it, and a run's bar moves as it works. Where the
 * stream cannot be opened the page asks every few seconds instead, and while
 * the tab is hidden it does neither.
 */

import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { createContext, Fragment, useContext, useId, useMemo, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  DataTable,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  Input,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Segmented,
  Select,
  Skeleton,
  Spinner,
  TableOptions,
  TBody,
  Td,
  Th,
  THead,
  Tr,
  useDataTable,
  type Column,
  type DataTableState,
} from '../../components/ui'
import { ApiError, api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useSettled } from '../../lib/debounce'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { describeIdentity, providerName } from '../../lib/labels'
import { useTaskBoard, type Following } from '../../lib/taskStream'
import type { Dict } from '../../lib/i18n'
import type {
  ImportRule,
  Job,
  JobDetail,
  JobEntry,
  JobsResponse,
  ScheduleMoments,
  Task,
  TaskId,
  TaskSchedule,
} from '../../lib/types'

const TASKS: TaskId[] = [
  'refresh.sweep',
  'refresh.all',
  'import.anime',
  'import.imdb',
  'imdb.akas',
  'imdb.episodes',
  'import.batch',
  'import.rules',
  'export.nfo',
  'media.store',
  'media.sweep',
  'media.variants',
  'media.avif',
  'tls.renew',
  'update.check',
  'arr.seed',
  'backup.run',
  'search.rebuild',
  'listing.relist',
]

/** Whether the page follows the tasks live: the logs then wait to be told, rather than asking. */
const Live = createContext(false)

/**
 * A run's note in words. The server writes its summaries in one fixed form
 * each — `3 refreshed, 0 failed`, `12 works, 340 episodes, 0 failed` — and
 * they are read back here; anything else is shown as it came. `locale`
 * writes an archive's size as the reader counts bytes.
 */
export function describeDetail(detail: string | null | undefined, t: Dict, locale: string): string {
  if (!detail) return ''
  const s = t.admin.tasks.summaries
  const n = (text: string | undefined) => Number(text ?? 0)
  let m: RegExpMatchArray | null
  if (detail === 'nothing was due') return s.nothingDue
  if ((m = detail.match(/^stopped: (\d+) refreshed, (\d+) failed, of (\d+)$/))) return s.stopped(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^(\d+) of (\d+): (\d+) refreshed, (\d+) failed$/))) return s.progress(n(m[1]), n(m[2]), n(m[3]), n(m[4]))
  if ((m = detail.match(/^(\d+) refreshed, (\d+) failed(?:, of (\d+))?$/)))
    return s.refreshed(n(m[1]), n(m[2]), m[3] === undefined ? undefined : n(m[3]))
  if ((m = detail.match(/^(\d+) works, (\d+) episodes, (\d+) failed$/))) return s.exported(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^stopped: (\d+) stored, (\d+) failed, of (\d+)$/))) return s.storedStopped(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^(\d+) of (\d+): (\d+) stored, (\d+) failed$/))) return s.storedProgress(n(m[1]), n(m[2]), n(m[3]), n(m[4]))
  if ((m = detail.match(/^(\d+) stored, (\d+) failed, of (\d+)$/))) return s.stored(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^(\d+) forgotten, (\d+) files removed$/))) return s.swept(n(m[1]), n(m[2]))
  // The pictures prepared, and the night's AVIF copies.
  if ((m = detail.match(/^stopped: (\d+) prepared, (\d+) failed, of (\d+)$/))) return s.preparedStopped(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^(\d+) of (\d+): (\d+) prepared, (\d+) failed$/))) return s.preparedProgress(n(m[1]), n(m[2]), n(m[3]), n(m[4]))
  if ((m = detail.match(/^(\d+) prepared, (\d+) failed, of (\d+)$/))) return s.prepared(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^stopped: (\d+) copies made, (\d+) failed, of (\d+) pictures$/))) return s.avifStopped(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^(\d+) looked at: (\d+) copies made, (\d+) failed$/))) return s.avifProgress(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^(\d+) copies made, (\d+) failed, of (\d+) pictures$/))) return s.avif(n(m[1]), n(m[2]), n(m[3]))
  // A certificate's end, as the reader's locale writes a date.
  const day = (iso: string | undefined) => (iso ? new Date(`${iso}T00:00:00Z`).toLocaleDateString(undefined, { dateStyle: 'medium', timeZone: 'UTC' }) : '')
  if ((m = detail.match(/^renewed; valid until (\d{4}-\d{2}-\d{2})$/))) return s.renewed(day(m[1]))
  if ((m = detail.match(/^valid until (\d{4}-\d{2}-\d{2}); nothing to do$/))) return s.certValid(day(m[1]))
  // A sync from chosen sources names them by their keys.
  const names = (keys: string | undefined) => (keys ?? '').split(', ').map(providerName).join(', ')
  if ((m = detail.match(/^synced from (.+)$/))) return s.synced(names(m[1]))
  if ((m = detail.match(/^none of (.+) answered$/))) return s.noneAnswered(names(m[1]))
  if ((m = detail.match(/^(\d+\.\d+\.\d+) is out$/))) return s.newer(m[1] ?? '')
  // The seeding from Sonarr and Radarr, a batch and the import rules: what
  // was there, what came in, what could not be found or failed, of how many
  // — as it goes, and once stopped.
  if ((m = detail.match(/^(\d+) already here, (\d+) imported, (\d+) not found, (\d+) failed, of (\d+)$/)))
    return s.seeded(...(m.slice(1, 6).map(Number) as [number, number, number, number, number]))
  if ((m = detail.match(/^(\d+) of (\d+): (\d+) already here, (\d+) imported, (\d+) not found, (\d+) failed$/)))
    return s.seededProgress(...(m.slice(1, 7).map(Number) as [number, number, number, number, number, number]))
  if ((m = detail.match(/^stopped: (\d+) already here, (\d+) imported, (\d+) not found, (\d+) failed, of (\d+)$/)))
    return s.seededStopped(...(m.slice(1, 6).map(Number) as [number, number, number, number, number]))
  // The works listed again.
  if ((m = detail.match(/^(\d+) listed again$/))) return s.listed(n(m[1]))
  // An archive made: its name, what it weighs, and how many of the
  // schedule's oldest went; a restore: what it put back, and what undoes it.
  if ((m = detail.match(/^(ams-\S+), (\d+) bytes, (\d+) removed$/)))
    return s.backedUp(m[1] ?? '', fmt.bytes(n(m[2]), locale), n(m[3]))
  if ((m = detail.match(/^(\d+) rows, (\d+) media put back(?:; undone by restoring (\S+))?$/)))
    return s.restored(n(m[1]), n(m[2]), m[3])
  // IMDb's lists: the ratings, and the titles and the episodes as they are
  // read — the megabytes read of the whole where its size is known — what
  // they kept, and how far one stopped got.
  if ((m = detail.match(/^(\d+) of (\d+) works rated(?:, (\d+) of (\d+) episodes)?$/)))
    return s.rated(n(m[1]), n(m[2]), m[3] === undefined ? undefined : n(m[3]), m[4] === undefined ? undefined : n(m[4]))
  if ((m = detail.match(/^(\d+)(?: of (\d+))?:? megabytes read, (\d+) (titles|episodes) kept$/)))
    return s.reading(m[4] === 'titles' ? 'titles' : 'episodes', n(m[1]), n(m[3]), m[2] === undefined ? undefined : n(m[2]))
  if ((m = detail.match(/^(\d+) titles for (\d+) of (\d+) works$/))) return s.titles(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^stopped: (\d+) titles for (\d+) works$/))) return s.titlesStopped(n(m[1]), n(m[2]))
  if ((m = detail.match(/^(\d+) episodes of (\d+) of (\d+) series$/))) return s.episodes(n(m[1]), n(m[2]), n(m[3]))
  if ((m = detail.match(/^stopped: (\d+) episodes for (\d+) series$/))) return s.episodesStopped(n(m[1]), n(m[2]))
  return t.admin.runs.notes[detail] ?? detail
}

/** A moment, as the schedule's next runs are said: the day by its name, and the time where the reader is. */
function moment(iso: string | undefined, locale: string): string | undefined {
  if (!iso) return undefined
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return undefined
  return new Intl.DateTimeFormat(locale, {
    weekday: 'long',
    day: 'numeric',
    month: 'long',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

/** A time of day in UTC (`HH:MM`), as the reader's own clock says it today. */
function localTime(hhmm: string, locale: string): string | undefined {
  const match = /^(\d{2}):(\d{2})$/.exec(hhmm)
  if (!match) return undefined
  const date = new Date()
  date.setUTCHours(Number(match[1]), Number(match[2]), 0, 0)
  return new Intl.DateTimeFormat(locale, { hour: '2-digit', minute: '2-digit' }).format(date)
}

export function Jobs() {
  const { t } = useI18n()
  const { tasks, following } = useTaskBoard()
  const running = tasks.data?.some((task) => task.running) ?? false
  const live = following === 'live'

  // The rules by their ids, to name a rule's run.
  const rules = useQuery({
    queryKey: ['import-rules'],
    queryFn: () => api.get<ImportRule[]>('/import/rules'),
    staleTime: 60_000,
  })
  const ruleNames = Object.fromEntries((rules.data ?? []).map((rule) => [rule.id, rule.name]))

  return (
    <Live.Provider value={live}>
      <div className="mx-auto max-w-6xl">
        <header className="rise mb-8">
          <Label>{t.admin.title}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.jobs}</h1>
          <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.admin.tasks.lead}</p>
          <FollowingLamp following={following} />
        </header>

        {tasks.isPending ? (
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {TASKS.map((id) => (
              <Skeleton key={id} className="h-56 w-full" />
            ))}
          </div>
        ) : tasks.isError ? (
          <p role="alert" className="flex items-center gap-2 text-sm text-status-error">
            <Glyph name="alert" className="size-4" />
            {tasks.error.message}
          </p>
        ) : (
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {tasks.data.map((task, index) => (
              <TaskCard key={task.id} task={task} delay={40 + index * 30} />
            ))}
          </div>
        )}

        <History polling={running && !live} live={live} ruleNames={ruleNames} />
      </div>
    </Live.Provider>
  )
}

/** How the cards are followed, said once and quietly. */
function FollowingLamp({ following }: { following: Following }) {
  const { t } = useI18n()
  return (
    <p role="status" className="mt-3 flex items-center gap-2 text-xs text-bone-faint">
      <Lamp tone={following === 'live' ? 'moss' : 'faint'}>
        <span className="text-xs text-bone-dim">{t.admin.tasks.following[following]}</span>
      </Lamp>
    </p>
  )
}

/* ── One task ─────────────────────────────────────────────────────────────── */

function TaskCard({ task, delay }: { task: Task; delay: number }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks
  const queryClient = useQueryClient()
  const [confirming, setConfirming] = useState(false)
  const [logOpen, setLogOpen] = useState(false)
  const [scheduling, setScheduling] = useState(false)
  const name = k.names[task.id] ?? task.id

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['tasks'] })
    void queryClient.invalidateQueries({ queryKey: ['jobs'] })
  }

  const stop = useMutation({
    mutationFn: (id: string) => api.post(`/jobs/${encodeURIComponent(id)}/cancel`),
    onSuccess: refresh,
  })
  // Asked of this run, not of one before it: the card outlives its runs.
  const stopAsked = stop.isSuccess && stop.variables === task.running?.id
  const run = useMutation({
    mutationFn: () => api.post<{ jobId?: string }>(`/tasks/${task.id}/run`),
    onSuccess: () => {
      setConfirming(false)
      stop.reset()
      refresh()
    },
  })

  const cadence =
    task.mode === 'scheduled' && task.everySeconds
      ? k.every(task.everySeconds)
      : task.mode === 'manual'
        ? k.manual
        : task.mode === 'scheduled'
          ? k.eachOwn
          : k.off
  const blocked = task.blocked && task.blocked !== 'running' ? task.blocked : undefined
  const waiting = task.blocked === 'busy'
  const last = task.last && !task.running ? task.last : undefined
  // The run whose log is read: the one under way, or the last.
  const logged = task.running ?? task.last
  // A batch is started from the import page, with its lines.
  const fromImport = task.id === 'import.batch'

  return (
    <Panel id={`task-${task.id}`} label={name} className="rise flex flex-col" style={{ animationDelay: `${delay}ms` }}>
      <div className="border-b border-rule px-5 py-4">
        <h2 className="font-display text-xl leading-tight text-bone">{name}</h2>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Chip tone={task.mode === 'off' ? 'neutral' : 'provider'}>
            <Glyph name={task.mode === 'manual' ? 'play' : task.mode === 'off' ? 'power' : 'clock'} className="size-3" />
            {cadence}
          </Chip>
          {task.schedule ? (
            // Set by somebody: in the brass of what people do by hand.
            <Chip tone="manual">
              <Glyph name="clock" className="size-3" />
              {scheduleWords(task.schedule, k)}
            </Chip>
          ) : null}
        </div>
        <p className="mt-2.5 text-xs leading-relaxed text-bone-dim">{k.about[task.id]}</p>
      </div>

      <dl className="grid flex-1 content-start gap-3 px-5 py-4 text-sm">
        {task.running ? (
          <div>
            <dt className="label">{k.last}</dt>
            <dd className="mt-1 space-y-2">
              <Lamp tone="brass">{t.admin.runs.running}</Lamp>
              <RunProgress task={task} name={name} />
              {task.running.detail ? (
                <p className="font-mono text-xs text-bone-dim tabular-nums">{describeDetail(task.running.detail, t, locale)}</p>
              ) : null}
            </dd>
          </div>
        ) : (
          <div>
            <dt className="label">{k.last}</dt>
            <dd className="mt-1">
              {last ? (
                <div className="space-y-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <RunStatus status={last.status} />
                    <span className="text-xs text-bone-faint" title={fmt.dateTime(last.createdAt, locale)}>
                      {fmt.relative(last.createdAt, locale)}
                    </span>
                  </div>
                  <p className={last.error ? 'text-xs break-words text-vermillion' : 'text-xs text-bone-dim'}>
                    {describeDetail(last.error ?? last.detail, t, locale)}
                  </p>
                  {last.status === 'failed' && task.lastSuccessAt ? (
                    <p className="text-xs text-bone-faint">
                      {k.lastSuccess(fmt.relative(task.lastSuccessAt, locale) ?? '')}
                    </p>
                  ) : null}
                </div>
              ) : (
                <span className="text-bone-faint">{k.never}</span>
              )}
            </dd>
          </div>
        )}
        {task.nextAt && !task.running ? (
          <div>
            <dt className="label">{k.next}</dt>
            <dd className="mt-1 text-bone" title={fmt.dateTime(task.nextAt, locale)}>
              {/* Due already — waiting for its window, a lock, the next
                  look — is said so, not as a moment in the past. */}
              {Date.parse(task.nextAt) <= Date.now() + 5_000 ? k.dueNow : fmt.relative(task.nextAt, locale)}
              <span className="block text-xs text-bone-faint">{moment(task.nextAt, locale)}</span>
            </dd>
          </div>
        ) : null}
      </dl>
      {blocked ? (
        <p className="flex items-start gap-2 px-5 pb-4 text-xs text-bone-faint">
          <Glyph name="power" className="mt-0.5 size-3 shrink-0" />
          {k.blocked[blocked]}
        </p>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 border-t border-rule px-5 py-3">
        {task.running && task.cancelable ? (
          <Button
            size="sm"
            variant="danger"
            disabled={stop.isPending || stopAsked}
            aria-label={`${k.cancel}: ${name}`}
            onClick={() => task.running && stop.mutate(task.running.id)}
          >
            {stop.isPending ? <Spinner className="size-4" /> : <Glyph name="close" className="size-4" />}
            {k.cancel}
          </Button>
        ) : fromImport ? (
          <Link
            to="/admin/discover?many=1"
            className="inline-flex min-h-11 items-center gap-1.5 text-sm text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
          >
            <Glyph name="download" className="size-4" />
            {k.openImport}
          </Link>
        ) : (
          <Button
            size="sm"
            variant="primary"
            disabled={Boolean(task.running) || Boolean(blocked) || waiting || run.isPending}
            aria-label={`${task.last ? k.runAgain : k.run}: ${name}`}
            onClick={() => (task.id === 'refresh.all' ? setConfirming(true) : run.mutate())}
          >
            {run.isPending || task.running ? <Spinner className="size-4" /> : <Glyph name={task.last ? 'refresh' : 'play'} className="size-4" />}
            {task.last ? k.runAgain : k.run}
          </Button>
        )}
        {logged ? (
          <Button size="sm" aria-label={`${k.log}: ${name}`} onClick={() => setLogOpen(true)}>
            <Glyph name="journal" className="size-4" />
            {k.log}
          </Button>
        ) : null}
        {task.schedulable ? (
          <Button size="sm" variant="quiet" aria-label={`${k.when.edit}: ${name}`} onClick={() => setScheduling(true)}>
            <Glyph name="clock" className="size-4" />
            {k.when.edit}
          </Button>
        ) : null}
        {task.id === 'import.rules' ? (
          <Link
            to="/admin/lists#import-rules"
            className="inline-flex min-h-11 items-center gap-1.5 text-sm text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
          >
            {k.openRules}
            <Glyph name="chevronRight" className="size-3.5" />
          </Link>
        ) : null}
        {stopAsked ? <span className="text-xs text-bone-faint">{k.stopAsked}</span> : null}
        {run.isError ? (
          <span role="alert" className="text-xs text-status-error">
            {run.error instanceof ApiError ? run.error.message : t.common.error}
          </span>
        ) : null}
      </div>

      <Dialog
        open={confirming}
        title={k.confirmAll}
        onClose={() => setConfirming(false)}
        footer={
          <>
            <Button onClick={() => setConfirming(false)}>{t.account.cancel}</Button>
            <Button variant="primary" disabled={run.isPending} onClick={() => run.mutate()}>
              {run.isPending ? <Spinner className="size-4" /> : null}
              {k.confirmAllGo}
            </Button>
          </>
        }
      >
        <p className="text-sm leading-relaxed text-bone-dim">{k.confirmAllBody}</p>
      </Dialog>

      <Dialog
        open={logOpen}
        size="lg"
        title={k.logOf(name)}
        onClose={() => setLogOpen(false)}
        footer={<Button onClick={() => setLogOpen(false)}>{k.close}</Button>}
      >
        {logged ? <JobDetailPanel job={logged} /> : null}
      </Dialog>

      {task.schedulable ? (
        <ScheduleDialog task={task} name={name} open={scheduling} onClose={() => setScheduling(false)} />
      ) : null}
    </Panel>
  )
}

/** A schedule in a few words: its window, or its expression, in UTC. */
function scheduleWords(schedule: TaskSchedule, k: Dict['admin']['tasks']): string {
  if (schedule.kind === 'window') return k.when.window(schedule.from ?? '', schedule.to ?? '')
  return k.when.cron(schedule.cron ?? '')
}

/**
 * How far a run has got: a bar, and the count beside it. One that does not
 * know how many there are is a bar that only says it is working.
 */
function RunProgress({ task, name }: { task: Task; name: string }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks
  const progress = task.progress
  const total = progress?.total ?? undefined
  const done = progress?.done ?? 0
  const share = total ? Math.min(1, done / Math.max(total, 1)) : undefined

  return (
    <div className="space-y-1">
      <progress
        aria-label={k.progressOf(name)}
        value={total ? Math.min(done, total) : undefined}
        max={total ?? undefined}
        className={cn(
          'block h-1.5 w-full appearance-none overflow-hidden rounded-full border-0 bg-ink-high',
          '[&::-webkit-progress-bar]:bg-ink-high [&::-webkit-progress-value]:bg-brass [&::-moz-progress-bar]:bg-brass',
          '[&::-webkit-progress-value]:transition-[width] [&::-webkit-progress-value]:duration-500',
          // Working, how far unknown: it breathes, unless the reader asked
          // for stillness.
          '[&:indeterminate]:animate-pulse [&:indeterminate]:bg-brass/40 [&:indeterminate::-webkit-progress-bar]:bg-brass/40 motion-reduce:[&:indeterminate]:animate-none',
        )}
      />
      {/* The run's own line, below, says how many of how many: here, only
          what it comes to, or how many so far when nobody knows of how many. */}
      {progress ? (
        <p className="text-right font-mono text-label text-bone-faint tabular-nums">
          {share !== undefined
            ? new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 0 }).format(share)
            : k.progressSoFar(fmt.count(done, locale))}
        </p>
      ) : null}
    </div>
  )
}

/** The common schedules, as a cron expression each. */
const PRESETS: { id: string; cron: string }[] = [
  { id: 'nightly', cron: '0 3 * * *' },
  { id: 'sundays', cron: '0 4 * * 0' },
  { id: 'mondays', cron: '0 5 * * 1' },
  { id: 'monthly', cron: '0 4 1 * *' },
]

type ScheduleKind = 'own' | 'window' | 'cron'

/**
 * When a periodic task runs: its own cadence; inside a window of the day;
 * or at the moments a cron expression names. What is being written is said
 * as it is written — its next runs, in the reader's time, or why it is no
 * schedule — before anything is kept.
 */
function ScheduleDialog({ task, name, open, onClose }: { task: Task; name: string; open: boolean; onClose: () => void }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks.when
  const queryClient = useQueryClient()
  const ids = useId()
  const [kind, setKind] = useState<ScheduleKind>(task.schedule?.kind ?? 'own')
  const [from, setFrom] = useState(task.schedule?.from ?? '01:00')
  const [to, setTo] = useState(task.schedule?.to ?? '06:00')
  const [cron, setCron] = useState(task.schedule?.cron ?? '0 4 * * 0')

  const setting: TaskSchedule | null =
    kind === 'window' ? { kind: 'window', from, to } : kind === 'cron' ? { kind: 'cron', cron } : null
  const settled = useSettled(setting, 350)
  const preview = useQuery({
    queryKey: ['schedule-preview', settled],
    queryFn: () => api.post<ScheduleMoments>('/tasks/schedules/preview', settled),
    enabled: open && settled !== null,
    retry: false,
    staleTime: 30_000,
  })

  const save = useMutation({
    mutationFn: () =>
      setting
        ? api.put<Task>(`/tasks/${task.id}/schedule`, setting)
        : api.delete<Task>(`/tasks/${task.id}/schedule`),
    onSuccess: (state) => {
      queryClient.setQueryData<Task[]>(['tasks'], (old) => old?.map((one) => (one.id === state.id ? state : one)))
      onClose()
    },
  })

  const localFrom = localTime(from, locale)
  const localTo = localTime(to, locale)

  return (
    <Dialog
      open={open}
      size="lg"
      title={k.editOf(name)}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t.common.cancel}</Button>
          <Button variant="primary" disabled={save.isPending || (setting !== null && preview.isError)} onClick={() => save.mutate()}>
            {save.isPending ? <Spinner className="size-4" /> : <Glyph name="check" className="size-4" />}
            {setting ? k.save : k.reset}
          </Button>
        </>
      }
    >
      <div className="space-y-5">
        <Segmented<ScheduleKind>
          label={k.heading}
          value={kind}
          onChange={setKind}
          options={[
            { value: 'own', label: k.own },
            { value: 'window', label: k.kindWindow },
            { value: 'cron', label: k.kindCron },
          ]}
        />
        <p className="text-sm leading-relaxed text-bone-dim">
          {kind === 'own'
            ? k.ownLead(task.everySeconds ? t.admin.tasks.every(task.everySeconds) : '')
            : kind === 'window'
              ? k.windowLead
              : k.cronLead}
        </p>

        {kind === 'window' ? (
          <div className="grid gap-4 sm:grid-cols-2">
            <FormField label={k.from} htmlFor={`${ids}-from`} hint={k.inUtc}>
              <Input id={`${ids}-from`} type="time" required value={from} onChange={(event) => setFrom(event.target.value)} />
            </FormField>
            <FormField label={k.to} htmlFor={`${ids}-to`}>
              <Input id={`${ids}-to`} type="time" required value={to} onChange={(event) => setTo(event.target.value)} />
            </FormField>
            {localFrom && localTo ? <p className="text-xs text-bone-faint sm:col-span-2">{k.whereYouAre(localFrom, localTo)}</p> : null}
          </div>
        ) : null}

        {kind === 'cron' ? (
          <div className="space-y-3">
            <FormField label={k.expression} htmlFor={`${ids}-cron`} hint={k.expressionHint}>
              <Input
                id={`${ids}-cron`}
                className="font-mono"
                spellCheck={false}
                autoComplete="off"
                value={cron}
                onChange={(event) => setCron(event.target.value)}
              />
            </FormField>
            <div>
              <p className="label mb-2">{k.presets}</p>
              <div className="flex flex-wrap gap-2">
                {PRESETS.map((preset) => (
                  <Button
                    key={preset.id}
                    size="sm"
                    variant="quiet"
                    aria-pressed={cron === preset.cron}
                    onClick={() => setCron(preset.cron)}
                  >
                    {k.preset[preset.id]}
                  </Button>
                ))}
              </div>
            </div>
          </div>
        ) : null}

        {setting ? (
          <div aria-live="polite">
            <p className="label mb-1.5">{k.next}</p>
            {preview.isError ? (
              <p role="alert" className="flex items-start gap-2 text-sm text-status-error">
                <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
                {preview.error instanceof ApiError ? preview.error.message : t.common.error}
              </p>
            ) : preview.data ? (
              <ul className="space-y-1 text-sm text-bone">
                {preview.data.next.map((at) => (
                  <li key={at} title={`${at.slice(0, 16).replace('T', ' ')} UTC`}>
                    {moment(at, locale)}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-sm text-bone-faint">{k.nextNone}</p>
            )}
          </div>
        ) : null}

        {save.isError ? (
          <p role="alert" className="text-sm text-status-error">
            {save.error instanceof ApiError ? save.error.message : t.common.actionFailed}
          </p>
        ) : null}
      </div>
    </Dialog>
  )
}

/* ── The history ──────────────────────────────────────────────────────────── */

/** How many runs a page of the history holds: the next page is a press away. */
const PAGE = 50

function History({ polling, live, ruleNames }: { polling: boolean; live: boolean; ruleNames: Record<string, string> }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks
  const [kind, setKind] = useState('')
  const [status, setStatus] = useState('')
  const [by, setBy] = useState('')

  const jobs = useInfiniteQuery({
    queryKey: ['jobs', 'history', kind, status, by],
    // Each page from where the last one ended, counted in what is loaded: a
    // run that begins meanwhile pushes the rest down, and the next page asks
    // for what follows what is already here.
    queryFn: ({ pageParam }) =>
      api.get<JobsResponse>(`/jobs${query({ kind, status, by, limit: PAGE, offset: pageParam })}`),
    initialPageParam: 0,
    getNextPageParam: (last, pages) => {
      const loaded = pages.reduce((sum, page) => sum + page.jobs.length, 0)
      return loaded < last.total ? loaded : undefined
    },
    placeholderData: keepPreviousData,
    // Followed while it is the newest that are on screen. Somebody who has
    // dug into the older pages is reading, not watching — and a poll would
    // ask for every page they opened. Followed live, the stream says when it
    // moved, and nothing is asked meanwhile.
    refetchInterval: (state) =>
      (state.state.data?.pages.length ?? 1) > 1 || live ? false : polling ? 3_000 : 20_000,
  })

  const runs = jobs.data?.pages.flatMap((page) => page.jobs) ?? []
  const total = jobs.data?.pages.at(-1)?.total
  const failedOnly = status === 'failed'
  const filtered = Boolean(kind || status || by)

  // The runs as the administration's other tables: read a page at a time,
  // newest first, so no head orders them — a page put in another order here
  // would say nothing of the runs on the other pages.
  const columns: Column[] = useMemo(
    () => [
      { id: 'when', label: t.admin.runs.colWhen },
      { id: 'status', label: t.admin.runs.colStatus },
      { id: 'kind', label: t.admin.runs.colKind, kind: 'primary', always: true },
      { id: 'detail', label: t.admin.runs.colDetail },
      { id: 'by', label: k.colBy, from: 'lg' },
      { id: 'took', label: k.colTook, align: 'right', from: 'md' },
      { id: 'again', label: k.again, kind: 'actions', always: true, align: 'right' },
    ],
    [t, k],
  )
  const table = useDataTable({ id: 'jobs', columns })

  return (
    // Clipped, not hidden: a hidden overflow would let the table's head
    // scroll away with its rows.
    <Panel label={k.history} className="rise mt-8 overflow-clip" style={{ animationDelay: '240ms' }}>
      <PanelHead
        title={k.history}
        action={
          <span className="flex items-center gap-3">
            {jobs.isFetching && !jobs.isFetchingNextPage ? <Spinner className="size-3.5 text-bone-faint" /> : null}
            {runs.length ? <TableOptions table={table} /> : null}
            <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.count(total, locale)}</span>
          </span>
        }
      />
      <div className="border-b border-rule px-5 py-3">
        {/* The failures, first: what the history is opened for, and a
            sentence's worth of work to find among a screenful of runs that
            went well. */}
        <button
          type="button"
          aria-pressed={failedOnly}
          onClick={() => setStatus(failedOnly ? '' : 'failed')}
          className={cn(
            'inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-full border px-4 text-sm font-medium transition-colors duration-150',
            failedOnly
              ? 'border-vermillion-deep bg-vermillion/10 text-vermillion'
              : 'border-rule-bright text-bone-dim hover:border-vermillion-deep hover:text-vermillion',
          )}
        >
          <Glyph name="alert" className="size-4" />
          {k.failuresOnly}
        </button>
      </div>
      <div className="grid gap-3 border-b border-rule px-5 py-3 sm:grid-cols-3">
        <div className="grid gap-1">
          <label htmlFor="history-task" className="label">
            {k.filterTask}
          </label>
          <Select id="history-task" value={kind} onChange={(event) => setKind(event.target.value)}>
            <option value="">{k.allTasks}</option>
            {[...TASKS, 'refresh.item'].map((id) => (
              <option key={id} value={id}>
                {k.names[id]}
              </option>
            ))}
          </Select>
        </div>
        <div className="grid gap-1">
          <label htmlFor="history-outcome" className="label">
            {k.filterOutcome}
          </label>
          <Select id="history-outcome" value={status} onChange={(event) => setStatus(event.target.value)}>
            <option value="">{k.allOutcomes}</option>
            <option value="running">{t.admin.runs.running}</option>
            <option value="succeeded">{t.admin.runs.succeeded}</option>
            <option value="failed">{t.admin.runs.failed}</option>
            <option value="stopped">{t.admin.runs.stopped}</option>
          </Select>
        </div>
        <div className="grid gap-1">
          <label htmlFor="history-by" className="label">
            {k.filterBy}
          </label>
          <Select id="history-by" value={by} onChange={(event) => setBy(event.target.value)}>
            <option value="">{k.everyone}</option>
            <option value="schedule">{k.theSchedule}</option>
            <option value="person">{k.someone}</option>
          </Select>
        </div>
      </div>

      {jobs.isPending ? (
        <div className="space-y-2 p-4">
          {Array.from({ length: 6 }, (_, index) => (
            <Skeleton key={index} className="h-8 w-full" />
          ))}
        </div>
      ) : jobs.isError ? (
        <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-status-error">
          <Glyph name="alert" className="size-4" />
          {t.admin.runs.loadFailed}
        </p>
      ) : runs.length === 0 ? (
        filtered ? (
          // Nothing failing is the good answer, and says so as one.
          <EmptyState
            title={failedOnly && !kind && !by ? k.noFailures : k.noMatch}
            hint={failedOnly && !kind && !by ? k.noFailuresHint : k.noMatchHint}
          />
        ) : (
          <EmptyState title={t.admin.runs.empty} hint={t.admin.runs.emptyHint} />
        )
      ) : (
        <>
          <JobTable jobs={runs} ruleNames={ruleNames} table={table} />
          {jobs.hasNextPage ? (
            <div className="flex flex-col items-center gap-2 border-t border-rule px-5 py-6">
              <Button onClick={() => void jobs.fetchNextPage()} disabled={jobs.isFetchingNextPage}>
                {jobs.isFetchingNextPage ? <Spinner className="size-4" /> : null}
                {k.loadMore}
              </Button>
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {fmt.count(runs.length, locale)} {t.common.of} {fmt.count(total, locale)}
              </span>
            </div>
          ) : null}
        </>
      )}
    </Panel>
  )
}

/**
 * Runs under the day they were started on, the newest day first, as the
 * reader's own calendar draws it. The runs arrive newest first, so a day is
 * a stretch of them and one pass groups them.
 */
function byDay(jobs: Job[]): { day: string; jobs: Job[] }[] {
  const groups: { day: string; jobs: Job[] }[] = []
  for (const job of jobs) {
    const created = new Date(job.createdAt)
    // A date nobody could read is still a run: under a heading of its own.
    const day = Number.isNaN(created.getTime()) ? '' : fmt.localDay(created)
    const last = groups.at(-1)
    if (last?.day === day) last.jobs.push(job)
    else groups.push({ day, jobs: [job] })
  }
  return groups
}

/** `Today`, `Yesterday`, then `Monday 5 October` — with the year once it is not this one's. */
function dayLabel(day: string, locale: string): string {
  if (!day) return '—'
  const [year, month, date] = day.split('-').map(Number) as [number, number, number]
  const now = new Date()
  const ago = Math.round(
    (Date.UTC(now.getFullYear(), now.getMonth(), now.getDate()) - Date.UTC(year, month - 1, date)) / 86_400_000,
  )
  const words =
    ago === 0 || ago === 1
      ? new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(-ago, 'day')
      : year === now.getFullYear()
        ? fmt.dayHeading(new Date(year, month - 1, date), locale)
        : (fmt.weekdayDate(day, locale) ?? day)
  return words.charAt(0).toLocaleUpperCase(locale) + words.slice(1)
}

/** How long a run took, or has been going. */
function took(job: Job, locale: string): string {
  const start = job.startedAt ? Date.parse(job.startedAt) : NaN
  const end = job.finishedAt ? Date.parse(job.finishedAt) : Date.now()
  if (Number.isNaN(start) || Number.isNaN(end)) return '—'
  const seconds = Math.max(0, Math.round((end - start) / 1000))
  if (seconds < 60) return `${seconds.toLocaleString(locale)} s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes.toLocaleString(locale)} min ${seconds % 60} s`
  return `${Math.floor(minutes / 60).toLocaleString(locale)} h ${minutes % 60} min`
}

/**
 * The rows of the dense table that are not runs — a day's heading, a run's
 * detail open under it — keep the table's roles, as its own rows do, for a
 * table drawn as cards that some engines would stop calling one.
 */
const ROW = { role: 'row' } as const
const CELL = { role: 'cell' } as const

/**
 * The runs under the day each was started on, as the dense table: a row per
 * run, its name the button that opens what it did under it — or Enter on the
 * row — and cards where the table is narrow. The day's heading and the open
 * detail span the row; in a card they are not padded as a run is.
 */
export function JobTable({
  jobs,
  ruleNames = {},
  table,
}: {
  jobs: Job[]
  ruleNames?: Record<string, string>
  table: DataTableState
}) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks
  const queryClient = useQueryClient()
  // The run whose detail is open, under its row: one at a time.
  const [open, setOpen] = useState<string | null>(null)
  const toggle = (id: string) => setOpen((current) => (current === id ? null : id))
  const nameOf = (job: Job) => k.names[job.kind] ?? job.kind
  // What a heading or a detail spans: the columns shown.
  const span = table.columns.filter((column) => table.shows(column.id)).length

  /** The run's name, as the button that opens what it did. */
  const opener = (job: Job, controls: string) => {
    const expanded = open === job.id
    return (
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={expanded ? controls : undefined}
        aria-label={expanded ? k.hideDetail(nameOf(job)) : k.showDetail(nameOf(job))}
        onClick={() => toggle(job.id)}
        className="inline-flex min-h-8 items-center gap-1.5 text-left text-sm text-bone transition-colors duration-150 hover:text-vermillion"
      >
        <Glyph
          name="chevronRight"
          className={cn('size-3.5 shrink-0 text-bone-faint transition-transform duration-200', expanded && 'rotate-90 text-vermillion')}
        />
        <span className="whitespace-nowrap">{nameOf(job)}</span>
      </button>
    )
  }

  // A rule's run, by the rule it ran.
  const ruleOf = (job: Job) => (job.kind === 'import.rules' && job.target ? ruleNames[job.target] : undefined)

  const again = useMutation({
    mutationFn: (job: Job) =>
      job.kind === 'refresh.item' && job.target
        ? api.post(`/items/${encodeURIComponent(job.target)}/refresh`)
        : job.kind === 'import.rules' && job.target
          ? api.post(`/import/rules/${encodeURIComponent(job.target)}/run`)
          : api.post(`/tasks/${encodeURIComponent(job.kind)}/run`),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ['tasks'] })
      void queryClient.invalidateQueries({ queryKey: ['jobs'] })
    },
  })

  const who = (job: Job) =>
    !job.triggeredBy ? k.schedule : job.triggeredBy === 'unknown' ? k.someone : describeIdentity(job.triggeredBy, t).name
  const groups = byDay(jobs)

  return (
    <>
      {again.isError ? (
        <p role="alert" className="flex items-center gap-2 border-b border-rule px-5 py-3 text-sm text-status-error">
          <Glyph name="alert" className="size-4 shrink-0" />
          {again.error instanceof ApiError ? again.error.message : t.common.error}
        </p>
      ) : null}
      <DataTable table={table} label={t.admin.jobs} onOpen={toggle}>
        <THead>
          <Th column="when" />
          <Th column="status" />
          <Th column="kind" />
          <Th column="detail" />
          <Th column="by" />
          <Th column="took" />
          <Th column="again">
            <span className="sr-only">{k.again}</span>
          </Th>
        </THead>
        <TBody>
          {groups.map((group) => (
            <Fragment key={group.day}>
              <tr {...ROW} data-day={group.day} style={{ padding: 0 }}>
                {/* A heading in a cell rather than a header cell: it describes a
                    stretch of rows, which no cell beside it could be read as
                    headed by. */}
                <td {...CELL} colSpan={span} className="col-span-full border-y border-rule bg-ink p-0">
                  <h3 className="flex items-baseline justify-between gap-3 px-5 py-2">
                    <DayHeading day={group.day} jobs={group.jobs} />
                  </h3>
                </td>
              </tr>
              {group.jobs.map((job) => {
                // Anything but a refresh of everything, which asks for a confirmation
                // of its own, from its card; and a batch, which is started from the
                // import page with its lines.
                const rerunnable = job.status !== 'running' && job.kind !== 'refresh.all' && job.kind !== 'import.batch'
                const expanded = open === job.id
                return (
                  <Fragment key={job.id}>
                    <Tr row={job.id} data-job={job.id} className={expanded ? 'bg-ink-high' : undefined}>
                      <Td column="when" className="whitespace-nowrap">
                        <span className="font-mono text-xs text-bone-faint tabular-nums" title={fmt.dateTime(job.createdAt, locale)}>
                          {fmt.relative(job.createdAt, locale)}
                        </span>
                      </Td>
                      <Td column="status">
                        <RunStatus status={job.status} />
                      </Td>
                      <Td column="kind">{opener(job, `job-${job.id}-detail`)}</Td>
                      <Td column="detail" className="w-full max-w-0">
                        {/* Which work, for a run that had one: "refreshed from a
                            provider" three times over said nothing of what. */}
                        {job.work ? (
                          <Link
                            to={`/admin/catalogue/${job.work.id}`}
                            className="block truncate text-sm text-bone underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
                          >
                            {job.work.title}
                          </Link>
                        ) : ruleOf(job) ? (
                          // A rule's run, by the rule's name.
                          <span className="block truncate text-sm text-bone">{ruleOf(job)}</span>
                        ) : null}
                        <span
                          className={job.error ? 'block truncate text-vermillion' : 'block truncate text-bone-dim'}
                          title={job.error ?? job.detail ?? undefined}
                        >
                          {job.error ? describeDetail(job.error, t, locale) : job.detail ? describeDetail(job.detail, t, locale) : job.work ? '' : '—'}
                        </span>
                      </Td>
                      <Td column="by" className="whitespace-nowrap">
                        <span className="text-xs text-bone-dim">{who(job)}</span>
                      </Td>
                      <Td column="took" className="font-mono text-xs whitespace-nowrap text-bone-faint tabular-nums">
                        {took(job, locale)}
                      </Td>
                      <Td column="again">
                        {rerunnable ? (
                          <Button
                            size="sm"
                            variant="quiet"
                            disabled={again.isPending && again.variables?.id === job.id}
                            onClick={() => again.mutate(job)}
                            aria-label={`${k.again}: ${k.names[job.kind] ?? job.kind}`}
                          >
                            {again.isPending && again.variables?.id === job.id ? (
                              <Spinner className="size-4" />
                            ) : (
                              <Glyph name="refresh" className="size-4" />
                            )}
                          </Button>
                        ) : null}
                      </Td>
                    </Tr>
                    {expanded ? (
                      <tr {...ROW} id={`job-${job.id}-detail`} style={{ padding: 0 }}>
                        <td {...CELL} colSpan={span} className="col-span-full border-b border-rule bg-ink p-0">
                          <div className="px-5 py-4">
                            <JobDetailPanel job={job} />
                          </div>
                        </td>
                      </tr>
                    ) : null}
                  </Fragment>
                )
              })}
            </Fragment>
          ))}
        </TBody>
      </DataTable>
    </>
  )
}

/** A day's name, and how many of the runs shown under it failed, if any did. */
function DayHeading({ day, jobs }: { day: string; jobs: Job[] }) {
  const { t, locale } = useI18n()
  const failed = jobs.filter((job) => job.status === 'failed').length

  return (
    <>
      <span className="label text-bone-dim">{dayLabel(day, locale)}</span>
      {failed ? (
        <span className="font-mono text-xs font-medium text-vermillion tabular-nums">
          {t.admin.tasks.dayFailed(failed)}
        </span>
      ) : null}
    </>
  )
}

/**
 * What a run did, under its row: its whole summary where the row cut it
 * short, when it began and ended, and each work it took with how it went.
 */
export function JobDetailPanel({ job }: { job: Job }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks

  const live = useContext(Live)
  const detail = useQuery({
    queryKey: ['job', job.id],
    queryFn: () => api.get<JobDetail>(`/jobs/${encodeURIComponent(job.id)}`),
    // A run under way writes as it goes: asked again every few seconds, or
    // told by the stream when the page follows it live.
    refetchInterval: job.status === 'running' && !live ? 3_000 : false,
  })

  if (detail.isPending) return <Skeleton className="h-16 w-full" />
  if (detail.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-status-error">
        <Glyph name="alert" className="size-4 shrink-0" />
        {t.admin.runs.loadFailed}
      </p>
    )
  }

  const { job: run, entries, entriesTotal } = detail.data
  const note = (text?: string) => (text ? (t.admin.runs.notes[text] ?? text) : '')

  return (
    <div className="grid gap-5 lg:grid-cols-[minmax(0,20rem)_minmax(0,1fr)]">
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
        <dt className="label pt-0.5">{t.admin.runs.colDetail}</dt>
        <dd className={cn('break-words', run.error ? 'text-vermillion' : 'text-bone')}>
          {run.error ? describeDetail(run.error, t, locale) : describeDetail(run.detail, t, locale) || '—'}
        </dd>
        <dt className="label pt-0.5">{k.startedAt}</dt>
        <dd className="font-mono text-xs text-bone-dim tabular-nums">{run.startedAt ? fmt.dateTime(run.startedAt, locale) : '—'}</dd>
        <dt className="label pt-0.5">{k.finishedAt}</dt>
        <dd className="font-mono text-xs text-bone-dim tabular-nums">
          {run.finishedAt ? fmt.dateTime(run.finishedAt, locale) : t.admin.runs.running}
        </dd>
        {run.instance ? (
          <>
            <dt className="label pt-0.5">{k.instance}</dt>
            <dd className="font-mono text-xs break-all text-bone-dim">{run.instance}</dd>
          </>
        ) : null}
      </dl>

      <div className="min-w-0">
        <p className="label mb-2">{k.entries(entriesTotal)}</p>
        {entriesTotal === 0 ? (
          // A sweep that found nothing due took no work: said as such, not
          // as a run that kept no detail.
          <p className="text-sm leading-relaxed text-bone-faint italic">{run.detail === 'nothing was due' ? k.noneDue : k.noEntries}</p>
        ) : (
          <ol className="divide-y divide-rule overflow-hidden rounded-card border border-rule">
            {entries.map((entry) => (
              <li key={entry.id} className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-1 px-3 py-2 sm:grid-cols-[7.5rem_minmax(0,1fr)]">
                <span className="pt-0.5">
                  <EntryOutcome outcome={entry.outcome} />
                </span>
                <span className="min-w-0">
                  <span className="flex items-center gap-2">
                    {entry.kind ? (
                      <Glyph
                        name={entry.kind === 'series' ? 'tv' : 'film'}
                        className="size-3.5 shrink-0 text-bone-faint"
                        title={entry.kind === 'series' ? t.nav.series : t.nav.films}
                      />
                    ) : null}
                    {entry.mediaId && entry.title ? (
                      <Link
                        to={`/admin/catalogue/${entry.mediaId}`}
                        className="truncate text-sm text-bone underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
                      >
                        {entry.title}
                      </Link>
                    ) : (
                      <span className="truncate text-sm text-bone-dim" title={entry.title ? undefined : entry.mediaId}>
                        {entry.title ?? k.goneWork}
                      </span>
                    )}
                  </span>
                  {entry.note ? (
                    <span className={cn('block text-xs break-words', entry.outcome === 'failed' ? 'text-vermillion' : 'text-bone-faint')}>
                      {note(entry.note)}
                    </span>
                  ) : null}
                </span>
              </li>
            ))}
          </ol>
        )}
        {entriesTotal > entries.length ? <p className="mt-2 text-xs text-bone-faint">{k.moreEntries(entriesTotal - entries.length)}</p> : null}
      </div>
    </div>
  )
}

/** How a run's work on one entry ended, as a glyph and a word. */
function EntryOutcome({ outcome }: { outcome: JobEntry['outcome'] }) {
  const { t } = useI18n()
  const word = t.admin.tasks.outcomes[outcome] ?? outcome
  if (outcome === 'failed') {
    return (
      <Chip tone="accent">
        <Glyph name="alert" className="size-3" />
        {word}
      </Chip>
    )
  }
  if (outcome === 'skipped') {
    return (
      <Chip>
        <Glyph name="close" className="size-3" />
        {word}
      </Chip>
    )
  }
  return (
    <Chip>
      <Glyph name="check" className="size-3" />
      {word}
    </Chip>
  )
}

/** Status as a glyph, a word and a colour, in that order of importance. */
export function RunStatus({ status }: { status: Job['status'] }) {
  const { t } = useI18n()

  if (status === 'failed') {
    return (
      <Chip tone="accent">
        <Glyph name="alert" className="size-3" />
        {t.admin.runs.failed}
      </Chip>
    )
  }

  if (status === 'stopped') {
    return (
      <Chip>
        <Glyph name="close" className="size-3" />
        {t.admin.runs.stopped}
      </Chip>
    )
  }

  if (status === 'running') {
    return (
      <Chip tone="provider">
        <Glyph name="clock" className="size-3" />
        {t.admin.runs.running}
      </Chip>
    )
  }

  return (
    <Chip>
      <Glyph name="check" className="size-3" />
      {t.admin.runs.succeeded}
    </Chip>
  )
}
