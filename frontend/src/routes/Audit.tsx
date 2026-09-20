import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import type { AuditEntry, AuditResponse } from '../lib/types'
import {
  Alert,
  Button,
  Display,
  Empty,
  Input,
  Label,
  Mono,
  Panel,
  Select,
  Spinner,
  Tag,
} from '../components/ui'

const PAGE = 100

export function Audit() {
  const [action, setAction] = useState('')
  const [actor, setActor] = useState('')
  const [page, setPage] = useState(0)

  const log = useQuery({
    queryKey: ['audit', action, actor, page],
    queryFn: () =>
      api.get<AuditResponse>(
        `/audit${query({ action, actor, limit: PAGE, offset: page * PAGE })}`,
      ),
    // A trail is only useful if it is current.
    refetchInterval: 30_000,
  })

  return (
    <div className="mx-auto max-w-5xl">
      <header className="reveal mb-8">
        <Label>Audit</Label>
        <Display className="mt-2">Who changed what</Display>
        <p className="mt-3 max-w-prose text-[13px] leading-relaxed text-dim">
          Every action that changes what this server serves. Reads are not recorded — one row per
          metadata request would bury everything that matters. Failed sign-ins <em>are</em>, because
          a run of them is the one thing here worth an alert.
        </p>
      </header>

      <div className="reveal mb-6 flex flex-wrap items-center gap-3" style={{ animationDelay: '60ms' }}>
        <Select
          value={action}
          onChange={(event) => {
            setAction(event.target.value)
            setPage(0)
          }}
        >
          <option value="">all actions</option>
          {(log.data?.actions ?? []).map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </Select>

        <Input
          placeholder="Filter by actor, e.g. admin:you"
          value={actor}
          onChange={(event) => {
            setActor(event.target.value)
            setPage(0)
          }}
          className="max-w-xs"
        />

        {log.isFetching && <Spinner />}

        {log.data && (
          <Mono className="ml-auto text-faint">
            {page * PAGE + 1}–{page * PAGE + log.data.entries.length} of {log.data.total}
          </Mono>
        )}
      </div>

      <Panel className="reveal overflow-hidden" style={{ animationDelay: '100ms' }}>
        {log.isPending ? (
          <div className="px-5 py-16 text-center">
            <Spinner />
          </div>
        ) : log.isError ? (
          <div className="p-5">
            <Alert>Could not load the audit trail.</Alert>
          </div>
        ) : log.data.entries.length === 0 ? (
          <Empty
            title="Nothing recorded"
            hint={
              action || actor
                ? 'No entry matches that filter.'
                : 'Entries appear as soon as anything is created, edited or revoked.'
            }
          />
        ) : (
          <ul className="divide-y divide-line">
            {log.data.entries.map((entry) => (
              <Row key={entry.id} entry={entry} />
            ))}
          </ul>
        )}
      </Panel>

      {log.data && (log.data.total > PAGE || page > 0) && (
        <div className="mt-4 flex items-center justify-between">
          <Button onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
            ← Newer
          </Button>
          <Button
            onClick={() => setPage((p) => p + 1)}
            disabled={(page + 1) * PAGE >= log.data.total}
          >
            Older →
          </Button>
        </div>
      )}
    </div>
  )
}

function Row({ entry }: { entry: AuditEntry }) {
  const when = new Date(entry.at)
  const valid = !Number.isNaN(when.getTime())

  return (
    <li
      className={cn(
        'edge grid items-baseline gap-x-4 gap-y-1 px-5 py-2.5 sm:grid-cols-[132px_190px_1fr]',
        entry.action === 'auth.sign_in_failed' && 'bg-rust/[0.05]',
        isManualEdit(entry.action) && 'edge-locked',
      )}
    >
      <Mono className="text-[11px] whitespace-nowrap text-faint">
        {valid ? when.toLocaleString(undefined, { dateStyle: 'short', timeStyle: 'medium' }) : entry.at}
      </Mono>

      <div className="flex items-center gap-2">
        <Tag tone={toneFor(entry.action)}>{entry.action}</Tag>
      </div>

      <div className="min-w-0">
        <div className="flex flex-wrap items-baseline gap-x-2">
          {entry.target && (
            <Mono className="text-[12px] break-all text-paper">{entry.target}</Mono>
          )}
          {entry.detail && <span className="text-[12px] text-dim">{entry.detail}</span>}
        </div>
        <div className="mt-0.5 flex flex-wrap gap-x-3">
          <Mono className="text-[11px] text-faint">{entry.actor ?? 'anonymous'}</Mono>
          {entry.ip && <Mono className="text-[11px] text-faint">{entry.ip}</Mono>}
        </div>
      </div>
    </li>
  )
}

/** Locking and unlocking get the amber edge, like everywhere else in the UI. */
function isManualEdit(action: string) {
  return action.startsWith('override.')
}

function toneFor(action: string) {
  if (action === 'auth.sign_in_failed') return 'bad' as const
  if (action.startsWith('override.')) return 'manual' as const
  if (action.startsWith('auth.')) return 'auto' as const
  if (action === 'item.deleted' || action === 'client.revoked') return 'bad' as const
  return 'neutral' as const
}
