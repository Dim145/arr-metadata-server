/**
 * What the catalogue writes for the programs around it: a Kometa metadata
 * file of what has been locked, for a Plex library, and the `.nfo` documents
 * of one selection.
 *
 * Both are the administrator's to ask for, and each says what came of it —
 * a file saved, nothing to write, the reason it could not be — rather than
 * leaving a button that seems to have done nothing.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import { ApiError, api, query } from '../lib/api'
import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import type { ExportStarted } from '../lib/types'
import { Button, Glyph, Spinner } from './ui'

type Kind = 'movie' | 'series'

/** What asking for the file came to. */
interface Outcome {
  tone: 'fine' | 'note' | 'bad'
  text: string
}

/**
 * The file, read whole, and how many works it speaks of — which the server
 * says beside it, so that a file with nothing in it is not handed over.
 *
 * Not `api.get`: the answer is YAML, and that reads a body as JSON.
 */
async function kometaFile(kind: Kind): Promise<{ works: number; text: string }> {
  const response = await fetch(`/api/v1/export/kometa.yml${query({ kind })}`, { credentials: 'same-origin' })
  const text = await response.text()

  if (!response.ok) {
    // The server's own words where it said some; a proxy's page is not them.
    let said = response.statusText
    try {
      const detail = JSON.parse(text) as { message?: string }
      if (detail.message) said = detail.message
    } catch {
      // Not JSON.
    }
    throw new ApiError(response.status, 'kometa', said)
  }

  return { works: Number(response.headers.get('x-ams-works') ?? 0), text }
}

/** A file the browser keeps, under the name it is given. */
function save(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'application/yaml' }))
  const link = window.document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

/**
 * The Kometa metadata files — one for the films, one for the series, a
 * library holding one kind — each downloaded when it has something to say.
 */
export function KometaExport() {
  const { t } = useI18n()
  const e = t.admin.exports
  const [busy, setBusy] = useState<Kind | null>(null)
  const [outcome, setOutcome] = useState<Outcome>()

  const download = async (kind: Kind) => {
    setBusy(kind)
    setOutcome(undefined)
    try {
      const file = await kometaFile(kind)
      if (file.works === 0) {
        setOutcome({ tone: 'note', text: e.kometaEmpty(kind) })
      } else {
        save(kind === 'movie' ? 'kometa-movies.yml' : 'kometa-series.yml', file.text)
        setOutcome({ tone: 'fine', text: e.kometaDone(kind, file.works) })
      }
    } catch (error) {
      setOutcome({ tone: 'bad', text: error instanceof ApiError && error.message ? error.message : e.kometaFailed })
    } finally {
      setBusy(null)
    }
  }

  const kinds: { kind: Kind; label: string }[] = [
    { kind: 'movie', label: e.kometaMovies },
    { kind: 'series', label: e.kometaSeries },
  ]

  return (
    <div id="kometa-export" className="border-t border-rule p-5">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="min-w-0">
          <p className="text-sm text-bone">{e.kometaTitle}</p>
          <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">{e.kometaBody}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          {kinds.map(({ kind, label }) => (
            <Button
              key={kind}
              onClick={() => void download(kind)}
              disabled={busy !== null}
              aria-label={e.kometaFile(label.toLowerCase())}
            >
              {busy === kind ? <Spinner className="size-4" /> : <Glyph name="download" className="size-4" />}
              {label}
            </Button>
          ))}
        </div>
      </div>
      {outcome ? (
        <p
          role={outcome.tone === 'bad' ? 'alert' : 'status'}
          className={cn(
            'mt-3 text-xs',
            outcome.tone === 'bad' ? 'text-vermillion' : outcome.tone === 'fine' ? 'text-moss' : 'text-bone-dim',
          )}
        >
          {outcome.text}
        </p>
      ) : null}
    </div>
  )
}

/**
 * The `.nfo` documents of one selection's works, written in the background
 * under the export folder as the whole export is: asked for, and followed in
 * Jobs, where the run says what it wrote.
 */
export function SelectionNfo({ listId }: { listId: string }) {
  const { t } = useI18n()
  const e = t.admin.exports
  const queryClient = useQueryClient()

  const run = useMutation({
    mutationFn: () => api.post<ExportStarted>(`/export/nfo${query({ list: listId })}`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['jobs'] })
    },
  })

  // The code alone: on this route it has exactly one cause, no export folder.
  const missingPath =
    run.error instanceof ApiError &&
    (run.error.code === 'export_not_configured' || run.error.code === 'provider_not_configured')

  return (
    <div className="flex flex-wrap items-center justify-between gap-4">
      <div className="min-w-0">
        <p className="text-sm text-bone">{e.nfoTitle}</p>
        <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">{e.nfoBody}</p>
        {run.isSuccess ? (
          <p role="status" className="mt-2 max-w-prose text-xs leading-relaxed text-bone-dim">
            {e.nfoStarted}{' '}
            <Link to="/admin/jobs" className="text-vermillion underline-offset-2 hover:underline">
              {e.nfoJobs}
            </Link>
          </p>
        ) : null}
        {run.isError ? (
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {missingPath ? e.nfoNoPath : run.error.message}
          </p>
        ) : null}
      </div>
      <Button onClick={() => run.mutate()} disabled={run.isPending}>
        {run.isPending ? <Spinner className="size-4" /> : <Glyph name="download" className="size-4" />}
        {e.nfoRun}
      </Button>
    </div>
  )
}
