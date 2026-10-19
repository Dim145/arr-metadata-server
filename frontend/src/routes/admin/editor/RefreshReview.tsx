/**
 * What a refresh would change, before it is made.
 *
 * The sources are asked as a refresh asks them, and nothing is written: what
 * they would write over is listed — the values a lock holds are not, since
 * no refresh reaches them — each with what it holds and what it would hold.
 * Any field a lock can hold can be ticked to be locked as it stands first;
 * the refresh then leaves it alone. The refresh itself is run from here, once
 * the person has seen what it does.
 */

import { useMutation, useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { Button, Dialog, Glyph, Skeleton, Spinner } from '../../../components/ui'
import { ApiError, api, query, segment } from '../../../lib/api'
import { useI18n, type Dict } from '../../../lib/i18n'
import { providerName } from '../../../lib/labels'
import type { FieldChange, FieldRegistry, MediaItem, RefreshPreview } from '../../../lib/types'

import { fieldLabel } from './FieldRow'
import { MOVED, episodeCode, kindOf, placeOf, revertible, shown } from './journal'

/** A change, by where it is and what of it. */
const keyOf = (change: FieldChange) => `${change.scope}/${change.field}`

export function RefreshReview({
  work,
  registry,
  onClose,
  onRefresh,
}: {
  work: MediaItem
  registry: FieldRegistry
  onClose: () => void
  /** The refresh in earnest: what the masthead's button did before it asked first. */
  onRefresh: () => void
}) {
  const { t } = useI18n()
  const r = t.admin.editor.review
  const [locking, setLocking] = useState<Set<string>>(() => new Set())

  // Asked once a dialog: each opening is a new one, and a preview kept
  // would show what the sources said then.
  const preview = useQuery({
    queryKey: ['item', work.id, 'preview'],
    queryFn: () => api.post<RefreshPreview>(`/items/${segment(work.id)}/refresh${query({ dryRun: 1 })}`),
    gcTime: 0,
    staleTime: 0,
    retry: false,
  })
  const changes = preview.data?.changes ?? []
  const lockable = changes.filter((change) => revertible(change.scope, change.field, registry))

  // Each value ticked locked as it stands, one after the other, then the
  // refresh: a lock that cannot be set stops it before anything moves.
  const run = useMutation({
    mutationFn: async () => {
      for (const change of lockable.filter((one) => locking.has(keyOf(one)))) {
        await api.put(`/items/${segment(work.id)}/overrides`, {
          scope: change.scope,
          field: change.field,
          value: change.before ?? null,
        })
      }
    },
    onSuccess: () => {
      onRefresh()
      onClose()
    },
  })

  const toggle = (change: FieldChange, on: boolean) =>
    setLocking((held) => {
      const next = new Set(held)
      if (on) next.add(keyOf(change))
      else next.delete(keyOf(change))
      return next
    })

  return (
    <Dialog
      open
      size="lg"
      title={r.title}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t.common.cancel}</Button>
          <Button variant="primary" disabled={run.isPending || preview.isPending} onClick={() => run.mutate()}>
            {run.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
            {locking.size ? r.lockAndRun(locking.size) : r.run}
          </Button>
        </>
      }
    >
      {preview.isPending ? (
        <div className="space-y-2" aria-busy="true">
          <p className="text-sm text-bone-dim">{r.asking}</p>
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-4/5" />
        </div>
      ) : preview.isError ? (
        <p role="alert" className="flex items-start gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
          {preview.error instanceof ApiError ? preview.error.message : r.failed}
        </p>
      ) : changes.length === 0 ? (
        <p className="text-sm text-bone-dim">{r.nothing}</p>
      ) : (
        <>
          <p className="mb-3 text-sm leading-relaxed text-bone-dim">{r.lead(changes.length)}</p>
          {lockable.length > 1 ? (
            <Button
              size="sm"
              variant="quiet"
              className="-ml-3 mb-2"
              onClick={() =>
                setLocking((held) => (held.size === lockable.length ? new Set() : new Set(lockable.map(keyOf))))
              }
            >
              <Glyph name="lock" className="size-3.5" />
              {locking.size === lockable.length ? r.lockNone : r.lockAll}
            </Button>
          ) : null}
          <ul className="divide-y divide-rule rounded-card border border-rule">
            {changes.map((change) => (
              <Proposed
                key={keyOf(change)}
                change={change}
                registry={registry}
                lockable={revertible(change.scope, change.field, registry)}
                locked={locking.has(keyOf(change))}
                onLock={(on) => toggle(change, on)}
              />
            ))}
          </ul>
        </>
      )}
      {run.isError ? (
        <p role="alert" className="mt-3 flex items-start gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
          {run.error instanceof ApiError ? run.error.message : t.common.actionFailed}
        </p>
      ) : null}
    </Dialog>
  )
}

/** One value the refresh would write over, and the box that keeps it as it is. */
function Proposed({
  change,
  registry,
  lockable,
  locked,
  onLock,
}: {
  change: FieldChange
  registry: FieldRegistry
  lockable: boolean
  locked: boolean
  onLock: (on: boolean) => void
}) {
  const { t } = useI18n()
  const r = t.admin.editor.review
  const words = { yes: t.common.yes, no: t.common.no }
  const place = placeOf(change.scope)
  const where =
    place.kind === 'episode'
      ? episodeCode(place.season, place.episode)
      : place.kind === 'season'
        ? t.admin.editor.journal.season(place.season)
        : undefined
  const id = `review-${change.scope.replace(/\W/g, '-')}-${change.field}`

  return (
    <li className="px-4 py-3">
      <p className="flex flex-wrap items-baseline gap-x-2 text-sm text-bone">
        {where ? <span className="font-mono text-xs text-bone-dim">{where}</span> : null}
        <span className="font-medium">{named(change, registry, t)}</span>
        {change.provider ? <span className="text-xs text-bone-dim">· {providerName(change.provider)}</span> : null}
      </p>
      <p className="mt-1 flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 text-sm leading-relaxed">
        <del className="line-clamp-2 min-w-0 break-words text-bone-faint decoration-bone-faint/60">
          {shown(change.before, words) ?? <i>{t.admin.editor.journal.nothing}</i>}
        </del>
        <Glyph name="chevronRight" className="size-3.5 shrink-0 self-center text-bone-faint" />
        <ins className="line-clamp-2 min-w-0 break-words text-bone no-underline">
          {shown(change.after, words) ?? <i>{t.admin.editor.journal.nothing}</i>}
        </ins>
      </p>
      {lockable ? (
        <label htmlFor={id} className="mt-1.5 inline-flex min-h-11 cursor-pointer items-center gap-2 text-xs text-bone-dim">
          <input
            id={id}
            type="checkbox"
            className="size-4 accent-brass"
            checked={locked}
            onChange={(event) => onLock(event.target.checked)}
          />
          {r.lockFirst}
        </label>
      ) : null}
    </li>
  )
}

/** A value's name, as the journal names it. */
function named(change: FieldChange, registry: FieldRegistry, t: Dict): string {
  const j = t.admin.editor.journal
  if (change.field === MOVED) return j.fields.number
  const kind = kindOf(change.scope, change.field)
  if (kind) return `${j.fields[kind]} · ${change.before === null ? j.came[kind] : j.went[kind]}`
  const place = placeOf(change.scope)
  const defs = place.kind === 'season' ? registry.season : place.kind === 'episode' ? registry.episode : registry.item
  const def = defs.find((one) => one.name === change.field)
  return def ? fieldLabel(def, t) : change.field
}
