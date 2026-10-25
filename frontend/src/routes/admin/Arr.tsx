/**
 * The operator's own Sonarr and Radarr, reached the other way.
 *
 * Elsewhere they ask and this server answers; here it asks them. First the
 * two connections, saved as one form — an address checked as an address, a
 * key that goes one way — each with a test that asks who answers. Then the
 * switch that tells them of a change, the seed that fills the catalogue from
 * their library, and what was told lately, with how each send went: a
 * failure is read here rather than hunted for in a log.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  DataTable,
  EmptyState,
  FormField,
  Glyph,
  Input,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Skeleton,
  Spinner,
  TableOptions,
  TBody,
  Td,
  Th,
  THead,
  Toggle,
  Tr,
  useDataTable,
  type Column,
} from '../../components/ui'
import { ApiError, api } from '../../lib/api'
import * as fmt from '../../lib/format'
import { useI18n, type Dict } from '../../lib/i18n'
import type { ArrApp, ArrConfiguration, ArrPush, ArrTest, Task } from '../../lib/types'

const APPS: ArrApp[] = ['sonarr', 'radarr']
const NAMES: Record<ArrApp, string> = { sonarr: 'Sonarr', radarr: 'Radarr' }
const PLACEHOLDERS: Record<ArrApp, string> = { sonarr: 'http://sonarr:8989', radarr: 'http://radarr:7878' }

/** What the form holds for one of the two, before it is saved. */
interface Draft {
  url: string
  /** The key as typed, when one is being typed; null keeps the one kept. */
  key: string | null
  /** The kept key asked to go. */
  clearKey: boolean
}

function draftsOf(config: ArrConfiguration): Record<ArrApp, Draft> {
  return {
    sonarr: { url: config.sonarr.url, key: null, clearKey: false },
    radarr: { url: config.radarr.url, key: null, clearKey: false },
  }
}

/** The origin an address is at — its scheme, host and port — or null for what is no address. */
function originOf(address: string): string | null {
  try {
    const origin = new URL(address.trim()).origin
    // An address of no web scheme has an opaque origin, which matches nothing.
    return origin === 'null' ? null : origin
  } catch {
    return null
  }
}

/**
 * Whether the key kept stays with the address typed. The server keeps a key
 * for the origin it was given with and sends it to no other, so an address
 * moved to another scheme, host or port needs its key typed again; an address
 * emptied takes the key along when saved.
 */
function keyMoves(kept: ArrConfiguration[ArrApp], typed: string): boolean {
  if (!kept.keySet || !typed.trim()) return false
  const origin = originOf(kept.url)
  return origin === null || originOf(typed) !== origin
}

/**
 * A seed run's summary in words. The server writes it in one fixed form —
 * `12 already here, 30 imported, 2 not found, 0 failed, of 44`, with
 * `{n} of {total}: ` before it while it runs and `stopped: ` once stopped.
 */
function describeSeed(detail: string | null | undefined, t: Dict): string | null {
  if (!detail) return null
  const p = t.admin.arrPage
  const m = detail.match(
    /^(?:(stopped): |(\d+) of (\d+): )?(\d+) already here, (\d+) imported, (\d+) not found, (\d+) failed(?:, of (\d+))?$/,
  )
  if (!m) return detail
  const n = (text: string | undefined) => Number(text ?? 0)
  if (m[2] !== undefined) return p.seedProgress(n(m[2]), n(m[3]))
  const said = p.seedSummary(n(m[5]), n(m[4]), n(m[6]), n(m[7]), n(m[8]))
  return m[1] ? `${p.seedStopped} · ${said}` : said
}

export function Arr() {
  const { t } = useI18n()
  const p = t.admin.arrPage

  const config = useQuery({
    queryKey: ['admin', 'arr'],
    queryFn: () => api.get<ArrConfiguration>('/admin/arr'),
    // Livelier while works wait to be told of: the sends are the progress.
    refetchInterval: (q) => ((q.state.data?.waiting ?? 0) > 0 ? 5_000 : 30_000),
  })

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.arrNav}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      </header>

      {config.isPending ? (
        <Skeleton className="h-96 w-full" />
      ) : config.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-status-error">
          <Glyph name="alert" className="size-4" />
          {p.loadFailed}
        </p>
      ) : (
        <div className="space-y-6">
          <Connections config={config.data} />
          <Push config={config.data} />
          <Seed connected={config.data.sonarr.ready || config.data.radarr.ready} />
          <Sent pushes={config.data.pushes} />
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs leading-relaxed text-bone-faint">
            <span className="max-w-prose">{p.renumbered}</span>
            <Link
              to="/admin/audit"
              className="inline-flex min-h-11 items-center gap-1.5 text-bone-dim underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-bone"
            >
              {p.audit}
              <Glyph name="chevronRight" className="size-3.5" />
            </Link>
          </p>
        </div>
      )}
    </div>
  )
}

/* ── The two connections ──────────────────────────────────────────────────── */

function Connections({ config }: { config: ArrConfiguration }) {
  const { t } = useI18n()
  const p = t.admin.arrPage
  const queryClient = useQueryClient()
  const [drafts, setDrafts] = useState<Record<ArrApp, Draft>>(() => draftsOf(config))

  const save = useMutation({
    mutationFn: () => {
      const body: Record<string, string> = {}
      for (const app of APPS) {
        const draft = drafts[app]
        body[`${app}Url`] = draft.url.trim()
        if (draft.clearKey) body[`${app}Key`] = ''
        else if (draft.key?.trim()) body[`${app}Key`] = draft.key.trim()
      }
      return api.put<ArrConfiguration>('/admin/arr', body)
    },
    onSuccess: (saved) => {
      queryClient.setQueryData(['admin', 'arr'], saved)
      setDrafts(draftsOf(saved))
      void queryClient.invalidateQueries({ queryKey: ['tasks'] })
    },
  })

  return (
    <form
      className="rise space-y-4"
      style={{ animationDelay: '40ms' }}
      onSubmit={(event) => {
        event.preventDefault()
        save.mutate()
      }}
    >
      <div className="grid gap-6 lg:grid-cols-2">
        {APPS.map((app) => (
          <ConnectionPanel
            key={app}
            app={app}
            config={config}
            draft={drafts[app]}
            onChange={(draft) => {
              save.reset()
              setDrafts((current) => ({ ...current, [app]: draft }))
            }}
          />
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <Button type="submit" variant="primary" disabled={save.isPending}>
          {save.isPending ? <Spinner className="size-4" /> : null}
          {save.isPending ? p.saving : p.save}
        </Button>
        {save.isSuccess ? (
          <span role="status" className="flex items-center gap-1.5 text-sm text-status-ok">
            <Glyph name="check" className="size-4" />
            {p.saved}
          </span>
        ) : null}
        {save.isError ? (
          <span role="alert" className="text-sm break-words text-status-error">
            {save.error instanceof ApiError ? save.error.message : t.common.error}
          </span>
        ) : null}
      </div>
    </form>
  )
}

function ConnectionPanel({
  app,
  config,
  draft,
  onChange,
}: {
  app: ArrApp
  config: ArrConfiguration
  draft: Draft
  onChange: (draft: Draft) => void
}) {
  const { t } = useI18n()
  const p = t.admin.arrPage
  const current = config[app]
  const name = NAMES[app]
  // The key kept goes with its address: at another, it is typed again,
  // and an address removed takes it along when saved.
  const moved = keyMoves(current, draft.url)
  const keptHere = current.keySet && !moved
  const leaving = current.keySet && current.url !== '' && !draft.url.trim()

  const test = useMutation({
    mutationFn: () =>
      api.post<ArrTest>('/admin/arr/test', {
        app,
        url: draft.url.trim() || undefined,
        key: draft.key?.trim() || undefined,
      }),
  })
  // A test answers for the address it was asked with: typing another
  // makes its word stale.
  const { reset } = test
  useEffect(() => reset(), [draft.url, draft.key, reset])

  return (
    <Panel label={name}>
      <PanelHead
        title={name}
        action={<Lamp tone={current.ready ? 'moss' : 'faint'}>{current.ready ? p.ready : p.notReady}</Lamp>}
      />
      <div className="space-y-5 p-5">
        <p className="text-xs leading-relaxed text-bone-faint">{p.connectionHint}</p>

        <FormField label={p.address} htmlFor={`arr-${app}-url`}>
          <Input
            id={`arr-${app}-url`}
            type="url"
            value={draft.url}
            placeholder={PLACEHOLDERS[app]}
            spellCheck={false}
            autoComplete="off"
            onChange={(event) => onChange({ ...draft, url: event.target.value })}
          />
        </FormField>

        <div className="space-y-1.5">
          <label className="label block" htmlFor={`arr-${app}-key`}>
            {p.key}
          </label>
          {current.keySet && (draft.clearKey || leaving) ? (
            <div className="flex flex-wrap items-center gap-2">
              <Chip tone="accent">{p.keyRemoved}</Chip>
              {leaving ? null : (
                <Button size="sm" variant="quiet" onClick={() => onChange({ ...draft, clearKey: false })}>
                  {moved ? p.keyTypeInstead : p.keyKeep}
                </Button>
              )}
            </div>
          ) : keptHere && draft.key === null ? (
            <div className="flex flex-wrap items-center gap-2">
              <Chip tone="provider">
                <Glyph name="lock" className="size-3" />
                {p.keySet}
              </Chip>
              <Button size="sm" onClick={() => onChange({ ...draft, key: '' })}>
                {p.keyReplace}
              </Button>
              <Button size="sm" variant="quiet" onClick={() => onChange({ ...draft, clearKey: true })}>
                {p.keyRemove}
              </Button>
            </div>
          ) : (
            <div className="space-y-2">
              <Input
                id={`arr-${app}-key`}
                type="password"
                autoComplete="new-password"
                spellCheck={false}
                required={moved}
                aria-describedby={moved ? `arr-${app}-key-moved` : undefined}
                value={draft.key ?? ''}
                onChange={(event) => onChange({ ...draft, key: event.target.value })}
              />
              {moved ? (
                <p id={`arr-${app}-key-moved`} className="flex items-start gap-2 text-xs leading-relaxed text-brass">
                  <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
                  {p.keyMoved}
                </p>
              ) : null}
              {keptHere ? (
                <Button size="sm" variant="quiet" onClick={() => onChange({ ...draft, key: null })}>
                  {p.keyKeep}
                </Button>
              ) : moved ? (
                <Button size="sm" variant="quiet" onClick={() => onChange({ ...draft, key: null, clearKey: true })}>
                  {p.keyRemove}
                </Button>
              ) : null}
            </div>
          )}
          <p className="text-xs text-bone-faint">{p.keyHint}</p>
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Button
            size="sm"
            disabled={!draft.url.trim() || (!keptHere && !draft.key?.trim()) || draft.clearKey || test.isPending}
            onClick={() => test.mutate()}
          >
            {test.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
            {test.isPending ? p.testing : p.test}
          </Button>
          {test.data?.ok && test.data.status ? (
            <span role="status" className="flex items-center gap-1.5 text-xs text-status-ok">
              <Glyph name="check" className="size-3.5" />
              {p.testOk(test.data.status.appName, test.data.status.version, test.data.status.instanceName)}
            </span>
          ) : test.data && !test.data.ok ? (
            <span role="alert" className="text-xs break-words text-status-error">
              {p.testFailed} {test.data.error}
            </span>
          ) : test.isError ? (
            <span role="alert" className="text-xs break-words text-status-error">
              {p.testFailed} {test.error instanceof ApiError ? test.error.message : t.common.error}
            </span>
          ) : null}
        </div>
      </div>
    </Panel>
  )
}

/* ── Telling them of a change ─────────────────────────────────────────────── */

function Push({ config }: { config: ArrConfiguration }) {
  const { t } = useI18n()
  const p = t.admin.arrPage
  const queryClient = useQueryClient()
  const connected = config.sonarr.ready || config.radarr.ready

  const set = useMutation({
    mutationFn: (on: boolean) => api.put<ArrConfiguration>('/admin/arr', { pushRefresh: on }),
    onSuccess: (saved) => queryClient.setQueryData(['admin', 'arr'], saved),
  })

  return (
    <Panel label={p.push} className="rise" style={{ animationDelay: '80ms' }}>
      <div className="flex items-start justify-between gap-4 px-5 py-4">
        <div className="min-w-0 space-y-1.5">
          <h2 className="font-display text-xl leading-tight text-bone">{p.push}</h2>
          <p className="max-w-prose text-xs leading-relaxed text-bone-dim">{p.pushHint}</p>
          {config.pushRefresh && !connected ? (
            <p className="flex items-start gap-2 text-xs text-brass">
              <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
              {p.pushNeeds}
            </p>
          ) : null}
          {config.waiting > 0 ? (
            <p role="status" className="flex items-center gap-2 text-xs text-bone-faint">
              <Spinner className="size-3.5" />
              {p.waiting(config.waiting)}
            </p>
          ) : null}
          {set.isError ? (
            <p role="alert" className="text-xs text-status-error">
              {set.error instanceof ApiError ? set.error.message : t.common.error}
            </p>
          ) : null}
        </div>
        <Toggle
          checked={config.pushRefresh}
          label={p.push}
          disabled={set.isPending}
          onChange={(next) => set.mutate(next)}
        />
      </div>
    </Panel>
  )
}

/* ── The seed ─────────────────────────────────────────────────────────────── */

function Seed({ connected }: { connected: boolean }) {
  const { t, locale } = useI18n()
  const p = t.admin.arrPage
  const queryClient = useQueryClient()

  const tasks = useQuery({
    queryKey: ['tasks'],
    queryFn: () => api.get<Task[]>('/tasks'),
    refetchInterval: (q) => (q.state.data?.find((task) => task.id === 'arr.seed')?.running ? 3_000 : 30_000),
  })
  const task = tasks.data?.find((each) => each.id === 'arr.seed')
  const run = useMutation({
    mutationFn: () => api.post<{ jobId?: string }>('/tasks/arr.seed/run'),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ['tasks'] })
      void queryClient.invalidateQueries({ queryKey: ['jobs'] })
    },
  })

  const last = task?.running ?? task?.last
  const blocked = task?.blocked && task.blocked !== 'running' ? task.blocked : undefined

  return (
    <Panel label={p.seed} className="rise" style={{ animationDelay: '120ms' }}>
      <PanelHead
        title={p.seed}
        action={
          <Link
            to="/admin/jobs"
            className="label -my-2 inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
          >
            {p.tasks}
          </Link>
        }
      />
      <div className="space-y-4 p-5">
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{p.seedLead}</p>

        <div className="text-sm">
          <span className="label block">{p.seedLast}</span>
          {tasks.isPending ? (
            <Skeleton className="mt-1.5 h-5 w-64" />
          ) : last ? (
            <div className="mt-1 space-y-1">
              <div className="flex flex-wrap items-center gap-2">
                {task?.running ? (
                  <Lamp tone="brass">{t.admin.runs.running}</Lamp>
                ) : last.status === 'failed' ? (
                  <Chip tone="accent">
                    <Glyph name="alert" className="size-3" />
                    {t.admin.runs.failed}
                  </Chip>
                ) : (
                  <Chip>
                    <Glyph name={last.status === 'stopped' ? 'close' : 'check'} className="size-3" />
                    {last.status === 'stopped' ? t.admin.runs.stopped : t.admin.runs.succeeded}
                  </Chip>
                )}
                <span className="text-xs text-bone-faint" title={fmt.dateTime(last.createdAt, locale)}>
                  {fmt.relative(last.createdAt, locale)}
                </span>
              </div>
              {describeSeed(last.detail, t) ? (
                <p className="font-mono text-xs text-bone-dim tabular-nums">{describeSeed(last.detail, t)}</p>
              ) : null}
              {last.error ? <p className="text-xs break-words text-vermillion">{last.error}</p> : null}
            </div>
          ) : (
            <p className="mt-1 text-bone-faint">{p.seedNever}</p>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Button
            variant="primary"
            size="sm"
            disabled={!connected || Boolean(task?.running) || Boolean(blocked) || run.isPending}
            onClick={() => run.mutate()}
          >
            {run.isPending || task?.running ? <Spinner className="size-4" /> : <Glyph name="play" className="size-4" />}
            {p.seedRun}
          </Button>
          {!connected ? (
            <span className="text-xs text-bone-faint">{t.admin.tasks.blocked.arr_off}</span>
          ) : blocked ? (
            <span className="text-xs text-bone-faint">{t.admin.tasks.blocked[blocked] ?? blocked}</span>
          ) : null}
          {run.isSuccess ? (
            <span role="status" className="text-xs text-status-ok">
              {p.seedStarted}
            </span>
          ) : null}
          {run.isError ? (
            <span role="alert" className="text-xs text-status-error">
              {run.error instanceof ApiError ? run.error.message : t.common.error}
            </span>
          ) : null}
        </div>
      </div>
    </Panel>
  )
}

/* ── What was told lately ─────────────────────────────────────────────────── */

function Outcome({ outcome }: { outcome: ArrPush['outcome'] }) {
  const { t } = useI18n()
  const word = t.admin.arrPage.outcomes[outcome] ?? outcome
  if (outcome === 'failed') {
    return (
      <Chip tone="accent">
        <Glyph name="alert" className="size-3" />
        {word}
      </Chip>
    )
  }
  return (
    <Chip tone={outcome === 'sent' ? 'provider' : 'neutral'}>
      <Glyph name={outcome === 'sent' ? 'check' : 'close'} className="size-3" />
      {word}
    </Chip>
  )
}

function Work({ push }: { push: ArrPush }) {
  const { t } = useI18n()
  if (push.mediaId && push.title) {
    return (
      <Link
        to={`/admin/catalogue/${push.mediaId}`}
        className="block truncate text-sm text-bone underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
      >
        {push.title}
      </Link>
    )
  }
  return <span className="block truncate text-sm text-bone-dim">{push.title ?? t.admin.arrPage.goneWork}</span>
}

function Sent({ pushes }: { pushes: ArrPush[] }) {
  const { t, locale } = useI18n()
  const p = t.admin.arrPage
  const why = (push: ArrPush) => push.reasons.map((reason) => p.reasons[reason] ?? reason).join(', ')

  // A send is known by the work it told of; on a phone, a card each, as six
  // columns do not fit. These are the latest sends, newest first, and no
  // head orders them: put in another order here, they would say nothing of
  // the sends before them.
  const columns: Column[] = useMemo(
    () => [
      { id: 'when', label: p.colWhen },
      { id: 'work', label: p.colWork, kind: 'primary', always: true },
      { id: 'to', label: p.colTo },
      { id: 'outcome', label: p.colOutcome },
      { id: 'why', label: p.colWhy },
      { id: 'detail', label: p.colDetail },
    ],
    [p],
  )
  const table = useDataTable({ id: 'arr-sent', columns })

  return (
    <Panel label={p.sent} className="rise overflow-clip" style={{ animationDelay: '160ms' }}>
      <PanelHead title={p.sent} action={pushes.length ? <TableOptions table={table} /> : undefined} />
      {pushes.length === 0 ? (
        <EmptyState title={p.sentEmpty} hint={p.sentEmptyHint} />
      ) : (
        <DataTable table={table} label={p.sent} minWidth="48rem">
          <THead>
            <Th column="when" />
            <Th column="work" />
            <Th column="to" />
            <Th column="outcome" />
            <Th column="why" />
            <Th column="detail" />
          </THead>
          <TBody>
            {pushes.map((push) => (
              <Tr key={push.id} row={String(push.id)}>
                <Td column="when" className="whitespace-nowrap">
                  <span className="font-mono text-xs text-bone-faint tabular-nums" title={fmt.dateTime(push.at, locale)}>
                    {fmt.relative(push.at, locale)}
                  </span>
                </Td>
                <Td column="work" className="max-w-[16rem]">
                  <Work push={push} />
                </Td>
                <Td column="to" className="whitespace-nowrap text-sm text-bone-dim">
                  {NAMES[push.app]}
                </Td>
                <Td column="outcome">
                  <Outcome outcome={push.outcome} />
                </Td>
                <Td column="why" className="text-xs text-bone-dim">
                  {why(push)}
                </Td>
                <Td column="detail" className="w-full max-w-0">
                  <span className="block truncate text-xs text-bone-faint" title={push.detail}>
                    {push.detail ?? '—'}
                  </span>
                </Td>
              </Tr>
            ))}
          </TBody>
        </DataTable>
      )}
    </Panel>
  )
}
