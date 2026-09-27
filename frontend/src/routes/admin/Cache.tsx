/**
 * The cache, read back: what each space holds in each tier and how often
 * it answered, the server behind the second tier and how it is doing.
 *
 * One sentence at the top says what an administrator would otherwise piece
 * together from the panels. A space is switched or emptied on its own; the
 * whole cache is emptied only after a word of warning, though nothing is
 * lost by it — the database is the reference, the cache only saves the
 * asking.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import {
  Button,
  Chip,
  Dialog,
  Glyph,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Skeleton,
  Spinner,
  Toggle,
} from '../../components/ui'
import { ApiError, api } from '../../lib/api'
import { cn } from '../../lib/cn'
import { bytes as fmtBytes, relative } from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { CacheFlushed, CacheReport, CacheSpace } from '../../lib/types'

const SPACES: CacheSpace['id'][] = ['items', 'searches', 'lists', 'relay', 'sessions']

/** Hits over reads, as a percentage — or none when nothing was read yet. */
function ratio(hits: number, misses: number): number | null {
  const reads = hits + misses
  return reads === 0 ? null : Math.round((hits / reads) * 100)
}

/** A percentage as the locale writes it: `70%` in English, `70 %` in French. */
function percent(value: number, locale: string): string {
  return new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 0 }).format(value / 100)
}

export function Cache() {
  const { t, locale } = useI18n()
  const p = t.admin.cachePage
  const queryClient = useQueryClient()
  const [asking, setAsking] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  const report = useQuery({
    queryKey: ['admin', 'cache'],
    queryFn: () => api.get<CacheReport>('/admin/cache'),
    // The figures move while the page is open; a glance every half-minute
    // is enough to see the cache fill.
    refetchInterval: 30_000,
  })

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['admin', 'cache'] })
  }

  const write = useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) =>
      api.put('/settings/server/-', { key, value }),
    // The switch moves at once; the server's word replaces it on the refetch.
    onMutate: ({ key, value }) => {
      const space = key.replace(/^cache\./, '')
      queryClient.setQueryData<CacheReport>(['admin', 'cache'], (current) =>
        current
          ? { ...current, spaces: current.spaces.map((s) => (s.id === space ? { ...s, enabled: value === 'true' } : s)) }
          : current,
      )
    },
    onSettled: refresh,
  })

  const flushSpace = useMutation({
    mutationFn: (space: string) => api.post<CacheFlushed>(`/admin/cache/${space}/flush`),
    onSuccess: (done, space) => {
      setNotice(p.flushed(p.names[space] ?? space, done.keys))
      refresh()
    },
  })

  const flushAll = useMutation({
    mutationFn: () => api.post<CacheFlushed>('/admin/cache/flush'),
    onSuccess: (done) => {
      setAsking(false)
      setNotice(p.flushedAll(done.keys))
      refresh()
    },
  })

  if (report.isPending) {
    return (
      <div className="mx-auto max-w-5xl space-y-6">
        <Skeleton className="h-28 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  // Nothing to show yet: the error is the page. With figures already shown,
  // a refetch that failed is a line under them, not their replacement.
  if (!report.data) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {report.error?.message ?? t.common.actionFailed}
      </p>
    )
  }

  const data = report.data
  const server = data.server
  const info = server.info
  const instances = data.instances
  const tiered = data.spaces.filter((s) => s.server)
  const memoryHits = data.spaces.reduce((n, s) => n + s.memory.hits, 0)
  const memoryMisses = data.spaces.reduce((n, s) => n + s.memory.misses, 0)
  const serverHits = tiered.reduce((n, s) => n + (s.server?.hits ?? 0), 0)
  const serverMisses = tiered.reduce((n, s) => n + (s.server?.misses ?? 0), 0)
  // Served from the cache at all: what memory answered, and what the server
  // answered of the rest — every server read follows a memory miss, so the
  // misses that count are the memory's less the server's hits.
  const overall = ratio(memoryHits + serverHits, Math.max(0, memoryMisses - serverHits))
  const memoryEntries = data.spaces.reduce((n, s) => n + s.memory.entries, 0)
  const memoryBytes = data.spaces.reduce((n, s) => n + (s.memory.bytes ?? 0), 0)
  const serverName = info ? (info.server === 'valkey' ? 'Valkey' : 'Redis') : null
  const latency =
    server.latencyMs === undefined
      ? null
      : `${server.latencyMs.toLocaleString(locale, { maximumFractionDigits: server.latencyMs < 10 ? 1 : 0 })} ms`
  // Only `noeviction` refuses to write at full memory; every other policy
  // evicts something.
  const noEviction = info?.evictionPolicy === 'noeviction'

  const summary = !server.configured
    ? p.summaryOff
    : !server.attached
      ? p.summaryWaiting(server.address ?? '')
      : !server.up
        ? p.summaryDown(server.address ?? '')
        : p.summaryOn(
            `${serverName ?? 'Redis'} ${info?.version ?? ''}`.trim(),
            latency ?? '—',
            fmtBytes(info?.usedMemory ?? 0, locale),
            info && info.maxmemory > 0 ? fmtBytes(info.maxmemory, locale) : null,
            overall === null ? null : percent(overall, locale),
          )

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.cacheNav}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      </header>

      {report.isError ? (
        <p role="alert" className="rise mb-6 flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {report.error.message}
        </p>
      ) : null}

      {/* The page, read back in a sentence. */}
      <Panel className="rise mb-6 border-l-2 border-l-vermillion" style={{ animationDelay: '40ms' }}>
        <div className="px-6 py-5">
          <Label>{p.summaryLabel}</Label>
          <p className="mt-2 font-display text-xl leading-snug text-bone sm:text-2xl">{summary}</p>
          <p className="mt-2 text-xs text-bone-faint">
            {data.publicSeconds > 0 ? p.publicSeconds(data.publicSeconds) : p.publicOff}
          </p>
        </div>
      </Panel>

      <div className="space-y-6">
        {/* The two tiers */}
        <Panel label={p.tiers} className="rise" style={{ animationDelay: '80ms' }}>
          <PanelHead
            title={p.tiers}
            action={
              server.attached && server.up ? (
                <Lamp tone="moss">{p.tiersOk}</Lamp>
              ) : (
                <Lamp tone={server.configured ? 'brass' : 'faint'}>{p.tiersMemory}</Lamp>
              )
            }
          />
          <div className="grid md:grid-cols-2">
            <Tier
              title={p.memoryTitle}
              sub={p.memorySub}
              locale={locale}
              figures={[
                { n: memoryEntries.toLocaleString(locale), k: p.entries },
                { n: fmtBytes(memoryBytes, locale), k: p.inMemory },
                { n: ratio(memoryHits, memoryMisses), k: p.hitRatio },
              ]}
            />
            <Tier
              title={serverName ?? p.serverTitle}
              sub={server.attached ? p.serverSub(server.prefix) : server.configured ? p.serverWaitingSub : p.serverNoneSub}
              divided
              locale={locale}
              figures={
                server.attached
                  ? [
                      { n: server.keys === undefined ? '—' : server.keys.toLocaleString(locale), k: p.keysUnder(server.prefix) },
                      {
                        n: fmtBytes(info?.usedMemory ?? 0, locale),
                        k: info && info.maxmemory > 0 ? p.of(fmtBytes(info.maxmemory, locale)) : p.used,
                      },
                      { n: ratio(serverHits, serverMisses), k: p.hitRatioAfterMemory },
                    ]
                  : []
              }
            />
          </div>
        </Panel>

        {/* The instances: one, or several */}
        <Panel label={p.instances} className="rise" style={{ animationDelay: '100ms' }}>
          <PanelHead
            title={p.instances}
            action={
              instances.mode === 'single' ? (
                <Lamp tone="faint">{p.instancesOne}</Lamp>
              ) : instances.leader ? (
                <Lamp tone="moss">{p.instancesMany(instances.all.length)}</Lamp>
              ) : (
                <Lamp tone="brass">{p.noLeader}</Lamp>
              )
            }
          />
          <div className="p-5">
            <p className="text-sm leading-relaxed text-bone-dim">
              {instances.mode === 'single' ? p.instancesSingleHint : p.instancesMultiHint(instances.leader ?? null)}
            </p>
            {instances.mode === 'multi' ? (
              <ul className="mt-4 divide-y divide-rule">
                {instances.all.map((one) => (
                  <li key={one.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 py-2.5 text-sm">
                    <span className="flex min-w-0 items-center gap-2">
                      <span aria-hidden className={cn('size-1.5 shrink-0 rounded-full', one.leads ? 'bg-moss' : 'bg-bone-faint')} />
                      <span className="truncate font-medium text-bone">{one.name}</span>
                    </span>
                    {one.leads ? <Lamp tone="moss">{p.instanceLeads}</Lamp> : null}
                    {one.id === instances.this.id ? <Chip tone="neutral">{p.instanceThis}</Chip> : null}
                    <span className="ml-auto font-mono text-xs text-bone-faint tabular-nums">
                      v{one.version} · {p.instanceStarted(relative(one.startedAt, locale) ?? '—')} ·{' '}
                      {p.instanceSeen(relative(one.seenAt, locale) ?? '—')}
                    </span>
                  </li>
                ))}
              </ul>
            ) : null}
          </div>
        </Panel>

        {/* Each space */}
        <Panel label={p.spaces} className="rise" style={{ animationDelay: '120ms' }}>
          <PanelHead title={p.spaces} action={<span className="hidden text-xs text-bone-faint sm:inline">{p.spacesHint}</span>} />
          <ul className="divide-y divide-rule">
            {SPACES.map((id) => {
              const space = data.spaces.find((s) => s.id === id)
              if (!space) return null
              const memoryRatio = ratio(space.memory.hits, space.memory.misses)
              const serverRatio = space.server ? ratio(space.server.hits, space.server.misses) : null
              const busy = flushSpace.isPending && flushSpace.variables === id
              return (
                <li
                  key={id}
                  className={cn(
                    'grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 px-5 py-3',
                    'md:grid-cols-[minmax(0,1.3fr)_9.5rem_6.5rem_9.5rem_4.5rem_auto]',
                    !space.enabled && 'opacity-60',
                  )}
                >
                  <div className="min-w-0 max-md:col-start-1 max-md:row-start-1">
                    <span className="block font-medium text-bone">{p.names[id]}</span>
                    <span className="block text-xs text-bone-faint">{p.about[id]}</span>
                  </div>
                  <span className="flex gap-1.5 max-md:col-span-2 max-md:row-start-2">
                    <Chip tone="provider">{p.memoryChip}</Chip>
                    {space.server ? <Chip tone="neutral">{p.serverChip}</Chip> : null}
                  </span>
                  <span className="font-mono text-xs tabular-nums text-bone-dim max-md:col-start-1 max-md:row-start-3">
                    <span className="sr-only">{p.keysCol}: </span>
                    {space.memory.entries.toLocaleString(locale)}
                    {space.server ? ` · ${space.server.keys === undefined ? '—' : space.server.keys.toLocaleString(locale)}` : ''}
                  </span>
                  <span className="max-md:col-start-2 max-md:row-start-3 max-md:text-right">
                    <span className="sr-only">{p.hitsCol}: </span>
                    <Ratio value={memoryRatio} locale={locale} />{' '}
                    {serverRatio !== null ? <Ratio value={serverRatio} dim locale={locale} /> : null}
                  </span>
                  <span className="font-mono text-xs tabular-nums text-bone-dim max-md:col-start-1 max-md:row-start-4">
                    <span className="sr-only">{p.ttl}: </span>
                    {p.ttlOf(space.ttlSeconds)}
                  </span>
                  <span className="flex items-center gap-2 justify-self-end max-md:col-start-2 max-md:row-start-1">
                    <Toggle
                      checked={space.enabled}
                      label={p.toggle(p.names[id] ?? id)}
                      onChange={(next) => write.mutate({ key: `cache.${id}`, value: String(next) })}
                      disabled={write.isPending}
                    />
                    <Button
                      size="sm"
                      variant="quiet"
                      disabled={busy}
                      aria-busy={busy}
                      aria-label={`${p.flush} · ${p.names[id] ?? id}`}
                      onClick={() => flushSpace.mutate(id)}
                    >
                      {busy ? <Spinner className="size-4" /> : null}
                      {p.flush}
                    </Button>
                  </span>
                </li>
              )
            })}
          </ul>
          <p role="status" aria-live="polite" className="border-t border-rule px-5 py-3 text-xs text-bone-faint">
            {flushSpace.isError
              ? flushSpace.error instanceof ApiError
                ? flushSpace.error.message
                : t.common.actionFailed
              : (notice ?? '')}
          </p>
        </Panel>

        <div className="grid gap-6 lg:grid-cols-2">
          {/* The server */}
          <Panel label={p.server} className="rise" style={{ animationDelay: '160ms' }}>
            <PanelHead
              title={p.server}
              action={
                !server.configured ? (
                  <Lamp tone="faint">{p.notConfigured}</Lamp>
                ) : !server.attached ? (
                  <Lamp tone="brass">{p.waiting}</Lamp>
                ) : server.up ? (
                  <Lamp tone="moss">{p.connected}</Lamp>
                ) : (
                  <Lamp tone="vermillion">{p.down}</Lamp>
                )
              }
            />
            <div className="p-5">
              {server.configured ? (
                <dl className="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_minmax(0,1fr)]">
                  <dt className="text-bone-faint">{p.version}</dt>
                  <dd className="text-bone">{info ? `${serverName} ${info.version}` : '—'}</dd>
                  <dt className="text-bone-faint">{p.address}</dt>
                  <dd className="min-w-0 font-mono text-xs break-all text-bone-dim">
                    {server.address ?? '—'} · {p.prefix} <code>{server.prefix}</code>
                  </dd>
                  <dt className="text-bone-faint">{p.memory}</dt>
                  <dd className="text-bone">
                    {info
                      ? info.maxmemory > 0
                        ? p.memoryOf(fmtBytes(info.usedMemory, locale), fmtBytes(info.maxmemory, locale))
                        : p.memoryUnbounded(fmtBytes(info.usedMemory, locale))
                      : '—'}
                  </dd>
                  <dt className="text-bone-faint">{p.eviction}</dt>
                  <dd className="text-bone">
                    {info ? (
                      <>
                        <code>{info.evictionPolicy || p.unknownPolicy}</code> · {p.evicted(info.evictedKeys)}
                      </>
                    ) : (
                      '—'
                    )}
                  </dd>
                  <dt className="text-bone-faint">{p.clients}</dt>
                  <dd className="text-bone">{info ? info.connectedClients.toLocaleString(locale) : '—'}</dd>
                  <dt className="text-bone-faint">{p.started}</dt>
                  <dd className="text-bone">
                    {info ? (relative(new Date(Date.now() - info.uptimeSeconds * 1000).toISOString(), locale) ?? '—') : '—'}
                  </dd>
                  <dt className="text-bone-faint">{p.latency}</dt>
                  <dd className="text-bone">{latency ?? '—'}</dd>
                  <dt className="text-bone-faint">{p.lastError}</dt>
                  <dd className={cn('text-xs', server.lastError ? 'text-brass' : 'text-bone-faint')}>
                    {server.lastError ?? p.none}
                    {server.errors > 0 ? ` · ${p.errors(server.errors)}` : ''}
                  </dd>
                </dl>
              ) : (
                <p className="text-sm leading-relaxed text-bone-dim">{p.noServerHint}</p>
              )}
              {noEviction ? (
                <p role="alert" className="mt-4 flex items-start gap-2 rounded-card border border-brass-deep bg-brass/[0.06] px-4 py-3 text-sm leading-relaxed text-bone">
                  <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-brass" />
                  {p.evictionWarning}
                </p>
              ) : null}
            </div>
          </Panel>

          {/* Actions and where the figures go */}
          <Panel label={p.actions} className="rise" style={{ animationDelay: '200ms' }}>
            <PanelHead title={p.actions} />
            <div className="space-y-4 p-5">
              <div className="flex flex-wrap items-center gap-2">
                <Button variant="danger" size="sm" onClick={() => setAsking(true)}>
                  {p.flushAll}
                </Button>
                <Button size="sm" onClick={refresh} disabled={report.isFetching}>
                  {p.refresh}
                </Button>
              </div>
              <p className="text-xs leading-relaxed text-bone-faint">{p.flushHint}</p>
              <h2 className="font-display text-lg font-medium text-bone">{p.metrics}</h2>
              <p className="text-xs leading-relaxed text-bone-faint">{p.metricsHint}</p>
              <p className="font-mono text-[0.6875rem] text-bone-faint">
                {p.stamp(data.generation, data.epoch)}
              </p>
            </div>
          </Panel>
        </div>
      </div>

      <Dialog
        open={asking}
        title={p.confirmTitle}
        onClose={() => setAsking(false)}
        footer={
          <>
            <Button onClick={() => setAsking(false)}>{p.keep}</Button>
            <Button variant="danger" disabled={flushAll.isPending} onClick={() => flushAll.mutate()}>
              {flushAll.isPending ? <Spinner className="size-4" /> : <Glyph name="trash" className="size-4" />}
              {p.confirm}
            </Button>
          </>
        }
      >
        {p.confirmBody(server.attached ? (server.keys ?? null) : null, server.attached)}
        {flushAll.isError ? (
          <p role="alert" className="mt-3 text-sm text-vermillion">
            {flushAll.error instanceof ApiError ? flushAll.error.message : t.common.actionFailed}
          </p>
        ) : null}
      </Dialog>
    </div>
  )
}

function Tier({
  title,
  sub,
  figures,
  divided,
  locale,
}: {
  title: string
  sub: string
  figures: { n: string | number | null; k: string }[]
  divided?: boolean
  locale: string
}) {
  return (
    <div className={cn('p-5', divided && 'border-t border-rule md:border-t-0 md:border-l')}>
      <h2 className="font-display text-xl font-medium text-bone">{title}</h2>
      <p className="mt-0.5 mb-4 text-xs text-bone-faint">{sub}</p>
      {figures.length ? (
        <dl className="grid grid-cols-3 gap-4">
          {figures.map((f) => (
            // The term first in the document, the value first on the screen.
            <div key={f.k} className="flex flex-col-reverse">
              <dt className="mt-1 font-mono text-[0.625rem] tracking-[0.12em] text-bone-faint uppercase">{f.k}</dt>
              <dd className="font-display text-2xl tabular-nums text-bone">
                {f.n === null ? '—' : typeof f.n === 'number' ? percent(f.n, locale) : f.n}
              </dd>
            </div>
          ))}
        </dl>
      ) : null}
    </div>
  )
}

function Ratio({ value, dim, locale }: { value: number | null; dim?: boolean; locale: string }) {
  if (value === null) return <span className={cn('font-mono text-xs', dim ? 'text-bone-faint' : 'text-bone-dim')}>—</span>
  return (
    <span className="inline-flex items-center gap-2">
      <span aria-hidden className="h-1.5 w-12 overflow-hidden rounded-full bg-ink-top">
        <span className={cn('block h-full', value < 50 ? 'bg-brass' : 'bg-moss')} style={{ width: `${value}%` }} />
      </span>
      <span className={cn('font-mono text-xs tabular-nums', dim ? 'text-bone-faint' : 'text-bone')}>{percent(value, locale)}</span>
    </span>
  )
}
