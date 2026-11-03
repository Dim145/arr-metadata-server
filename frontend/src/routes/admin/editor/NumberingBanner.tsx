/**
 * While a series is numbered otherwise than by TheTVDB: since when, why,
 * and — for whoever may edit the work — the way back, which refreshes the
 * series at once.
 *
 * Two cases. TMDB numbers a series TheTVDB stopped answering for, until
 * TheTVDB answers again; the way back clears the fallback. And a series
 * TheTVDB no longer knows — deleted, merged into another, or answered twice
 * that it has none — is served numbered as it was then, frozen, the episodes
 * aired since placed after it; the way back takes TheTVDB's numbering again
 * where TheTVDB knows the series again, under the id it kept for one merged
 * into another — which Sonarr renumbers by, as it is cautioned beside the
 * button. Still nothing from TheTVDB, the banner says so.
 *
 * Whoever keeps the catalogue is told besides, quietly, when TheTVDB has said
 * once that it has no such series: nothing changes until it says so again.
 *
 * On the work's page and in its editor alike: the numbering is what Sonarr
 * and Plex are served, and a reader who finds the seasons cut otherwise than
 * TheTVDB cuts them is owed the reason.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { Button, Glyph, Spinner } from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import { useI18n } from '../../../lib/i18n'
import type { Orders } from '../../../lib/types'

import { backSaid, numberingSaid } from './numbering'

export function NumberingBanner({
  workId,
  orders,
  canWrite,
  onChanged,
  className,
}: {
  workId: string
  /** The work's orders, as `GET /items/{id}/orders` answers: their numbering says who numbers it. */
  orders: Orders | undefined
  canWrite: boolean
  /** Told once the numbering was given back, to read the work again. */
  onChanged?: () => void
  className?: string
}) {
  const { t, locale } = useI18n()
  const w = t.work.tmdbNumbering
  const queryClient = useQueryClient()
  const [said, setSaid] = useState<string>()

  const back = useMutation({
    mutationFn: () => api.post<Orders>(`/items/${encodeURIComponent(workId)}/numbering/restore`),
    onSuccess: (answer) => {
      setSaid(backSaid(orders?.numbering, answer.numbering, w))
      queryClient.setQueryData(['orders', workId], answer)
      void queryClient.invalidateQueries({ queryKey: ['work', workId] })
      onChanged?.()
    },
  })

  const shown = numberingSaid(orders?.numbering, w, locale)
  // TheTVDB's word that it has no such series changes nothing yet: for
  // whoever keeps the catalogue, not for every reader.
  if (!shown || (shown.tone === 'note' && !canWrite)) {
    // Given back: said once, where the banner was.
    return said ? (
      <p role="status" className={cn('flex items-center gap-2 text-sm text-status-ok', className)}>
        <Glyph name="check" className="size-4 shrink-0" />
        {said}
      </p>
    ) : null
  }

  if (shown.tone === 'note') {
    return (
      <div
        role="status"
        className={cn('flex items-start gap-3 rounded-panel border border-rule bg-ink-raised px-4 py-3', className)}
      >
        <Glyph name="clock" className="mt-0.5 hidden size-4 shrink-0 text-bone-faint sm:block" />
        <div className="min-w-0">
          <p className="text-sm text-bone">{shown.title}</p>
          {shown.lines.map((line) => (
            <p key={line} className="mt-0.5 text-xs leading-relaxed text-bone-dim">
              {line}
            </p>
          ))}
        </div>
      </div>
    )
  }

  return (
    <div
      role="status"
      className={cn(
        'flex flex-col gap-3 rounded-panel border border-brass-deep bg-brass/[0.06] px-4 py-3 sm:flex-row sm:items-center',
        className,
      )}
    >
      <Glyph name="alert" className="hidden size-5 shrink-0 text-brass sm:block" />
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-bone">{shown.title}</p>
        {shown.lines.map((line) => (
          <p key={line} className="mt-0.5 text-xs leading-relaxed text-bone-dim">
            {line}
          </p>
        ))}
        {canWrite && shown.caution ? <p className="mt-1 text-xs leading-relaxed text-brass">{shown.caution}</p> : null}
        {said ? (
          <p role="alert" className="mt-1 text-xs text-brass">
            {said}
          </p>
        ) : null}
        {back.error ? (
          <p role="alert" className="mt-1 flex items-center gap-1.5 text-xs text-status-error">
            <Glyph name="alert" className="size-3.5 shrink-0" />
            {back.error.message || t.common.actionFailed}
          </p>
        ) : null}
      </div>
      {canWrite && shown.back ? (
        <Button size="sm" className="shrink-0" disabled={back.isPending} onClick={() => back.mutate()}>
          {back.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
          {w.back}
        </Button>
      ) : null}
    </div>
  )
}
