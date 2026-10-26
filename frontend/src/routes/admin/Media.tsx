/**
 * The media kept: how the store stands, what it keeps, and what it could
 * not fetch.
 *
 * Numbers first, since they say whether the thing works at all; then the
 * switches, each with what it costs; then the two tasks; then the failures,
 * each with its reason and a way to try again; then the works by what their
 * media come to, the heaviest first. When nothing is configured the page
 * says what to set, and nothing else — the switches would only pretend.
 */

import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useMemo, useState } from 'react'
import { Link, useNavigate } from 'react-router'

import {
  Button,
  Chip,
  DataTable,
  Dialog,
  EmptyState,
  Glyph,
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
  Toggle,
  Tr,
  sortRows,
  useDataTable,
  type Column,
} from '../../components/ui'
import { ApiError, api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { MediaAsset, MediaByWork, MediaStatus, MediaVariantFormat, TaskId, VariantsDeleted } from '../../lib/types'

/** The switches that have a format's copies made, which can be deleted once they are off. */
const FORMAT_OF: Partial<Record<string, MediaVariantFormat>> = { 'media.webp': 'webp', 'media.avif': 'avif' }
const SETTING_OF: Record<MediaVariantFormat, string> = { webp: 'media.webp', avif: 'media.avif' }
const FORMAT_NAME: Record<MediaVariantFormat, string> = { webp: 'WebP', avif: 'AVIF' }

/** How many copies of a format are kept, and what they weigh. */
function copiesOf(variants: MediaStatus['variants'], format: MediaVariantFormat): { copies: number; bytes: number } {
  return format === 'webp'
    ? { copies: variants.webp, bytes: variants.webpBytes }
    : { copies: variants.avif, bytes: variants.avifBytes }
}

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
  // The format being turned off while copies of it are kept, as the page
  // asks what becomes of them; and the deletion the last answer started.
  const [turningOff, setTurningOff] = useState<MediaVariantFormat | null>(null)
  const [purged, setPurged] = useState<VariantsDeleted | null>(null)

  const status = useQuery({
    queryKey: ['media', 'status'],
    queryFn: () => api.get<MediaStatus>('/media/status'),
    // Live while something runs: the counts are the progress.
    refetchInterval: (q) => {
      const now = q.state.data
      return now?.fetching || now?.sweeping || now?.preparing || now?.purging ? 3_000 : 30_000
    },
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
  // What a switch changes: the figures here, and — the WebP copies' — what
  // this browser's pages are given, which the server's options say.
  const changed = (key: string) => {
    invalidate()
    if (key === 'media.webp') void queryClient.invalidateQueries({ queryKey: ['auth', 'options'] })
  }

  const set = useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) =>
      api.put('/settings/server/-', { key, value }),
    onSuccess: (_, { key }) => changed(key),
  })

  // Every copy of a format deleted, in the background on the server, which
  // answers at once with what there was. Asked once more a second later
  // when it is refused for now (409): of several instances, the one asked
  // may not have heard yet that the format was turned off.
  const purge = useMutation({
    mutationFn: async (format: MediaVariantFormat) => {
      const path = `/media/variants/${format}`
      try {
        return await api.delete<VariantsDeleted>(path)
      } catch (error) {
        if (!(error instanceof ApiError) || error.status !== 409) throw error
        await new Promise((resolve) => window.setTimeout(resolve, 1_000))
        return await api.delete<VariantsDeleted>(path)
      }
    },
    onSuccess: (deleted) => {
      setTurningOff(null)
      setPurged(deleted)
      invalidate()
    },
  })

  // A format turned off from the dialog — the server deletes none of its
  // copies while it is on — and then, where that was the answer, its
  // copies deleted.
  const turnOff = useMutation({
    mutationFn: ({ format }: { format: MediaVariantFormat; purge: boolean }) =>
      api.put('/settings/server/-', { key: SETTING_OF[format], value: 'false' }),
    onSuccess: (_, { format, purge: deleting }) => {
      changed(SETTING_OF[format])
      if (deleting) purge.mutate(format)
      else setTurningOff(null)
    },
  })

  // A switch pressed. Turning a format's copies off while some are kept
  // asks first whether to delete them — counted again before it asks, as
  // the pages have copies made between two looks at the store; anything
  // else is done at once.
  const flip = (key: string, next: boolean) => {
    const format = FORMAT_OF[key]
    if (!format || next) {
      set.mutate({ key, value: String(next) })
      return
    }
    void status.refetch().then(({ data }) => {
      if (data && copiesOf(data.variants, format).copies > 0) {
        turnOff.reset()
        purge.reset()
        setTurningOff(format)
      } else {
        set.mutate({ key, value: 'false' })
      }
    })
  }
  const answering = turnOff.isPending || purge.isPending
  const asked = turningOff && status.data ? { format: turningOff, ...copiesOf(status.data.variants, turningOff) } : null

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
  // The failures, as a dense table: their columns, ordered in the page.
  const troubledColumns: Column[] = useMemo(
    () => [
      { id: 'address', label: p.colAddress, kind: 'primary', always: true },
      { id: 'kind', label: p.colKind, sort: 'asc' },
      { id: 'attempts', label: p.colAttempts, align: 'right', sort: 'desc' },
      { id: 'error', label: p.colError },
      { id: 'retry', label: p.retry, kind: 'actions', always: true, align: 'right' },
    ],
    [p],
  )
  const troubledTable = useDataTable({ id: 'media-troubled', columns: troubledColumns })
  const troubledRows = sortRows(
    troubled.data ?? [],
    troubledTable.sort,
    { kind: (asset) => p.kinds[asset.kind] ?? asset.kind, attempts: (asset) => asset.attempts },
    locale,
  )

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
        <p role="alert" className="flex items-center gap-2 text-sm text-status-error">
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
                {/* The copies the pages are given, made of what is kept: what
                    each kind weighs. */}
                <p className="border-t border-rule px-5 py-3 text-sm text-bone-dim">
                  <span className="label mr-2">{p.copies}</span>
                  <span className="font-mono text-xs tabular-nums">
                    {p.copiesLine(
                      p.copiesOf(fmt.count(status.data.variants.webp, locale), fmt.bytes(status.data.variants.webpBytes, locale)),
                      p.copiesOf(fmt.count(status.data.variants.avif, locale), fmt.bytes(status.data.variants.avifBytes, locale)),
                      p.copiesOf(fmt.count(status.data.variants.jpeg, locale), fmt.bytes(status.data.variants.jpegBytes, locale)),
                    )}
                  </span>
                </p>
                {(status.data.fetching || status.data.sweeping || status.data.preparing || status.data.purging || !status.data.publicUrl) ? (
                  <div className="space-y-2 border-t border-rule px-5 py-3 text-sm">
                    {status.data.fetching ? <Lamp tone="vermillion">{p.fetching}</Lamp> : null}
                    {status.data.sweeping ? <Lamp tone="vermillion">{p.sweeping}</Lamp> : null}
                    {status.data.preparing ? <Lamp tone="vermillion">{p.preparing}</Lamp> : null}
                    {status.data.purging ? <Lamp tone="vermillion">{p.purging}</Lamp> : null}
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
                      ['media.webp', p.webp, p.webpHint, status.data.webp],
                      ['media.avif', p.avif, p.avifHint, status.data.avif],
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
                        disabled={set.isPending || answering}
                        onChange={(next) => flip(key, next)}
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
                {/* What the last deletion started on: the lamp above says
                    while it runs, and the copies' line counts it down. */}
                <p role="status" className={cn('px-5 text-sm text-bone-dim', purged && 'border-t border-rule py-3')}>
                  {purged
                    ? p.turnOff.deleting(
                        purged.copies,
                        fmt.count(purged.copies, locale),
                        FORMAT_NAME[purged.format],
                        fmt.bytes(purged.bytes, locale),
                      )
                    : ''}
                </p>
                {set.isError ? (
                  <p role="alert" className="border-t border-rule px-5 py-3 text-sm text-status-error">
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
                {/* What an earlier version stored and never prepared: the
                    task is the administrator's to run, never the start's. */}
                <div className="flex flex-wrap items-center gap-3 border-t border-rule px-5 py-4">
                  {status.data.unprepared > 0 ? (
                    <>
                      {/* Without the WebP copies, preparing a picture is
                          working out its placeholder alone. */}
                      <p className="max-w-prose flex-1 text-sm leading-relaxed text-bone-dim">
                        {status.data.webp ? p.unprepared(status.data.unprepared) : p.unpreparedWhole(status.data.unprepared)}
                      </p>
                      <Button
                        disabled={run.isPending || status.data.preparing}
                        onClick={() => run.mutate('media.variants')}
                      >
                        {run.isPending && run.variables === 'media.variants' ? (
                          <Spinner className="size-4" />
                        ) : (
                          <Glyph name="image" className="size-4" />
                        )}
                        {p.prepareNow}
                      </Button>
                    </>
                  ) : (
                    <p className="text-sm text-bone-faint">{p.allPrepared}</p>
                  )}
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
                  <p role="alert" className="mt-3 text-sm text-status-error">
                    {reset.error instanceof ApiError ? reset.error.message : t.common.actionFailed}
                  </p>
                ) : null}
              </Dialog>

              {/* A format's copies turned off while some are kept: kept, or
                  deleted to have the space back. Cancelled, nothing changes. */}
              <Dialog
                open={asked !== null}
                title={asked ? p.turnOff.title(FORMAT_NAME[asked.format]) : ''}
                onClose={() => setTurningOff(null)}
                footer={
                  asked ? (
                    <>
                      <Button onClick={() => setTurningOff(null)}>{t.common.cancel}</Button>
                      <Button disabled={answering} onClick={() => turnOff.mutate({ format: asked.format, purge: false })}>
                        {turnOff.isPending && !turnOff.variables.purge ? <Spinner className="size-4" /> : null}
                        {p.turnOff.keep(asked.copies)}
                      </Button>
                      <Button
                        variant="danger"
                        disabled={answering}
                        onClick={() => turnOff.mutate({ format: asked.format, purge: true })}
                      >
                        {purge.isPending || (turnOff.isPending && turnOff.variables.purge) ? (
                          <Spinner className="size-4" />
                        ) : (
                          <Glyph name="trash" className="size-4" />
                        )}
                        {p.turnOff.delete(asked.copies)}
                      </Button>
                    </>
                  ) : null
                }
              >
                {asked
                  ? p.turnOff[asked.format](asked.copies, fmt.count(asked.copies, locale), fmt.bytes(asked.bytes, locale))
                  : null}
                {turnOff.isError ? (
                  <p role="alert" className="mt-3 text-sm text-status-error">
                    {turnOff.error instanceof ApiError ? turnOff.error.message : t.common.actionFailed}
                  </p>
                ) : purge.isError ? (
                  <p role="alert" className="mt-3 text-sm text-status-error">
                    {p.turnOff.kept(purge.error instanceof ApiError ? purge.error.message : t.common.actionFailed)}
                  </p>
                ) : null}
              </Dialog>

              <Panel className="rise" style={{ animationDelay: '180ms' }}>
                <PanelHead
                  title={p.troubled}
                  action={
                    troubled.data?.length ? (
                      <span className="flex flex-wrap items-center gap-2">
                        <TableOptions table={troubledTable} />
                        <Button size="sm" disabled={retryAll.isPending} onClick={() => retryAll.mutate()}>
                          <Glyph name="refresh" className="size-4" />
                          {p.retryAll}
                        </Button>
                      </span>
                    ) : undefined
                  }
                />
                <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">{p.troubledLead}</p>
                <div role="status" className="px-5 text-sm text-status-ok">
                  {retryAll.isSuccess ? p.retried(retryAll.data.retried) : ''}
                </div>
                {troubled.isPending ? (
                  <Skeleton className="m-5 h-24" />
                ) : !troubled.data?.length ? (
                  <EmptyState size="line" title={p.none} />
                ) : (
                  <DataTable table={troubledTable} label={p.troubled}>
                    <THead>
                      <Th column="address" />
                      <Th column="kind" />
                      <Th column="attempts" />
                      <Th column="error" />
                      <Th column="retry">
                        <span className="sr-only">{p.retry}</span>
                      </Th>
                    </THead>
                    <TBody>
                      {troubledRows.map((asset) => (
                        <Tr key={asset.id} row={asset.id}>
                          <Td column="address" className="w-full max-w-0">
                            <span className="block truncate font-mono text-xs text-bone" title={asset.origin}>
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
                          <Td column="kind">{p.kinds[asset.kind] ?? asset.kind}</Td>
                          <Td column="attempts">
                            <span className="font-mono text-xs tabular-nums">{asset.attempts}</span>
                          </Td>
                          <Td column="error">
                            <span className="block max-w-md text-xs break-words text-bone-dim">{asset.error ?? '—'}</span>
                          </Td>
                          <Td column="retry">
                            <Button size="sm" disabled={retry.isPending} onClick={() => retry.mutate(asset.id)}>
                              {p.retry}
                            </Button>
                          </Td>
                        </Tr>
                      ))}
                    </TBody>
                  </DataTable>
                )}
              </Panel>

              <ByWork />
            </>
          ) : null}
        </div>
      )}
    </div>
  )
}

/** How the works are ranked by their media. */
type Order = 'space' | 'count' | 'failed'

/** The column that ranks the works each way, and the way each column ranks them. */
const ORDER_COLUMN: Record<Order, string> = { space: 'bytes', count: 'stored', failed: 'failed' }
const COLUMN_ORDER: Record<string, Order> = { bytes: 'space', stored: 'count', failed: 'failed' }

/** How many works a page of the ranking holds. */
const PAGE = 50

/**
 * The works by what their media come to: how many each keeps, how many wait
 * or were given up on, and what those kept weigh with the copies made of
 * them — the heaviest first, or the most kept, or the most given up on. A
 * picture two works share counts for each, as each one's media list shows it.
 *
 * Ranked by the server, a page at a time: by the switch above the table, or
 * by its column heads where the table is wide enough to show them — always
 * the most first, which is what the ranking is for. Enter on a row opens the
 * work in the editor.
 */
function ByWork() {
  const { t, locale } = useI18n()
  const w = t.admin.mediaPage.works
  const navigate = useNavigate()
  const [order, setOrder] = useState<Order>('space')

  const columns: Column[] = useMemo(
    () => [
      { id: 'work', label: w.colWork, kind: 'primary', always: true },
      { id: 'stored', label: w.colKept, align: 'right', sort: 'desc' },
      { id: 'pending', label: w.colWaiting, align: 'right' },
      { id: 'failed', label: w.colFailed, align: 'right', sort: 'desc' },
      { id: 'bytes', label: w.colSize, align: 'right', sort: 'desc' },
      { id: 'copies', label: w.colCopies, align: 'right', from: 'md' },
    ],
    [w],
  )
  const table = useDataTable({
    id: 'media-works',
    columns,
    sort: { column: ORDER_COLUMN[order], direction: 'desc' },
    onSort: (next) => setOrder(COLUMN_ORDER[next.column] ?? 'space'),
  })

  const works = useInfiniteQuery({
    queryKey: ['media', 'works', order],
    queryFn: ({ pageParam }) => api.get<MediaByWork>(`/media/works${query({ sort: order, limit: PAGE, offset: pageParam })}`),
    initialPageParam: 0,
    getNextPageParam: (last, pages) => {
      const loaded = pages.reduce((sum, page) => sum + page.works.length, 0)
      return loaded < last.total ? loaded : undefined
    },
    placeholderData: keepPreviousData,
    staleTime: 60_000,
  })
  const rows = works.data?.pages.flatMap((page) => page.works) ?? []
  const total = works.data?.pages.at(-1)?.total ?? 0

  return (
    <Panel id="media-by-work" label={w.title} className="rise overflow-clip" style={{ animationDelay: '220ms' }}>
      <PanelHead
        title={w.title}
        action={
          <span className="flex items-center gap-3">
            {rows.length ? <TableOptions table={table} /> : null}
            <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.count(total, locale)}</span>
          </span>
        }
      />
      <div className="space-y-3 px-5 py-3">
        <p className="max-w-prose text-xs leading-relaxed text-bone-faint">{w.lead}</p>
        <Segmented<Order>
          label={w.order}
          value={order}
          onChange={setOrder}
          options={[
            { value: 'space', label: w.orders.space },
            { value: 'count', label: w.orders.count },
            { value: 'failed', label: w.orders.failed },
          ]}
        />
      </div>
      {works.isPending ? (
        <Skeleton className="m-5 h-24" />
      ) : works.isError ? (
        <p role="alert" className="px-5 py-4 text-sm text-status-error">
          {w.loadFailed}
        </p>
      ) : rows.length === 0 ? (
        <EmptyState size="line" title={w.none} />
      ) : (
        <>
          <DataTable table={table} label={w.title} onOpen={(id) => void navigate(`/admin/catalogue/${id}`)}>
            <THead>
              <Th column="work" />
              <Th column="stored" />
              <Th column="pending" />
              <Th column="failed" />
              <Th column="bytes" />
              <Th column="copies" />
            </THead>
            <TBody>
              {rows.map((work) => (
                <Tr key={work.id} row={work.id} data-work={work.id}>
                  <Td column="work" className="w-full max-w-0">
                    {/* Stretched over the row, so a tap anywhere on it opens the work. */}
                    <Link
                      to={`/admin/catalogue/${work.id}`}
                      className="group flex items-center gap-2 truncate text-bone transition-colors duration-150 after:absolute after:inset-0 after:content-[''] hover:text-vermillion"
                    >
                      <Glyph name={work.kind === 'series' ? 'tv' : 'film'} className="size-3.5 shrink-0 text-bone-faint" />
                      <span className="truncate underline decoration-rule-bright underline-offset-2 group-hover:decoration-vermillion">
                        {work.title}
                      </span>
                      {work.year ? <span className="font-mono text-xs text-bone-faint">{work.year}</span> : null}
                    </Link>
                  </Td>
                  <Td column="stored">
                    <span className="font-mono text-xs tabular-nums">{fmt.count(work.stored, locale)}</span>
                  </Td>
                  <Td column="pending">
                    <span className="font-mono text-xs text-bone-dim tabular-nums">{work.pending ? fmt.count(work.pending, locale) : '—'}</span>
                  </Td>
                  <Td column="failed">
                    <span className={work.failed ? 'font-mono text-xs text-vermillion tabular-nums' : 'font-mono text-xs text-bone-faint tabular-nums'}>
                      {work.failed ? fmt.count(work.failed, locale) : '—'}
                    </span>
                  </Td>
                  <Td column="bytes">
                    <span className="font-mono text-xs whitespace-nowrap tabular-nums">{fmt.bytes(work.bytes, locale)}</span>
                  </Td>
                  <Td column="copies">
                    <span className="font-mono text-xs whitespace-nowrap text-bone-dim tabular-nums">
                      {work.copiesBytes ? fmt.bytes(work.copiesBytes, locale) : '—'}
                    </span>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </DataTable>
          {works.hasNextPage ? (
            <div className="flex flex-col items-center gap-2 border-t border-rule px-5 py-5">
              <Button onClick={() => void works.fetchNextPage()} disabled={works.isFetchingNextPage}>
                {works.isFetchingNextPage ? <Spinner className="size-4" /> : null}
                {w.more}
              </Button>
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {fmt.count(rows.length, locale)} {t.common.of} {fmt.count(total, locale)}
              </span>
            </div>
          ) : null}
        </>
      )}
    </Panel>
  )
}
