/**
 * Who changed what.
 *
 * Action names stay in the server's own spelling, set in mono: `override.set`
 * is an identifier a person will grep the logs for, and translating it would
 * break the one thing this table is for. What it means is said beside it, in
 * the reader's language, and a work is named by its title with its id kept —
 * `01a0…#item/genres` was the whole of a lock's line.
 */

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  EmptyState,
  Glyph,
  Input,
  Label,
  Panel,
  Select,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Th,
  Tr,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useSettled } from '../../lib/debounce'
import { useI18n } from '../../lib/i18n'
import type { AuditEntry, AuditResponse } from '../../lib/types'

const PAGE = 100

export function Audit() {
  const { t, locale } = useI18n()

  const [action, setAction] = useState('')
  const [actor, setActor] = useState('')
  const [page, setPage] = useState(0)

  const settledActor = useSettled(actor)

  const log = useQuery({
    queryKey: ['audit', action, settledActor, page],
    queryFn: () =>
      api.get<AuditResponse>(
        `/audit${query({ action, actor: settledActor, limit: PAGE, offset: page * PAGE })}`,
      ),
    // A trail is only useful if it is current.
    refetchInterval: 30_000,
  })

  const filtered = Boolean(action || actor)

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {t.admin.audit}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
          {t.admin.trail.lead}
        </p>
      </header>

      <div className="rise mb-6 flex flex-wrap items-end gap-3 border-y border-rule py-4">
        <div className="min-w-44">
          <label htmlFor="audit-action" className="label mb-1.5 block">
            {t.admin.trail.action}
          </label>
          <Select
            id="audit-action"
            value={action}
            onChange={(event) => {
              setAction(event.target.value)
              setPage(0)
            }}
          >
            <option value="">{t.admin.trail.allActions}</option>
            {(log.data?.actions ?? []).map((name) => (
              <option key={name} value={name}>
                {t.admin.trail.says[name] ? `${t.admin.trail.says[name]} · ${name}` : name}
              </option>
            ))}
          </Select>
        </div>

        <div className="min-w-44 flex-1">
          <label htmlFor="audit-actor" className="label mb-1.5 block">
            {t.admin.trail.actor}
          </label>
          <Input
            id="audit-actor"
            value={actor}
            onChange={(event) => {
              setActor(event.target.value)
              setPage(0)
            }}
          />
          <p className="mt-1 text-xs text-bone-faint">{t.admin.trail.actorHint}</p>
        </div>

        <span className="ml-auto flex items-center gap-3 self-center">
          {log.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
          {log.data ? (
            <span className="font-mono text-xs text-bone-faint tabular-nums">
              {t.admin.trail.range(
                log.data.entries.length === 0 ? 0 : page * PAGE + 1,
                page * PAGE + log.data.entries.length,
                log.data.total,
              )}
            </span>
          ) : null}
        </span>
      </div>

      <Panel className="rise overflow-hidden" style={{ animationDelay: '60ms' }}>
        {log.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 10 }, (_, index) => (
              <Skeleton key={index} className="h-8 w-full" />
            ))}
          </div>
        ) : log.isError ? (
          <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {t.admin.trail.loadFailed}
          </p>
        ) : log.data.entries.length === 0 ? (
          <EmptyState
            title={t.admin.trail.empty}
            hint={filtered ? t.admin.trail.emptyFilteredHint : t.admin.trail.emptyHint}
          />
        ) : (
          <TableScroll label={t.admin.audit}>
            <table className="w-full min-w-[20rem] border-collapse text-left">
              <thead>
                <tr>
                  <Th>{t.admin.trail.colWhen}</Th>
                  <Th>{t.admin.trail.colAction}</Th>
                  <Th className="hidden sm:table-cell">{t.admin.trail.colTarget}</Th>
                  <Th align="right" className="hidden md:table-cell">
                    {t.admin.trail.colActor}
                  </Th>
                </tr>
              </thead>
              <tbody>
                {log.data.entries.map((entry) => (
                  <Row key={entry.id} entry={entry} locale={locale} />
                ))}
              </tbody>
            </table>
          </TableScroll>
        )}
      </Panel>

      {log.data && (log.data.total > PAGE || page > 0) ? (
        <div className="mt-4 flex items-center justify-between gap-3">
          <Button onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
            <Glyph name="chevronLeft" className="size-4" />
            {t.admin.trail.newer}
          </Button>
          <Button
            onClick={() => setPage((p) => p + 1)}
            disabled={(page + 1) * PAGE >= log.data.total}
          >
            {t.admin.trail.older}
            <Glyph name="chevronRight" className="size-4" />
          </Button>
        </div>
      ) : null}
    </div>
  )
}

function Row({ entry, locale }: { entry: AuditEntry; locale: string }) {
  const { t } = useI18n()

  return (
    <Tr
      className={cn(
        entry.action === 'auth.sign_in_failed' ? 'bg-vermillion/[0.06]' : '',
        // Locking and unlocking carry the brass edge they carry everywhere else.
        entry.action.startsWith('override.') ? 'border-l-2 border-brass' : '',
      )}
    >
      <Td className="whitespace-nowrap">
        <span
          className="font-mono text-xs text-bone-faint tabular-nums"
          title={fmt.dateTime(entry.at, locale)}
        >
          {fmt.relative(entry.at, locale)}
        </span>
      </Td>

      <Td>
        <span className="block text-sm text-bone">{t.admin.trail.says[entry.action] ?? entry.action}</span>
        <Chip tone={tone(entry.action)} className="mt-1">
          {entry.action}
        </Chip>
        {/* The target column is not drawn on a phone: what it says, here. */}
        {entry.target || entry.detail ? (
          <span className="mt-1 block truncate text-xs text-bone-dim sm:hidden">
            {entry.target ? <Target entry={entry} /> : (t.admin.runs.notes[entry.detail!] ?? entry.detail)}
          </span>
        ) : null}
      </Td>

      <Td className="hidden w-full max-w-0 sm:table-cell">
        <span className="block truncate text-sm text-bone">
          <Target entry={entry} />
        </span>
        {entry.detail ? (
          <span className="block truncate text-xs text-bone-faint">
            {t.admin.runs.notes[entry.detail] ?? entry.detail}
          </span>
        ) : null}
      </Td>

      <Td align="right" className="hidden whitespace-nowrap md:table-cell">
        <span className="block font-mono text-xs text-bone-dim">
          {entry.actor ?? t.admin.trail.anonymous}
        </span>
        {entry.ip ? (
          <span className="block font-mono text-[0.6875rem] text-bone-faint tabular-nums">
            {entry.ip}
          </span>
        ) : null}
      </Td>
    </Tr>
  )
}

/**
 * What an entry acted on, as a reader names it: a work by its title and the
 * field of it, a season or an episode by its number — and the raw target, in
 * mono, where there is nothing better to say.
 */
function Target({ entry }: { entry: AuditEntry }) {
  const { t } = useI18n()
  if (!entry.target) return <>—</>
  if (!entry.work) return <span className="font-mono text-xs">{entry.target}</span>

  // `{id}#{scope}/{field}`: `item/genres`, `season:2/title`, `episode:5x25/airDate`.
  const [, path = ''] = entry.target.split('#')
  const [scope = '', field] = path.split('/')
  const [, numbers = ''] = scope.split(':')
  const [season, episode] = numbers.split('x').map(Number)
  const where =
    scope.startsWith('episode:') && season !== undefined && episode !== undefined
      ? `S${String(season).padStart(2, '0')}E${String(episode).padStart(2, '0')}`
      : scope.startsWith('season:') && season !== undefined
        ? t.admin.trail.season(season)
        : undefined
  const named = field ? ((t.labels.fields as Record<string, string>)[field] ?? field) : undefined

  return (
    <>
      <Link
        to={`/admin/catalogue/${entry.work.id}`}
        className="underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
      >
        {entry.work.title}
      </Link>
      {[where, named].filter(Boolean).map((part) => (
        <span key={part} className="text-bone-dim">
          {' · '}
          {part}
        </span>
      ))}
    </>
  )
}

/** The accent is spent on the two lines an operator should stop at. */
function tone(action: string) {
  if (action === 'auth.sign_in_failed') return 'accent' as const
  if (action === 'item.deleted' || action === 'client.revoked') return 'accent' as const
  if (action.startsWith('override.')) return 'manual' as const
  if (action.startsWith('auth.')) return 'provider' as const
  return 'neutral' as const
}
