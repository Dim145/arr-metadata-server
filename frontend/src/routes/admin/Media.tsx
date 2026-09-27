/**
 * The media kept: how the store stands, what it keeps, and what it could
 * not fetch.
 *
 * Numbers first, since they say whether the thing works at all; then the
 * switches, each with what it costs; then the two tasks; then the failures,
 * each with its reason and a way to try again. When nothing is configured
 * the page says what to set, and nothing else — the switches would only
 * pretend.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  Dialog,
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
  Toggle,
} from '../../components/ui'
import { ApiError, api, query } from '../../lib/api'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { MediaAsset, MediaStatus, TaskId } from '../../lib/types'

/** An address as a row shows it: its host, and the end of its path. */
function shortAddress(origin: string): string {
  if (origin.startsWith('upload:')) return origin
  try {
    const url = new URL(origin)
    const tail = url.pathname.length > 40 ? `…${url.pathname.slice(-38)}` : url.pathname
    return `${url.host}${tail}`
  } catch {
    return origin
  }
}

export function Media() {
  const { t, locale } = useI18n()
  const p = t.admin.mediaPage
  const queryClient = useQueryClient()
  // One line under the buttons says what the last action did.
  const [notice, setNotice] = useState<string | null>(null)
  const [asking, setAsking] = useState(false)

  const status = useQuery({
    queryKey: ['media', 'status'],
    queryFn: () => api.get<MediaStatus>('/media/status'),
    // Live while something runs: the counts are the progress.
    refetchInterval: (q) => (q.state.data?.fetching || q.state.data?.sweeping ? 3_000 : 30_000),
  })
  const on = status.data?.backend !== undefined && status.data.backend !== 'off'

  const troubled = useQuery({
    queryKey: ['media', 'troubled'],
    queryFn: () => api.get<MediaAsset[]>(`/media/troubled${query({ limit: 100 })}`),
    enabled: on,
    refetchInterval: 30_000,
  })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['media'] })
    void queryClient.invalidateQueries({ queryKey: ['tasks'] })
  }

  const set = useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) =>
      api.put('/settings/server/-', { key, value }),
    onSuccess: invalidate,
  })

  const run = useMutation({
    mutationFn: (id: TaskId) => api.post<{ jobId?: string }>(`/tasks/${id}/run`),
    onMutate: () => setNotice(null),
    onSuccess: () => {
      setNotice(p.started)
      invalidate()
    },
  })

  const retry = useMutation({
    mutationFn: (id: string) => api.post(`/media/assets/${id}/retry`),
    onSuccess: invalidate,
  })
  const retryAll = useMutation({
    mutationFn: () => api.post<{ retried: number }>('/media/retry'),
    onSuccess: invalidate,
  })
  const reset = useMutation({
    mutationFn: () => api.post<{ forgotten: number }>('/media/reset'),
    onSuccess: (data) => {
      setAsking(false)
      setNotice(p.resetDone(data.forgotten))
      invalidate()
    },
  })

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.mediaNav}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      </header>

      {status.isPending ? (
        <Skeleton className="h-40 w-full" />
      ) : status.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {p.loadFailed}
        </p>
      ) : (
        <div className="space-y-6">
          <Panel className="rise" style={{ animationDelay: '60ms' }}>
            <PanelHead
              title={p.store}
              action={
                <Chip tone={on ? 'provider' : 'neutral'}>
                  <Glyph name={on ? 'database' : 'cloud'} className="size-3" />
                  {p.backend[status.data.backend] ?? status.data.backend}
                </Chip>
              }
            />
            {on ? (
              <>
                <dl className="grid grid-cols-2 gap-px bg-rule sm:grid-cols-4">
                  {(
                    [
                      ['stored', fmt.count(status.data.counts.stored, locale)],
                      ['pending', fmt.count(status.data.counts.pending, locale)],
                      ['failed', fmt.count(status.data.counts.failed, locale)],
                      ['bytes', fmt.bytes(status.data.counts.bytes, locale)],
                    ] as const
                  ).map(([key, value]) => (
                    <div key={key} className="bg-ink-raised px-5 py-4">
                      <dt className="label">{p.counts[key]}</dt>
                      <dd className="mt-1 font-display text-2xl text-bone tabular-nums">{value}</dd>
                    </div>
                  ))}
                </dl>
                {(status.data.fetching || status.data.sweeping || !status.data.publicUrl) ? (
                  <div className="space-y-2 border-t border-rule px-5 py-3 text-sm">
                    {status.data.fetching ? <Lamp tone="vermillion">{p.fetching}</Lamp> : null}
                    {status.data.sweeping ? <Lamp tone="vermillion">{p.sweeping}</Lamp> : null}
                    {!status.data.publicUrl ? (
                      <p className="flex items-start gap-2 text-bone-dim">
                        <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-brass" />
                        {p.noPublicUrl}
                      </p>
                    ) : null}
                  </div>
                ) : null}
                {/* Not a task: the store's own undoing, kept apart from
                    the routine. */}
                <div className="flex flex-wrap items-center gap-3 border-t border-rule px-5 py-3">
                  <Button variant="danger" size="sm" disabled={status.data.fetching} onClick={() => setAsking(true)}>
                    <Glyph name="cloud" className="size-4" />
                    {p.reset}
                  </Button>
                  <p className="text-xs leading-relaxed text-bone-faint">{p.resetHint}</p>
                </div>
              </>
            ) : (
              <p className="px-5 py-4 text-sm leading-relaxed text-bone-dim">{p.offHint}</p>
            )}
          </Panel>

          {on ? (
            <>
              <Panel className="rise" style={{ animationDelay: '100ms' }}>
                <PanelHead title={p.settings} />
                <ul className="divide-y divide-rule">
                  {(
                    [
                      ['media.store', p.storeOn, p.storeOnHint, status.data.storing],
                      ['media.people', p.people, p.peopleHint, status.data.people],
                      ['media.audio', p.audio, p.audioHint, status.data.audio],
                    ] as const
                  ).map(([key, label, hint, checked]) => (
                    <li key={key} className="flex items-start gap-4 px-5 py-3">
                      <div className="min-w-0 flex-1">
                        <label htmlFor={`media-${key}`} className="block text-sm text-bone">
                          {label}
                        </label>
                        <p className="mt-0.5 text-xs leading-relaxed text-bone-faint">{hint}</p>
                      </div>
                      <Toggle
                        id={`media-${key}`}
                        checked={checked}
                        label={label}
                        disabled={set.isPending}
                        onChange={(next) => set.mutate({ key, value: String(next) })}
                      />
                    </li>
                  ))}
                  {status.data.backend === 's3' ? (
                    <li className="px-5 py-3">
                      <label htmlFor="media-serve" className="block text-sm text-bone">
                        {p.serve}
                      </label>
                      <Select
                        id="media-serve"
                        className="mt-2 w-auto min-w-64"
                        value={status.data.serve}
                        disabled={set.isPending}
                        onChange={(event) => set.mutate({ key: 'media.serve', value: event.target.value })}
                      >
                        <option value="proxy">{p.serveProxy}</option>
                        <option value="redirect">{p.serveRedirect}</option>
                      </Select>
                      <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">{p.serveHint}</p>
                    </li>
                  ) : null}
                </ul>
                {set.isError ? (
                  <p role="alert" className="border-t border-rule px-5 py-3 text-sm text-vermillion">
                    {set.error instanceof ApiError ? set.error.message : t.common.actionFailed}
                  </p>
                ) : null}
              </Panel>

              <Panel className="rise" style={{ animationDelay: '140ms' }}>
                <PanelHead
                  title={p.tasks}
                  action={
                    <Link to="/admin/jobs" className="label transition-colors duration-150 hover:text-vermillion">
                      {t.admin.jobs}
                    </Link>
                  }
                />
                <div className="flex flex-wrap items-center gap-3 px-5 py-4">
                  <Button
                    variant="primary"
                    disabled={run.isPending || status.data.fetching}
                    onClick={() => run.mutate('media.store')}
                  >
                    {run.isPending && run.variables === 'media.store' ? <Spinner className="size-4" /> : <Glyph name="download" className="size-4" />}
                    {p.storeAll}
                  </Button>
                  <Button disabled={run.isPending || status.data.sweeping} onClick={() => run.mutate('media.sweep')}>
                    {run.isPending && run.variables === 'media.sweep' ? <Spinner className="size-4" /> : <Glyph name="trash" className="size-4" />}
                    {p.sweepNow}
                  </Button>
                  <p role="status" className="text-sm text-bone-dim">
                    {run.isError
                      ? run.error instanceof ApiError
                        ? run.error.message
                        : t.common.actionFailed
                      : (notice ?? '')}
                  </p>
                </div>
              </Panel>

              <Dialog
                open={asking}
                title={p.resetTitle}
                onClose={() => setAsking(false)}
                footer={
                  <>
                    <Button onClick={() => setAsking(false)}>{t.common.cancel}</Button>
                    <Button variant="danger" disabled={reset.isPending} onClick={() => reset.mutate()}>
                      {reset.isPending ? <Spinner className="size-4" /> : <Glyph name="cloud" className="size-4" />}
                      {p.reset}
                    </Button>
                  </>
                }
              >
                {p.resetBody}
                {reset.isError ? (
                  <p role="alert" className="mt-3 text-sm text-vermillion">
                    {reset.error instanceof ApiError ? reset.error.message : t.common.actionFailed}
                  </p>
                ) : null}
              </Dialog>

              <Panel className="rise" style={{ animationDelay: '180ms' }}>
                <PanelHead
                  title={p.troubled}
                  action={
                    troubled.data?.length ? (
                      <Button size="sm" disabled={retryAll.isPending} onClick={() => retryAll.mutate()}>
                        <Glyph name="refresh" className="size-4" />
                        {p.retryAll}
                      </Button>
                    ) : undefined
                  }
                />
                <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">{p.troubledLead}</p>
                <div role="status" className="px-5 text-sm text-moss">
                  {retryAll.isSuccess ? p.retried(retryAll.data.retried) : ''}
                </div>
                {troubled.isPending ? (
                  <Skeleton className="m-5 h-24" />
                ) : !troubled.data?.length ? (
                  <p className="px-5 py-4 text-sm text-bone-faint">{p.none}</p>
                ) : (
                  <>
                    <div className="hidden md:block">
                      <TableScroll label={p.troubled}>
                        <table className="w-full border-collapse text-left text-sm">
                        <thead>
                          <tr>
                            <Th>{p.colAddress}</Th>
                            <Th>{p.colKind}</Th>
                            <Th align="right">{p.colAttempts}</Th>
                            <Th>{p.colError}</Th>
                            <Th>
                              <span className="sr-only">{p.retry}</span>
                            </Th>
                          </tr>
                        </thead>
                        <tbody>
                          {troubled.data.map((asset) => (
                            <tr key={asset.id} className="border-t border-rule">
                              <Td>
                                <span className="block max-w-xs truncate font-mono text-xs" title={asset.origin}>
                                  {shortAddress(asset.origin)}
                                </span>
                                {asset.wantedBy ? (
                                  <Link
                                    to={`/admin/catalogue/${asset.wantedBy}`}
                                    className="text-xs text-vermillion underline-offset-4 hover:underline"
                                  >
                                    {p.openWork}
                                  </Link>
                                ) : null}
                              </Td>
                              <Td>{p.kinds[asset.kind] ?? asset.kind}</Td>
                              <Td align="right">
                                <span className="font-mono text-xs tabular-nums">{asset.attempts}</span>
                              </Td>
                              <Td>
                                <span className="block max-w-md text-xs break-words text-bone-dim">{asset.error ?? '—'}</span>
                              </Td>
                              <Td align="right">
                                <Button size="sm" disabled={retry.isPending} onClick={() => retry.mutate(asset.id)}>
                                  {p.retry}
                                </Button>
                              </Td>
                            </tr>
                          ))}
                        </tbody>
                        </table>
                      </TableScroll>
                    </div>
                    <ul className="divide-y divide-rule md:hidden">
                      {troubled.data.map((asset) => (
                        <li key={asset.id} className="px-5 py-3">
                          <p className="truncate font-mono text-xs text-bone" title={asset.origin}>
                            {shortAddress(asset.origin)}
                          </p>
                          <p className="mt-1 text-xs text-bone-dim">
                            {p.kinds[asset.kind] ?? asset.kind} · {p.colAttempts} {asset.attempts}
                          </p>
                          {asset.error ? <p className="mt-1 text-xs break-words text-bone-faint">{asset.error}</p> : null}
                          <div className="mt-2 flex flex-wrap items-center gap-3">
                            <Button size="sm" disabled={retry.isPending} onClick={() => retry.mutate(asset.id)}>
                              {p.retry}
                            </Button>
                            {asset.wantedBy ? (
                              <Link
                                to={`/admin/catalogue/${asset.wantedBy}`}
                                className="text-xs text-vermillion underline-offset-4 hover:underline"
                              >
                                {p.openWork}
                              </Link>
                            ) : null}
                          </div>
                        </li>
                      ))}
                    </ul>
                  </>
                )}
              </Panel>
            </>
          ) : null}
        </div>
      )}
    </div>
  )
}
