/**
 * The tasks: what the server does on its own, and what it did.
 *
 * Above, one card per task — what it is for, when it runs, how its last run
 * went, when the next one comes — with the button to run it now, and to stop
 * the one long task that can stop. Below, the history: one row per run rather
 * than per work, filtered by task, outcome, and whether the schedule or a
 * person started it. Both are polled while something runs, and otherwise
 * left alone.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Fragment, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  Glyph,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Select,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Th,
  Tr,
} from '../../components/ui'
import { ApiError, api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { describeIdentity, providerName } from '../../lib/labels'
import type { Dict } from '../../lib/i18n'
import type { Job, JobDetail, JobEntry, JobsResponse, Task, TaskId } from '../../lib/types'

const TASKS: TaskId[] = [
  'refresh.sweep',
  'refresh.all',
  'import.anime',
  'import.imdb',
  'export.nfo',
  'media.store',
  'media.sweep',
  'tls.renew',
]

/**
 * A run's note in words. The server writes its summaries in one fixed form
 * each — `3 refreshed, 0 failed`, `12 works, 340 episodes, 0 failed` — and
 * they are read back here; anything else is shown as it came.
 */
export function describeDetail(detail: string | undefined, t: Dict): string {
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
  // A certificate's end, as the reader's locale writes a date.
  const day = (iso: string | undefined) => (iso ? new Date(`${iso}T00:00:00Z`).toLocaleDateString(undefined, { dateStyle: 'medium', timeZone: 'UTC' }) : '')
  if ((m = detail.match(/^renewed; valid until (\d{4}-\d{2}-\d{2})$/))) return s.renewed(day(m[1]))
  if ((m = detail.match(/^valid until (\d{4}-\d{2}-\d{2}); nothing to do$/))) return s.certValid(day(m[1]))
  // A sync from chosen sources names them by their keys.
  const names = (keys: string | undefined) => (keys ?? '').split(', ').map(providerName).join(', ')
  if ((m = detail.match(/^synced from (.+)$/))) return s.synced(names(m[1]))
  if ((m = detail.match(/^none of (.+) answered$/))) return s.noneAnswered(names(m[1]))
  return t.admin.runs.notes[detail] ?? detail
}

export function Jobs() {
  const { t } = useI18n()

  const tasks = useQuery({
    queryKey: ['tasks'],
    queryFn: () => api.get<Task[]>('/tasks'),
    // Every few seconds while something runs, to follow it; otherwise the
    // next due dates only move by the minute.
    refetchInterval: (q) => (q.state.data?.some((task) => task.running) ? 3_000 : 30_000),
  })
  const running = tasks.data?.some((task) => task.running) ?? false

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.jobs}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.admin.tasks.lead}</p>
      </header>

      {tasks.isPending ? (
        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          {TASKS.map((id) => (
            <Skeleton key={id} className="h-56 w-full" />
          ))}
        </div>
      ) : tasks.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {tasks.error.message}
        </p>
      ) : (
        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          {tasks.data.map((task, index) => (
            <TaskCard key={task.id} task={task} delay={40 + index * 40} />
          ))}
        </div>
      )}

      <History polling={running} />
    </div>
  )
}

/* ── One task ─────────────────────────────────────────────────────────────── */

function TaskCard({ task, delay }: { task: Task; delay: number }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks
  const queryClient = useQueryClient()
  const [confirming, setConfirming] = useState(false)

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

  const schedule =
    task.mode === 'scheduled' && task.everySeconds
      ? k.every(task.everySeconds)
      : task.mode === 'manual'
        ? k.manual
        : k.off
  const blocked = task.blocked && task.blocked !== 'running' ? task.blocked : undefined
  const waiting = task.blocked === 'busy'
  const last = task.last && !task.running ? task.last : undefined

  return (
    <Panel label={k.names[task.id]} className="rise flex flex-col" style={{ animationDelay: `${delay}ms` }}>
      <div className="border-b border-rule px-5 py-4">
        <h2 className="font-display text-xl leading-tight text-bone">{k.names[task.id]}</h2>
        <Chip tone={task.mode === 'off' ? 'neutral' : 'provider'} className="mt-2">
          <Glyph name={task.mode === 'manual' ? 'play' : task.mode === 'off' ? 'power' : 'clock'} className="size-3" />
          {schedule}
        </Chip>
        <p className="mt-2.5 text-xs leading-relaxed text-bone-dim">{k.about[task.id]}</p>
      </div>

      <dl className="grid flex-1 content-start gap-3 px-5 py-4 text-sm">
        {task.running ? (
          <div>
            <dt className="label">{k.last}</dt>
            <dd className="mt-1 space-y-1">
              <Lamp tone="brass">{t.admin.runs.running}</Lamp>
              {task.running.detail ? (
                <p className="font-mono text-xs text-bone-dim tabular-nums">{describeDetail(task.running.detail, t)}</p>
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
                    {describeDetail(last.error ?? last.detail, t)}
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
              {fmt.relative(task.nextAt, locale)}
            </dd>
          </div>
        ) : null}
      </dl>
      {blocked ? (
        <p className="flex items-start gap-2 px-5 pb-4 text-xs text-bone-faint">
          <Glyph name="power" className="mt-0.5 size-3.5 shrink-0" />
          {k.blocked[blocked]}
        </p>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 border-t border-rule px-5 py-3">
        {task.running && task.cancelable ? (
          <Button
            size="sm"
            variant="danger"
            disabled={stop.isPending || stopAsked}
            onClick={() => task.running && stop.mutate(task.running.id)}
          >
            {stop.isPending ? <Spinner className="size-4" /> : <Glyph name="close" className="size-4" />}
            {k.stop}
          </Button>
        ) : (
          <Button
            size="sm"
            variant="primary"
            disabled={Boolean(task.running) || Boolean(blocked) || waiting || run.isPending}
            onClick={() => (task.id === 'refresh.all' ? setConfirming(true) : run.mutate())}
          >
            {run.isPending || task.running ? <Spinner className="size-4" /> : <Glyph name="play" className="size-4" />}
            {k.run}
          </Button>
        )}
        {stopAsked ? <span className="text-xs text-bone-faint">{k.stopAsked}</span> : null}
        {run.isError ? (
          <span role="alert" className="text-xs text-vermillion">
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
    </Panel>
  )
}

/* ── The history ──────────────────────────────────────────────────────────── */

function History({ polling }: { polling: boolean }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks
  const [kind, setKind] = useState('')
  const [status, setStatus] = useState('')
  const [by, setBy] = useState('')

  const jobs = useQuery({
    queryKey: ['jobs', kind, status, by],
    queryFn: () => api.get<JobsResponse>(`/jobs${query({ kind, status, by, limit: 100 })}`),
    placeholderData: (previous) => previous,
    refetchInterval: polling ? 3_000 : 20_000,
  })

  return (
    <Panel label={k.history} className="rise mt-8 overflow-hidden" style={{ animationDelay: '240ms' }}>
      <PanelHead
        title={k.history}
        action={
          <span className="flex items-center gap-3">
            {jobs.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
            <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.count(jobs.data?.total, locale)}</span>
          </span>
        }
      />
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
        <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {t.admin.runs.loadFailed}
        </p>
      ) : jobs.data.jobs.length === 0 ? (
        <EmptyState title={t.admin.runs.empty} hint={t.admin.runs.emptyHint} />
      ) : (
        <JobTable jobs={jobs.data.jobs} />
      )}
    </Panel>
  )
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

export function JobTable({ jobs }: { jobs: Job[] }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks
  const queryClient = useQueryClient()
  // The run whose detail is open, under its row: one at a time.
  const [open, setOpen] = useState<string | null>(null)
  const toggle = (id: string) => setOpen((current) => (current === id ? null : id))
  const nameOf = (job: Job) => k.names[job.kind] ?? job.kind

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

  const again = useMutation({
    mutationFn: (job: Job) =>
      job.kind === 'refresh.item' && job.target
        ? api.post(`/items/${encodeURIComponent(job.target)}/refresh`)
        : api.post(`/tasks/${encodeURIComponent(job.kind)}/run`),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ['tasks'] })
      void queryClient.invalidateQueries({ queryKey: ['jobs'] })
    },
  })

  const who = (job: Job) =>
    !job.triggeredBy ? k.schedule : job.triggeredBy === 'unknown' ? k.someone : describeIdentity(job.triggeredBy, t).name

  return (
    <>
    {again.isError ? (
      <p role="alert" className="flex items-center gap-2 border-b border-rule px-5 py-3 text-sm text-vermillion">
        <Glyph name="alert" className="size-4 shrink-0" />
        {again.error instanceof ApiError ? again.error.message : t.common.error}
      </p>
    ) : null}
    {/* On a phone, one block per run: seven columns do not fit, and a table
        that scrolls sideways hides most of what it holds. */}
    <ul className="divide-y divide-rule md:hidden">
      {jobs.map((job) => (
        <li key={job.id} className="px-4 py-3">
          <div className="space-y-1.5">
            <div className="flex items-center justify-between gap-3">
              {opener(job, `job-${job.id}-detail-narrow`)}
              <RunStatus status={job.status} />
            </div>
            {job.work ? (
              <Link to={`/admin/catalogue/${job.work.id}`} className="block truncate text-sm text-bone underline decoration-rule-bright underline-offset-2">
                {job.work.title}
              </Link>
            ) : null}
            <p className={job.error ? 'text-xs break-words text-vermillion' : 'text-xs text-bone-dim'}>
              {describeDetail(job.error ?? job.detail, t)}
            </p>
            <p className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
              {fmt.relative(job.createdAt, locale)} · {who(job)} · {took(job, locale)}
            </p>
          </div>
          {open === job.id ? (
            <div id={`job-${job.id}-detail-narrow`} className="mt-3 border-t border-rule pt-3">
              <JobDetailPanel job={job} />
            </div>
          ) : null}
        </li>
      ))}
    </ul>
    <div className="hidden md:block">
    <TableScroll label={t.admin.jobs}>
      <table className="w-full min-w-[52rem] border-collapse text-left">
        <thead>
          <tr>
            <Th>{t.admin.runs.colWhen}</Th>
            <Th>{t.admin.runs.colStatus}</Th>
            <Th>{t.admin.runs.colKind}</Th>
            <Th>{t.admin.runs.colDetail}</Th>
            <Th>{k.colBy}</Th>
            <Th className="text-right">{k.colTook}</Th>
            <Th>
              <span className="sr-only">{k.again}</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {jobs.map((job) => {
            // Anything but a refresh of everything, which asks for a confirmation
            // of its own, from its card.
            const rerunnable = job.status !== 'running' && job.kind !== 'refresh.all'
            const expanded = open === job.id
            return (
              <Fragment key={job.id}>
              <Tr className={expanded ? 'bg-ink-high' : undefined}>
                <Td className="whitespace-nowrap">
                  <span className="font-mono text-xs text-bone-faint tabular-nums" title={fmt.dateTime(job.createdAt, locale)}>
                    {fmt.relative(job.createdAt, locale)}
                  </span>
                </Td>
                <Td>
                  <RunStatus status={job.status} />
                </Td>
                <Td>{opener(job, `job-${job.id}-detail`)}</Td>
                <Td className="w-full max-w-0">
                  {/* Which work, for a run that had one: "refreshed from a
                      provider" three times over said nothing of what. */}
                  {job.work ? (
                    <Link
                      to={`/admin/catalogue/${job.work.id}`}
                      className="block truncate text-sm text-bone underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
                    >
                      {job.work.title}
                    </Link>
                  ) : null}
                  <span className={job.error ? 'block truncate text-vermillion' : 'block truncate text-bone-dim'} title={job.error ?? job.detail}>
                    {job.error ? describeDetail(job.error, t) : job.detail ? describeDetail(job.detail, t) : job.work ? '' : '—'}
                  </span>
                </Td>
                <Td className="whitespace-nowrap">
                  <span className="text-xs text-bone-dim">{who(job)}</span>
                </Td>
                <Td className="text-right font-mono text-xs whitespace-nowrap text-bone-faint tabular-nums">
                  {took(job, locale)}
                </Td>
                <Td className="text-right">
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
                <tr id={`job-${job.id}-detail`}>
                  <td colSpan={7} className="border-b border-rule bg-ink px-5 py-4">
                    <JobDetailPanel job={job} />
                  </td>
                </tr>
              ) : null}
              </Fragment>
            )
          })}
        </tbody>
      </table>
    </TableScroll>
    </div>
    </>
  )
}

/**
 * What a run did, under its row: its whole summary where the row cut it
 * short, when it began and ended, and each work it took with how it went.
 */
function JobDetailPanel({ job }: { job: Job }) {
  const { t, locale } = useI18n()
  const k = t.admin.tasks

  const detail = useQuery({
    queryKey: ['job', job.id],
    queryFn: () => api.get<JobDetail>(`/jobs/${encodeURIComponent(job.id)}`),
    // A run under way writes as it goes.
    refetchInterval: job.status === 'running' ? 3_000 : false,
  })

  if (detail.isPending) return <Skeleton className="h-16 w-full" />
  if (detail.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
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
          {run.error ? describeDetail(run.error, t) : describeDetail(run.detail, t) || '—'}
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
