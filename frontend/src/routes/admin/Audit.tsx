/**
 * Who changed what.
 *
 * Action names stay in the server's own spelling, set in mono: `override.set`
 * is an identifier a person will grep the logs for, and translating it would
 * break the one thing this table is for. Everything around it — the time, the
 * actor, the empty state — is in the reader's language.
 */

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

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
import { useI18n } from '../../lib/i18n'
import type { AuditEntry, AuditResponse } from '../../lib/types'

const PAGE = 100

export function Audit() {
  const { t, locale } = useI18n()

  const [action, setAction] = useState('')
  const [actor, setActor] = useState('')
  const [page, setPage] = useState(0)

  const log = useQuery({
    queryKey: ['audit', action, actor, page],
    queryFn: () =>
      api.get<AuditResponse>(`/audit${query({ action, actor, limit: PAGE, offset: page * PAGE })}`),
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
                {name}
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
          <TableScroll>
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
        <Chip tone={tone(entry.action)}>{entry.action}</Chip>
        <span className="mt-1 block truncate font-mono text-[0.6875rem] text-bone-faint sm:hidden">
          {entry.target ?? entry.detail ?? ''}
        </span>
      </Td>

      <Td className="hidden w-full max-w-0 sm:table-cell">
        <span className="block truncate font-mono text-xs text-bone">{entry.target ?? '—'}</span>
        {entry.detail ? (
          <span className="block truncate text-xs text-bone-faint">{entry.detail}</span>
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

/** The accent is spent on the two lines an operator should stop at. */
function tone(action: string) {
  if (action === 'auth.sign_in_failed') return 'accent' as const
  if (action === 'item.deleted' || action === 'client.revoked') return 'accent' as const
  if (action.startsWith('override.')) return 'manual' as const
  if (action.startsWith('auth.')) return 'provider' as const
  return 'neutral' as const
}
