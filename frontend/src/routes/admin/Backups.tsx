/**
 * The archives of the database: when the last was made and the next will
 * be, the schedule and how many it keeps, every archive with what it holds —
 * downloaded, deleted, restored.
 *
 * A restore is the one thing on this side that shuts the server: the dialog
 * says so before it starts, in that order — the safety copy first, every
 * door answering 503 until it is done, everybody signed out but those the
 * archive holds. Then the page follows it through the door it shut: every
 * answer is a 503 carrying the step it is at, read here, until the doors
 * open again and the restore says how it went — or, the session gone with
 * the database that held it, the sign-in page is where to go next.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  ButtonLink,
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
import { ApiError, api, segment } from '../../lib/api'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { Backup as Archive, BackupList, BackupRestore as RestoreState } from '../../lib/types'

/** What a door says while the server restores: the step it is at. */
interface Under {
  phase: string
  table?: string
  step: number
  steps: number
}

/** How a restore followed from this page stands. */
type Following =
  | { stage: 'under'; under: Under | null }
  | { stage: 'done'; state: RestoreState }
  | { stage: 'signedOut' }

/**
 * Ask how the restore stands, through a door that may be shut: a 503 is not
 * a failure here but the step it is at, and a 401 is the database restored
 * without this session in it.
 */
async function askRestore(): Promise<Following | null> {
  let response: Response
  try {
    response = await fetch('/api/v1/admin/backups/restore', { credentials: 'same-origin' })
  } catch {
    // Not answering at all: asked again.
    return null
  }
  if (response.status === 401) return { stage: 'signedOut' }
  const body = (await response.json().catch(() => null)) as
    | (RestoreState & { maintenance?: Under })
    | null
  if (response.status === 503) return { stage: 'under', under: body?.maintenance ?? null }
  if (!response.ok || !body) return null
  return body.state === 'running' ? { stage: 'under', under: null } : { stage: 'done', state: body }
}

export function Backups() {
  const { t, locale } = useI18n()
  const p = t.admin.backupsPage
  const queryClient = useQueryClient()
  const [notice, setNotice] = useState<string | null>(null)
  const [withMedia, setWithMedia] = useState(false)
  const [deleting, setDeleting] = useState<Archive | null>(null)
  const [restoring, setRestoring] = useState<Archive | null>(null)
  const [passphrase, setPassphrase] = useState('')
  const [following, setFollowing] = useState<Following | null>(null)

  const followingUnder = following?.stage === 'under'

  const listing = useQuery({
    queryKey: ['admin', 'backups'],
    queryFn: () => api.get<BackupList>('/admin/backups'),
    // Every few seconds while an archive is being made; nothing while a
    // restore has the doors shut.
    refetchInterval: (q) => (q.state.data?.running ? 3_000 : 30_000),
    enabled: !followingUnder,
  })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['admin', 'backups'] })
    void queryClient.invalidateQueries({ queryKey: ['tasks'] })
    void queryClient.invalidateQueries({ queryKey: ['admin', 'todo'] })
  }

  // Following a restore: asked every second until it is over, and then
  // whatever the page showed read again.
  useEffect(() => {
    if (!followingUnder) return
    let stopped = false
    const ask = async () => {
      const now = await askRestore()
      if (stopped || !now) return
      setFollowing(now)
      if (now.stage !== 'under') void queryClient.invalidateQueries()
    }
    const timer = window.setInterval(() => void ask(), 1_000)
    return () => {
      stopped = true
      window.clearInterval(timer)
    }
  }, [followingUnder, queryClient])

  const set = useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) => api.put('/settings/server/-', { key, value }),
    onSuccess: invalidate,
  })

  const backUp = useMutation({
    mutationFn: () => api.post<{ jobId?: string }>('/admin/backups', { includeMedia: withMedia }),
    onMutate: () => setNotice(null),
    onSuccess: () => {
      setNotice(p.started)
      invalidate()
    },
  })

  const remove = useMutation({
    mutationFn: (archive: Archive) => api.delete(`/admin/backups/${segment(archive.name)}`),
    onSuccess: () => {
      setDeleting(null)
      invalidate()
    },
  })

  const restore = useMutation({
    mutationFn: (archive: Archive) =>
      api.post(`/admin/backups/${segment(archive.name)}/restore`, { passphrase: passphrase || undefined }),
    onSuccess: () => {
      setRestoring(null)
      setPassphrase('')
      setFollowing({ stage: 'under', under: null })
    },
  })

  const data = listing.data

  // An archive is known by when it was made. Its actions are a line of
  // their own on a card, as they were on a phone: three buttons beside the
  // date would leave the date no room.
  const columns: Column[] = useMemo(
    () => [
      { id: 'made', label: p.colMade, kind: 'primary', always: true, sort: 'desc' },
      { id: 'why', label: p.colWhy, sort: 'asc' },
      { id: 'holds', label: p.colHolds },
      { id: 'size', label: p.colSize, align: 'right', sort: 'desc' },
      { id: 'actions', label: p.colActions, always: true, align: 'right' },
    ],
    [p],
  )
  const table = useDataTable({ id: 'backups', columns })
  const archives = sortRows(
    data?.archives ?? [],
    table.sort,
    {
      made: (archive) => Date.parse(archive.createdAt),
      why: (archive) => p.reasons[archive.reason] ?? archive.reason,
      size: (archive) => archive.sizeBytes,
    },
    locale,
  )

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.backupsNav}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      </header>

      {following ? <Follow following={following} onClose={() => setFollowing(null)} /> : null}

      {listing.isPending && !data ? (
        <div className="space-y-6">
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-64 w-full" />
        </div>
      ) : !data ? (
        followingUnder ? null : (
          <p role="alert" className="flex items-center gap-2 text-sm text-status-error">
            <Glyph name="alert" className="size-4" />
            {listing.error instanceof ApiError ? listing.error.message : p.loadFailed}
          </p>
        )
      ) : (
        <div className="space-y-6">
          <Panel className="rise" style={{ animationDelay: '40ms' }}>
            <dl className="grid grid-cols-2 gap-px bg-rule lg:grid-cols-4">
              <div className="bg-ink-raised px-5 py-4">
                <dt className="label">{p.last}</dt>
                <dd className="mt-1 text-sm text-bone">
                  {data.lastAt ? (
                    <>
                      <span className="block font-display text-xl">{fmt.relative(data.lastAt, locale)}</span>
                      <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.dateTime(data.lastAt, locale)}</span>
                    </>
                  ) : (
                    <span className="font-display text-xl text-bone-dim">{p.never}</span>
                  )}
                </dd>
              </div>
              <div className="bg-ink-raised px-5 py-4">
                <dt className="label">{p.next}</dt>
                <dd className="mt-1 text-sm text-bone">
                  {data.nextAt ? (
                    <>
                      <span className="block font-display text-xl">{fmt.relative(data.nextAt, locale)}</span>
                      <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.dateTime(data.nextAt, locale)}</span>
                    </>
                  ) : (
                    <span className="font-display text-xl text-bone-dim">{p.nextOff}</span>
                  )}
                </dd>
              </div>
              <div className="bg-ink-raised px-5 py-4">
                <dt className="label">{p.keptIn}</dt>
                <dd className="mt-1 text-sm text-bone">
                  <span className="block">{p.places[data.place] ?? data.place}</span>
                  <span className="block font-mono text-xs break-all text-bone-faint">{data.location}</span>
                </dd>
              </div>
              <div className="bg-ink-raised px-5 py-4">
                <dt className="label">{p.encryption}</dt>
                <dd className="mt-1 text-sm text-bone">
                  <span className="flex items-center gap-1.5">
                    <Glyph name={data.encrypted ? 'lock' : 'unlock'} className="size-4 text-bone-faint" />
                    {data.encrypted ? p.encrypted : p.plain}
                  </span>
                </dd>
              </div>
            </dl>
            <div className="space-y-2 border-t border-rule px-5 py-3 text-sm">
              {data.overdue ? (
                <p className="flex items-start gap-2 text-bone">
                  <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-vermillion" />
                  {p.overdue}
                </p>
              ) : null}
              <p className="max-w-prose text-xs leading-relaxed text-bone-faint">
                {data.encrypted ? p.encryptedHint : p.plainHint}
              </p>
            </div>
          </Panel>

          <Panel className="rise" style={{ animationDelay: '80ms' }}>
            <PanelHead title={p.schedule} />
            <ul className="divide-y divide-rule">
              <li className="px-5 py-3">
                <label htmlFor="backup-schedule" className="block text-sm text-bone">
                  {p.howOften}
                </label>
                <Select
                  id="backup-schedule"
                  className="mt-2 w-auto min-w-48"
                  value={data.schedule}
                  disabled={set.isPending}
                  onChange={(event) => set.mutate({ key: 'backup.schedule', value: event.target.value })}
                >
                  {(['off', 'daily', 'weekly', 'monthly'] as const).map((schedule) => (
                    <option key={schedule} value={schedule}>
                      {p.schedules[schedule]}
                    </option>
                  ))}
                </Select>
                <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">{p.howOftenHint}</p>
              </li>
              <li className="px-5 py-3">
                <Keep
                  value={data.keep}
                  disabled={set.isPending}
                  onSave={(keep) => set.mutate({ key: 'backup.keep', value: String(keep) })}
                />
              </li>
              <li className="flex items-start gap-4 px-5 py-3">
                <div className="min-w-0 flex-1">
                  <label htmlFor="backup-media" className="block text-sm text-bone">
                    {p.withMedia}
                  </label>
                  <p className="mt-0.5 text-xs leading-relaxed text-bone-faint">
                    {data.mediaOnDisk ? p.withMediaHint : data.mediaStore === 's3' ? p.mediaInBucket : p.mediaNone}
                  </p>
                </div>
                <Toggle
                  id="backup-media"
                  checked={data.includeMedia && data.mediaOnDisk}
                  label={p.withMedia}
                  disabled={set.isPending || !data.mediaOnDisk}
                  onChange={(next) => set.mutate({ key: 'backup.includeMedia', value: String(next) })}
                />
              </li>
            </ul>
            {set.isError ? (
              <p role="alert" className="border-t border-rule px-5 py-3 text-sm text-status-error">
                {set.error instanceof ApiError ? set.error.message : t.common.actionFailed}
              </p>
            ) : null}
          </Panel>

          <Panel className="rise" style={{ animationDelay: '120ms' }}>
            <PanelHead
              title={p.archives}
              action={
                <span className="flex items-center gap-4">
                  {data.archives.length ? <TableOptions table={table} /> : null}
                  <Link to="/admin/jobs" className="label transition-colors duration-150 hover:text-vermillion">
                    {t.admin.jobs}
                  </Link>
                </span>
              }
            />
            <div className="flex flex-wrap items-center gap-x-5 gap-y-3 border-b border-rule px-5 py-4">
              <Button
                variant="primary"
                disabled={backUp.isPending || !!data.running}
                onClick={() => backUp.mutate()}
              >
                {backUp.isPending || data.running ? <Spinner className="size-4" /> : <Glyph name="shield" className="size-4" />}
                {p.backUpNow}
              </Button>
              {data.mediaOnDisk ? (
                <span className="flex items-center gap-3 text-sm text-bone-dim">
                  <Toggle id="backup-now-media" checked={withMedia} label={p.withMedia} onChange={setWithMedia} />
                  <label htmlFor="backup-now-media">{p.withMedia}</label>
                </span>
              ) : null}
              <p role="status" className="text-sm text-bone-dim">
                {backUp.isError
                  ? backUp.error instanceof ApiError
                    ? backUp.error.message
                    : t.common.actionFailed
                  : data.running
                    ? p.running
                    : (notice ?? '')}
              </p>
            </div>
            {data.restore.state !== 'idle' && data.restore.state !== 'running' ? (
              <LastRestore state={data.restore} />
            ) : null}
            {data.archives.length === 0 ? (
              <EmptyState size="line" title={p.none} />
            ) : (
              <DataTable table={table} label={p.archives}>
                <THead>
                  <Th column="made" />
                  <Th column="why" />
                  <Th column="holds" />
                  <Th column="size" />
                  <Th column="actions">
                    <span className="sr-only">{p.colActions}</span>
                  </Th>
                </THead>
                <TBody>
                  {archives.map((archive) => (
                    <Tr key={archive.name} row={archive.name}>
                      <Td column="made">
                        <span className="block text-bone">{fmt.dateTime(archive.createdAt, locale)}</span>
                        <span className="block font-mono text-xs break-all text-bone-faint">{archive.name}</span>
                      </Td>
                      <Td column="why">
                        <Reason archive={archive} />
                      </Td>
                      <Td column="holds">
                        <Holds archive={archive} />
                      </Td>
                      <Td column="size">
                        <span className="font-mono text-xs whitespace-nowrap tabular-nums">
                          {fmt.bytes(archive.sizeBytes, locale)}
                        </span>
                      </Td>
                      <Td column="actions">
                        <Actions archive={archive} onDelete={setDeleting} onRestore={setRestoring} />
                      </Td>
                    </Tr>
                  ))}
                </TBody>
              </DataTable>
            )}
          </Panel>

          <Dialog
            open={deleting !== null}
            title={p.deleteTitle}
            onClose={() => setDeleting(null)}
            footer={
              <>
                <Button onClick={() => setDeleting(null)}>{t.common.cancel}</Button>
                <Button
                  variant="danger"
                  disabled={remove.isPending}
                  onClick={() => deleting && remove.mutate(deleting)}
                >
                  {remove.isPending ? <Spinner className="size-4" /> : <Glyph name="trash" className="size-4" />}
                  {deleting?.present === false ? p.forget : p.delete}
                </Button>
              </>
            }
          >
            {deleting ? (deleting.present ? p.deleteBody(deleting.name) : p.forgetBody(deleting.name)) : null}
            {remove.isError ? (
              <p role="alert" className="mt-3 text-sm text-status-error">
                {remove.error instanceof ApiError ? remove.error.message : t.common.actionFailed}
              </p>
            ) : null}
          </Dialog>

          <Dialog
            open={restoring !== null}
            title={restoring ? p.restoreTitle(fmt.dateTime(restoring.createdAt, locale) ?? restoring.name) : ''}
            onClose={() => {
              setRestoring(null)
              setPassphrase('')
              restore.reset()
            }}
            footer={
              <>
                <Button
                  onClick={() => {
                    setRestoring(null)
                    setPassphrase('')
                    restore.reset()
                  }}
                >
                  {t.common.cancel}
                </Button>
                <Button
                  variant="danger"
                  disabled={restore.isPending || (!!restoring?.encrypted && !data.encrypted && !passphrase)}
                  onClick={() => restoring && restore.mutate(restoring)}
                >
                  {restore.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
                  {p.restoreGo}
                </Button>
              </>
            }
          >
            <div className="space-y-3 text-sm leading-relaxed">
              <p className="text-bone">{p.restoreBody}</p>
              <ol className="list-decimal space-y-2 pl-5 text-bone-dim">
                <li>{p.restoreSafety}</li>
                <li>{p.restoreDoors}</li>
                <li>{p.restoreSignOut}</li>
              </ol>
              {data.place === 'bucket' ? <p className="text-bone-dim">{p.restoreOthers}</p> : null}
              {restoring?.encrypted ? (
                <FormField
                  label={p.passphrase}
                  htmlFor="backup-passphrase"
                  hint={data.encrypted ? p.passphraseHint : p.passphraseNeeded}
                >
                  <Input
                    id="backup-passphrase"
                    type="password"
                    autoComplete="off"
                    value={passphrase}
                    onChange={(event) => setPassphrase(event.target.value)}
                  />
                </FormField>
              ) : null}
              {restore.isError ? (
                <p role="alert" className="text-status-error">
                  {restore.error instanceof ApiError ? restore.error.message : t.common.actionFailed}
                </p>
              ) : null}
            </div>
          </Dialog>
        </div>
      )}
    </div>
  )
}

/** Why an archive was made, and whether it is where it should be. */
function Reason({ archive }: { archive: Archive }) {
  const { t } = useI18n()
  const p = t.admin.backupsPage
  return (
    <span className="flex flex-wrap items-center gap-1.5">
      <Chip tone={archive.reason === 'safety' ? 'manual' : archive.reason === 'scheduled' ? 'neutral' : 'provider'}>
        {p.reasons[archive.reason] ?? archive.reason}
      </Chip>
      {archive.encrypted ? (
        <Chip>
          <Glyph name="lock" className="size-3" />
          {p.encryptedChip}
        </Chip>
      ) : null}
      {!archive.present ? (
        <span title={p.missingHint}>
          <Chip tone="accent">{p.missing}</Chip>
        </span>
      ) : null}
      {!archive.described ? (
        <span title={p.foundHint}>
          <Chip>{p.found}</Chip>
        </span>
      ) : null}
    </span>
  )
}

/** What an archive holds, as far as this database knows. */
function Holds({ archive }: { archive: Archive }) {
  const { t, locale } = useI18n()
  const p = t.admin.backupsPage
  const engine = archive.engine ? (p.engines[archive.engine] ?? archive.engine) : null
  return (
    <span className="block text-xs leading-relaxed text-bone-dim">
      {engine ? p.database(engine) : p.unknownDatabase}
      {archive.includesMedia ? ` ${p.withMediaCount(fmt.count(archive.mediaFiles, locale), archive.mediaFiles)}` : ''}
      {archive.reason === 'upgrade' && archive.serverVersion && archive.upgradeTo !== null ? (
        <span className="block text-bone-faint">
          {p.upgradeDetail(archive.serverVersion, String(archive.upgradeTo).padStart(4, '0'))}
        </span>
      ) : null}
      {archive.createdBy ? <span className="block text-bone-faint">{p.by(archive.createdBy)}</span> : null}
    </span>
  )
}

/** Download, restore, delete: what can be done with an archive. */
function Actions({
  archive,
  onDelete,
  onRestore,
}: {
  archive: Archive
  onDelete: (archive: Archive) => void
  onRestore: (archive: Archive) => void
}) {
  const { t } = useI18n()
  const p = t.admin.backupsPage
  return (
    <span className="flex flex-wrap items-center justify-end gap-2">
      {archive.present ? (
        <>
          <ButtonLink size="sm" href={`/api/v1/admin/backups/${segment(archive.name)}/download`} download={archive.name}>
            <Glyph name="download" className="size-4" />
            {p.download}
          </ButtonLink>
          <Button size="sm" onClick={() => onRestore(archive)}>
            <Glyph name="refresh" className="size-4" />
            {p.restore}
          </Button>
        </>
      ) : null}
      <Button size="sm" variant="quiet" onClick={() => onDelete(archive)}>
        <Glyph name="trash" className="size-4" />
        {archive.present ? p.delete : p.forget}
      </Button>
    </span>
  )
}

/** How the last restore went, when one was asked for. */
function LastRestore({ state }: { state: RestoreState }) {
  const { t, locale } = useI18n()
  const p = t.admin.backupsPage
  const when = fmt.dateTime(state.finishedAt ?? state.startedAt, locale) ?? ''
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-rule px-5 py-3 text-sm">
      <Label>{p.lastRestore}</Label>
      <Lamp tone={state.state === 'succeeded' ? 'moss' : 'vermillion'}>
        {p.lastRestoreOf(state.archive ?? '', when)}
      </Lamp>
      {state.state === 'failed' ? (
        <span className="text-xs break-words text-vermillion">
          {p.lastRestoreFailed}
          {state.error ? ` — ${state.error}` : ''}
        </span>
      ) : null}
    </div>
  )
}

/** A restore, followed from the page that started it. */
function Follow({ following, onClose }: { following: Following; onClose: () => void }) {
  const { t, locale } = useI18n()
  const p = t.admin.backupsPage

  if (following.stage === 'signedOut') {
    return (
      <Panel className="rise mb-6">
        <PanelHead title={p.doneTitle} />
        <div className="space-y-3 px-5 py-4 text-sm">
          <p className="flex items-start gap-2 text-bone">
            <Glyph name="check" className="mt-0.5 size-4 shrink-0 text-status-ok" />
            {p.signInAgain}
          </p>
          {/* A page of its own: what this one held went with the session. */}
          <ButtonLink href="/login">{p.signIn}</ButtonLink>
        </div>
      </Panel>
    )
  }

  if (following.stage === 'under') {
    const under = following.under
    const share = under && under.phase === 'copying' && under.steps > 0 ? (under.step + 1) / under.steps : null
    return (
      <Panel className="rise mb-6">
        <PanelHead title={p.restoring} />
        <div role="status" aria-live="polite" className="space-y-3 px-5 py-4 text-sm">
          <p className="flex items-center gap-2 text-bone">
            <Spinner className="size-4" />
            {under ? (p.phases[under.phase] ?? under.phase) : p.waiting}
          </p>
          {under && under.phase === 'copying' && under.table ? (
            <p className="font-mono text-xs text-bone-faint">{p.table(under.table, under.step + 1, under.steps)}</p>
          ) : null}
          {share !== null ? (
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-rule">
              <div className="h-full rounded-full bg-vermillion transition-[width] duration-500" style={{ width: `${Math.round(share * 100)}%` }} />
            </div>
          ) : null}
          <p className="max-w-prose text-xs leading-relaxed text-bone-faint">{p.restoreDoors}</p>
        </div>
      </Panel>
    )
  }

  const { state } = following
  const restored = state.restored
  return (
    <Panel className="rise mb-6">
      <PanelHead
        title={state.state === 'succeeded' ? p.doneTitle : p.failedTitle}
        action={
          <Button size="sm" variant="quiet" onClick={onClose}>
            {t.common.close}
          </Button>
        }
      />
      <div role="status" className="space-y-2 px-5 py-4 text-sm">
        {state.state === 'succeeded' && restored ? (
          <>
            <p className="flex items-start gap-2 text-bone">
              <Glyph name="check" className="mt-0.5 size-4 shrink-0 text-status-ok" />
              {p.done(fmt.count(restored.rows, locale))}
              {restored.mediaRestored > 0 ? ` ${p.doneMedia(fmt.count(restored.mediaRestored, locale))}` : ''}
            </p>
            {restored.media && (restored.media.missing > 0 || restored.media.unreferenced > 0) ? (
              <p className="text-bone-dim">
                {p.mediaCheck(fmt.count(restored.media.missing, locale), fmt.count(restored.media.unreferenced, locale))}
              </p>
            ) : null}
            {restored.safety ? <p className="text-bone-dim">{p.undo(restored.safety)}</p> : null}
          </>
        ) : state.state === 'succeeded' ? (
          <p className="flex items-start gap-2 text-bone">
            <Glyph name="check" className="mt-0.5 size-4 shrink-0 text-status-ok" />
            {p.doneTitle}
          </p>
        ) : (
          <p role="alert" className="flex items-start gap-2 text-status-error">
            <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
            <span>
              {p.failed}
              {state.error ? <span className="block text-xs break-words text-bone-dim">{state.error}</span> : null}
            </span>
          </p>
        )}
      </div>
    </Panel>
  )
}

/** How many of the schedule's archives are kept: typed, then saved. */
function Keep({ value, disabled, onSave }: { value: number; disabled: boolean; onSave: (keep: number) => void }) {
  const { t } = useI18n()
  const p = t.admin.backupsPage
  const [draft, setDraft] = useState(String(value))
  useEffect(() => setDraft(String(value)), [value])
  const keep = Number(draft)
  const valid = Number.isInteger(keep) && keep >= 1 && keep <= 365
  return (
    <form
      className="flex flex-wrap items-end gap-3"
      onSubmit={(event) => {
        event.preventDefault()
        if (valid && keep !== value) onSave(keep)
      }}
    >
      <div>
        <label htmlFor="backup-keep" className="block text-sm text-bone">
          {p.keep}
        </label>
        <Input
          id="backup-keep"
          type="number"
          inputMode="numeric"
          min={1}
          max={365}
          className="mt-2 w-28"
          value={draft}
          disabled={disabled}
          aria-invalid={!valid}
          onChange={(event) => setDraft(event.target.value)}
        />
      </div>
      <Button type="submit" size="sm" disabled={disabled || !valid || keep === value}>
        {t.common.save}
      </Button>
      <p className="w-full max-w-prose text-xs leading-relaxed text-bone-faint">{p.keepHint}</p>
    </form>
  )
}
