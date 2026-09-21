/**
 * What the scheduler has been doing.
 *
 * One row per run rather than per work: a sweep that refreshed forty entries is
 * one thing that happened, and forty rows would hide the one that failed.
 */

import { useQuery } from '@tanstack/react-query'

import {
  Chip,
  EmptyState,
  Glyph,
  Label,
  Panel,
  PanelHead,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Th,
  Tr,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { Job, JobsResponse } from '../../lib/types'

export function Jobs() {
  const { t, locale } = useI18n()

  const jobs = useQuery({
    queryKey: ['jobs'],
    queryFn: () => api.get<JobsResponse>(`/jobs${query({ limit: 50 })}`),
    refetchInterval: 20_000,
  })

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {t.admin.jobs}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.admin.runs.lead}</p>
      </header>

      <Panel className="rise overflow-hidden" style={{ animationDelay: '60ms' }}>
        <PanelHead
          title={t.admin.overview.recentRuns}
          action={
            <span className="flex items-center gap-3">
              {jobs.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {fmt.count(jobs.data?.total, locale)}
              </span>
            </span>
          }
        />

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
    </div>
  )
}

export function JobTable({ jobs }: { jobs: Job[] }) {
  const { t, locale } = useI18n()

  return (
    <TableScroll>
      <table className="w-full min-w-[40rem] border-collapse text-left">
        <thead>
          <tr>
            <Th>{t.admin.runs.colWhen}</Th>
            <Th>{t.admin.runs.colStatus}</Th>
            <Th>{t.admin.runs.colKind}</Th>
            <Th>{t.admin.runs.colDetail}</Th>
          </tr>
        </thead>
        <tbody>
          {jobs.map((job) => (
            <Tr key={job.id}>
              <Td className="whitespace-nowrap">
                <span
                  className="font-mono text-xs text-bone-faint tabular-nums"
                  title={fmt.dateTime(job.createdAt, locale)}
                >
                  {fmt.relative(job.createdAt, locale)}
                </span>
              </Td>
              <Td>
                <RunStatus status={job.status} />
              </Td>
              <Td>
                <span className="font-mono text-xs whitespace-nowrap text-slate">{job.kind}</span>
              </Td>
              <Td className="w-full max-w-0">
                <span className={job.error ? 'block truncate text-vermillion' : 'block truncate'}>
                  {job.error ?? job.detail ?? '—'}
                </span>
              </Td>
            </Tr>
          ))}
        </tbody>
      </table>
    </TableScroll>
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
