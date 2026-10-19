/**
 * While TMDB numbers a series TheTVDB stopped answering for: since when, why,
 * and — for whoever may edit the work — the way back, which clears the
 * fallback and refreshes the series at once. TheTVDB answering, it numbers
 * the series again; still silent, TMDB keeps it, and the banner says so.
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
import * as fmt from '../../../lib/format'
import { useI18n } from '../../../lib/i18n'
import type { Orders } from '../../../lib/types'

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
      queryClient.setQueryData(['orders', workId], answer)
      void queryClient.invalidateQueries({ queryKey: ['work', workId] })
      setSaid(answer.numbering.heldByTmdb ? w.backFailed : w.backDone)
      onChanged?.()
    },
  })

  const numbering = orders?.numbering
  if (!numbering?.heldByTmdb) {
    // Given back: said once, where the banner was.
    return said ? (
      <p role="status" className={cn('flex items-center gap-2 text-sm text-moss', className)}>
        <Glyph name="check" className="size-4 shrink-0" />
        {said}
      </p>
    ) : null
  }

  const since = numbering.fallbackSince ? fmt.longDate(numbering.fallbackSince, locale) : undefined
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
        <p className="text-sm font-medium text-bone">{since ? w.since(since) : w.heldByTmdb}</p>
        <p className="mt-0.5 text-xs leading-relaxed text-bone-dim">{numbering.frozen ? w.whyFrozen : w.why}</p>
        {said ? (
          <p role="alert" className="mt-1 text-xs text-brass">
            {said}
          </p>
        ) : null}
        {back.error ? (
          <p role="alert" className="mt-1 flex items-center gap-1.5 text-xs text-vermillion">
            <Glyph name="alert" className="size-3.5 shrink-0" />
            {back.error.message || t.common.actionFailed}
          </p>
        ) : null}
      </div>
      {canWrite ? (
        <Button size="sm" className="shrink-0" disabled={back.isPending} onClick={() => back.mutate()}>
          {back.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
          {w.back}
        </Button>
      ) : null}
    </div>
  )
}
