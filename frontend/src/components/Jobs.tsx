import { useMutation, useQuery } from '@tanstack/react-query'

import { api } from '../lib/api'
import type { ExportSummary, JobsResponse } from '../lib/types'
import { Alert, Button, Empty, Mono, Panel, PanelHead, Spinner, Tag } from './ui'

/** What the scheduler has been doing. One row per run, not per item. */
export function Jobs() {
  const jobs = useQuery({
    queryKey: ['jobs'],
    queryFn: () => api.get<JobsResponse>('/jobs?limit=25'),
    refetchInterval: 20_000,
  })

  return (
    <Panel className="reveal" style={{ animationDelay: '160ms' }}>
      <PanelHead
        title="Background work"
        aside={jobs.isFetching ? <Spinner /> : <Mono className="text-faint">{jobs.data?.total ?? 0}</Mono>}
      />
      {jobs.isPending ? (
        <div className="px-5 py-10 text-center">
          <Spinner />
        </div>
      ) : jobs.isError ? (
        <div className="p-5">
          <Alert>Could not load the job history.</Alert>
        </div>
      ) : jobs.data.jobs.length === 0 ? (
        <Empty
          title="Nothing has run yet"
          hint="A sweep is recorded each time the scheduler wakes, and a hand-triggered refresh gets its own row."
        />
      ) : (
        <ul className="divide-y divide-line">
          {jobs.data.jobs.map((job) => (
            <li key={job.id} className="edge flex items-baseline gap-3 px-5 py-2.5">
              <Mono className="w-[122px] shrink-0 text-[11px] text-faint">
                {new Date(job.createdAt).toLocaleString(undefined, {
                  dateStyle: 'short',
                  timeStyle: 'short',
                })}
              </Mono>
              <Tag
                tone={
                  job.status === 'failed' ? 'bad' : job.status === 'running' ? 'auto' : 'good'
                }
              >
                {job.status}
              </Tag>
              <Mono className="text-[11px] text-signal">{job.kind}</Mono>
              <span className="min-w-0 flex-1 truncate text-[12px] text-dim">
                {job.error ?? job.detail ?? ''}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  )
}

/** Write a `.nfo` document for everything, for a library Plex reads. */
export function NfoExport() {
  const run = useMutation({ mutationFn: () => api.post<ExportSummary>('/export/nfo') })

  return (
    <div className="flex flex-wrap items-center justify-between gap-4 p-5">
      <div className="min-w-0">
        <p className="text-[14px] text-paper">Export every entry as .nfo</p>
        <p className="mt-1 max-w-prose text-[12px] leading-relaxed text-faint">
          Writes Kodi/XBMC documents under <Mono className="text-dim">AMS_NFO_EXPORT_PATH</Mono>,
          which is how your edits reach Plex — it has no configurable metadata source. If Sonarr or
          Radarr manage the library, enabling <em>their</em> Kodi metadata writer is simpler: they
          already write these files beside the media, from what this server gave them.
        </p>
        {run.isSuccess && (
          <p className="mt-2 font-mono text-[12px] text-sage">
            {run.data.works} works, {run.data.episodes} episodes → {run.data.root}
            {run.data.failed > 0 && ` (${run.data.failed} failed)`}
          </p>
        )}
        {run.isError && (
          <p className="mt-2 text-[12px] text-rust">
            {run.error.message.includes('TMDB API key') ||
            run.error.message.includes('provider_not_configured')
              ? 'AMS_NFO_EXPORT_PATH is not set.'
              : run.error.message}
          </p>
        )}
      </div>
      <Button variant="primary" onClick={() => run.mutate()} disabled={run.isPending}>
        {run.isPending ? <Spinner className="border-void/40 border-t-void" /> : 'Export'}
      </Button>
    </div>
  )
}
