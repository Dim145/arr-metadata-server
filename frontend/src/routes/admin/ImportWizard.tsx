/**
 * Many works imported at once: ids pasted, a file of them dropped, or works
 * picked from the search — set beside the catalogue before anything is
 * imported, then imported one after the other as a run the tasks page
 * follows.
 *
 * Four steps, each a screen of its own: the source; the preview, where each
 * line says whether its work is already held, to import, or not found; the
 * options — a language the episodes are fetched in too, the selections the
 * works go into, whether they are switched on; and the run, followed here as
 * it goes, and on the tasks page.
 */

import { useMutation, useQuery } from '@tanstack/react-query'
import { useEffect, useId, useRef, useState, type DragEvent } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  FormField,
  Glyph,
  Lamp,
  Panel,
  PanelHead,
  Segmented,
  Select,
  Spinner,
  Textarea,
  Toggle,
} from '../../components/ui'
import { ApiError, api } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { MOST_LINES, linesOfFile, linesOfText, worthImporting } from '../../lib/importFile'
import { languageName } from '../../lib/labels'
import { TEXT_LANGUAGES } from '../../lib/prefs'
import type {
  BatchStarted,
  CuratedLists,
  ImportLine,
  ImportPreview,
  ImportPreviewLine,
  JobDetail,
  MediaKind,
} from '../../lib/types'
import { JobDetailPanel, RunStatus, describeDetail } from './Jobs'

type Step = 'source' | 'preview' | 'options' | 'follow'
const STEPS: Step[] = ['source', 'preview', 'options', 'follow']

/** Which lines the preview shows. */
type Shown = 'all' | 'candidate' | 'held' | 'trouble'

/** The most lines the preview draws at once: a batch of two thousand reads as its counts. */
const DRAWN = 500

export function ImportWizard({ picked, onClearPicked }: { picked: ImportLine[]; onClearPicked: () => void }) {
  const { t } = useI18n()
  const w = t.admin.wizard
  const [step, setStep] = useState<Step>('source')
  const [text, setText] = useState('')
  const [file, setFile] = useState<{ name: string; lines: ImportLine[] } | null>(null)
  const [kind, setKind] = useState<'' | MediaKind>('')
  const [preview, setPreview] = useState<ImportPreview | null>(null)
  const [language, setLanguage] = useState('')
  const [lists, setLists] = useState<string[]>([])
  const [enabled, setEnabled] = useState(true)
  const [started, setStarted] = useState<BatchStarted | null>(null)

  const lines = [...linesOfText(text), ...(file?.lines ?? []), ...picked]
  const tooMany = lines.length > MOST_LINES

  // Each step's heading takes the focus as it opens, so a keyboard and a
  // screen reader start where the step does.
  const heading = useRef<HTMLHeadingElement>(null)
  const first = useRef(true)
  useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    heading.current?.focus()
  }, [step])

  const check = useMutation({
    mutationFn: () => api.post<ImportPreview>('/items/import/preview', { lines, kind: kind || undefined }),
    onSuccess: (answer) => {
      setPreview(answer)
      setStep('preview')
    },
  })

  const start = useMutation({
    mutationFn: () =>
      api.post<BatchStarted>('/items/import/batch', {
        // What the preview found no provider has is not asked for again.
        lines: worthImporting(lines, preview),
        kind: kind || undefined,
        language: language || undefined,
        lists,
        enabled,
      }),
    onSuccess: (answer) => {
      setStarted(answer)
      setStep('follow')
    },
  })

  const again = () => {
    setText('')
    setFile(null)
    setPreview(null)
    setStarted(null)
    onClearPicked()
    check.reset()
    start.reset()
    setStep('source')
  }

  const at = STEPS.indexOf(step)

  return (
    <Panel label={w.title} className="rise" style={{ animationDelay: '60ms' }}>
      <PanelHead title={w.title} />
      <div className="border-b border-rule px-5 py-4">
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{w.lead}</p>
        <ol className="mt-4 flex flex-wrap gap-x-5 gap-y-2" aria-label={w.stepsLabel}>
          {STEPS.map((one, index) => (
            <li
              key={one}
              aria-current={one === step ? 'step' : undefined}
              className={cn(
                'flex items-center gap-2 text-sm',
                one === step ? 'text-bone' : index < at ? 'text-bone-dim' : 'text-bone-faint',
              )}
            >
              <span
                aria-hidden
                className={cn(
                  'grid size-6 place-items-center rounded-full border font-mono text-xs tabular-nums',
                  one === step ? 'border-vermillion text-vermillion' : index < at ? 'border-status-ok text-status-ok' : 'border-rule',
                )}
              >
                {index < at ? <Glyph name="check" className="size-3" /> : index + 1}
              </span>
              {w.steps[one]}
            </li>
          ))}
        </ol>
      </div>

      <div className="px-5 py-5">
        <h3 ref={heading} tabIndex={-1} className="font-display text-lg text-bone outline-none">
          {w.stepOf(at + 1, STEPS.length, w.steps[step] ?? step)}
        </h3>

        {step === 'source' ? (
          <Source
            text={text}
            setText={setText}
            file={file}
            setFile={setFile}
            kind={kind}
            setKind={setKind}
            picked={picked.length}
            onClearPicked={onClearPicked}
            count={lines.length}
            tooMany={tooMany}
            checking={check.isPending}
            error={check.isError ? (check.error instanceof ApiError ? check.error.message : t.common.error) : undefined}
            onCheck={() => check.mutate()}
          />
        ) : null}

        {step === 'preview' && preview ? (
          <PreviewStep preview={preview} onBack={() => setStep('source')} onNext={() => setStep('options')} />
        ) : null}

        {step === 'options' && preview ? (
          <Options
            preview={preview}
            language={language}
            setLanguage={setLanguage}
            lists={lists}
            setLists={setLists}
            enabled={enabled}
            setEnabled={setEnabled}
            starting={start.isPending}
            error={start.isError ? (start.error instanceof ApiError ? start.error.message : t.common.error) : undefined}
            onBack={() => setStep('preview')}
            onStart={() => start.mutate()}
          />
        ) : null}

        {step === 'follow' && started ? <Follow started={started} onAgain={again} /> : null}
      </div>
    </Panel>
  )
}

/* ── The source ───────────────────────────────────────────────────────────── */

function Source({
  text,
  setText,
  file,
  setFile,
  kind,
  setKind,
  picked,
  onClearPicked,
  count,
  tooMany,
  checking,
  error,
  onCheck,
}: {
  text: string
  setText: (next: string) => void
  file: { name: string; lines: ImportLine[] } | null
  setFile: (next: { name: string; lines: ImportLine[] } | null) => void
  kind: '' | MediaKind
  setKind: (next: '' | MediaKind) => void
  picked: number
  onClearPicked: () => void
  count: number
  tooMany: boolean
  checking: boolean
  error: string | undefined
  onCheck: () => void
}) {
  const { t, locale } = useI18n()
  const w = t.admin.wizard
  const ids = useId()
  const [over, setOver] = useState(false)
  const [unread, setUnread] = useState<string | null>(null)

  const read = async (chosen: File | undefined) => {
    if (!chosen) return
    const lines = linesOfFile(chosen.name, await chosen.text())
    if (lines.length === 0) {
      setUnread(chosen.name)
      setFile(null)
    } else {
      setUnread(null)
      setFile({ name: chosen.name, lines })
    }
  }

  const dropped = (event: DragEvent<HTMLLabelElement>) => {
    event.preventDefault()
    setOver(false)
    void read(event.dataTransfer.files[0])
  }

  return (
    <div className="mt-4 space-y-5">
      <FormField label={w.paste} htmlFor={`${ids}-text`} hint={w.pasteHint}>
        <Textarea
          id={`${ids}-text`}
          rows={8}
          spellCheck={false}
          className="font-mono text-xs"
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
      </FormField>

      <div className="space-y-1.5">
        <p className="label">{w.drop}</p>
        <label
          onDragOver={(event) => {
            event.preventDefault()
            setOver(true)
          }}
          onDragLeave={() => setOver(false)}
          onDrop={dropped}
          className={cn(
            'flex min-h-24 cursor-pointer flex-col items-center justify-center gap-2 rounded-card border border-dashed px-4 py-5 text-center text-sm',
            'transition-colors duration-150 focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-vermillion',
            over ? 'border-vermillion bg-vermillion/5 text-bone' : 'border-rule-bright text-bone-dim hover:border-bone-faint',
          )}
        >
          <Glyph name="download" className="size-5 text-bone-faint" />
          <span>{w.dropHere}</span>
          <input
            type="file"
            accept=".csv,.tsv,.json,.txt,text/csv,application/json,text/plain"
            className="sr-only"
            onChange={(event) => void read(event.target.files?.[0])}
          />
        </label>
        <p className="text-xs text-bone-faint">{w.dropHint}</p>
        <div role="status" className="text-sm">
          {file ? (
            <p className="flex flex-wrap items-center gap-2 text-status-ok">
              <Glyph name="check" className="size-4" />
              {w.read(file.lines.length, file.name)}
              <Button size="sm" variant="quiet" onClick={() => setFile(null)}>
                {w.forget}
              </Button>
            </p>
          ) : unread ? (
            <p className="text-vermillion">{w.unreadFile(unread)}</p>
          ) : null}
        </div>
      </div>

      {picked > 0 ? (
        <p className="flex flex-wrap items-center gap-2 text-sm text-bone">
          <Glyph name="search" className="size-4 text-bone-faint" />
          {w.fromSearch(picked)}
          <Button size="sm" variant="quiet" onClick={onClearPicked}>
            {w.clearPicked}
          </Button>
        </p>
      ) : null}

      <div className="max-w-sm">
        <FormField label={w.tmdbKind} htmlFor={`${ids}-kind`} hint={w.tmdbKindHint}>
          <Select id={`${ids}-kind`} value={kind} onChange={(event) => setKind(event.target.value as '' | MediaKind)}>
            <option value="">{w.unsure}</option>
            <option value="movie">{t.nav.films}</option>
            <option value="series">{t.nav.series}</option>
          </Select>
        </FormField>
      </div>

      <div className="flex flex-wrap items-center gap-3 border-t border-rule pt-4">
        <Button variant="primary" disabled={count === 0 || tooMany || checking} onClick={onCheck}>
          {checking ? <Spinner className="size-4" /> : <Glyph name="search" className="size-4" />}
          {checking ? w.checking : w.check}
        </Button>
        <span className="font-mono text-xs text-bone-faint tabular-nums">{w.lines(count, fmt.count(count, locale))}</span>
        {checking ? <p className="max-w-prose text-xs leading-relaxed text-bone-faint">{w.checkingHint}</p> : null}
        {tooMany ? (
          <p role="alert" className="text-sm text-status-error">
            {w.tooMany(fmt.count(MOST_LINES, locale))}
          </p>
        ) : null}
        {error ? (
          <p role="alert" className="text-sm text-status-error">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  )
}

/* ── The preview ──────────────────────────────────────────────────────────── */

const TROUBLE: ImportPreviewLine['status'][] = ['missing', 'unreadable', 'needs_kind', 'repeated']

function PreviewStep({ preview, onBack, onNext }: { preview: ImportPreview; onBack: () => void; onNext: () => void }) {
  const { t, locale } = useI18n()
  const w = t.admin.wizard
  const [shown, setShown] = useState<Shown>('all')
  const { counts } = preview
  const wanted = (line: ImportPreviewLine) =>
    shown === 'all' ||
    (shown === 'candidate' && line.status === 'candidate') ||
    (shown === 'held' && line.status === 'held') ||
    (shown === 'trouble' && TROUBLE.includes(line.status))
  const lines = preview.lines.filter(wanted)
  const unchecked = preview.lines.filter((line) => line.status === 'candidate' && !line.checked).length

  return (
    <div className="mt-4 space-y-4">
      <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-card border border-rule bg-rule sm:grid-cols-3">
        {(['candidate', 'held', 'missing', 'unreadable', 'needs_kind', 'repeated'] as const).map((status) => (
          <div key={status} className="bg-ink-raised px-4 py-3">
            <dt className="label">{w.counts[status]}</dt>
            <dd
              className={cn(
                'mt-1 font-display text-2xl tabular-nums',
                status === 'candidate' ? 'text-bone' : status === 'held' ? 'text-status-ok' : counts[camel(status)] ? 'text-status-error' : 'text-bone-faint',
              )}
            >
              {fmt.count(counts[camel(status)], locale)}
            </dd>
          </div>
        ))}
      </dl>
      {unchecked > 0 ? <p className="text-xs text-bone-faint">{w.unchecked(unchecked)}</p> : null}

      <Segmented<Shown>
        label={w.show}
        value={shown}
        onChange={setShown}
        options={[
          { value: 'all', label: w.showAll },
          { value: 'candidate', label: w.counts.candidate },
          { value: 'held', label: w.counts.held },
          { value: 'trouble', label: w.showTrouble },
        ]}
      />

      <ol className="max-h-[28rem] divide-y divide-rule overflow-y-auto rounded-card border border-rule" aria-label={w.linesLabel}>
        {lines.slice(0, DRAWN).map((line) => (
          <PreviewRow key={line.line} line={line} />
        ))}
        {lines.length === 0 ? <li className="px-4 py-6 text-sm text-bone-faint">{w.noneShown}</li> : null}
      </ol>
      {lines.length > DRAWN ? <p className="text-xs text-bone-faint">{w.moreLines(lines.length - DRAWN)}</p> : null}

      <div className="flex flex-wrap items-center gap-3 border-t border-rule pt-4">
        <Button onClick={onBack}>
          <Glyph name="chevronLeft" className="size-4" />
          {w.back}
        </Button>
        <Button variant="primary" disabled={counts.candidate + counts.held === 0} onClick={onNext}>
          {w.next}
          <Glyph name="chevronRight" className="size-4" />
        </Button>
        {counts.candidate + counts.held === 0 ? <p className="text-sm text-bone-faint">{w.nothingToImport}</p> : null}
      </div>
    </div>
  )
}

/** A status as the counts are keyed. */
function camel(status: ImportPreviewLine['status']): keyof ImportPreview['counts'] {
  return status === 'needs_kind' ? 'needsKind' : status
}

function PreviewRow({ line }: { line: ImportPreviewLine }) {
  const { t } = useI18n()
  const w = t.admin.wizard
  const tone = line.status === 'candidate' ? 'provider' : line.status === 'held' ? 'neutral' : 'accent'

  return (
    <li className="grid grid-cols-[3rem_minmax(0,1fr)] gap-x-3 gap-y-1 px-4 py-2.5 sm:grid-cols-[3rem_minmax(0,14rem)_minmax(0,1fr)_auto]">
      <span className="font-mono text-label text-bone-faint tabular-nums">{line.line + 1}</span>
      <span className="truncate font-mono text-xs text-bone-dim" title={line.text}>
        {line.text}
      </span>
      <span className="col-start-2 min-w-0 text-sm sm:col-start-auto">
        {line.work ? (
          <Link
            to={`/admin/catalogue/${line.work.id}`}
            className="truncate text-bone underline decoration-rule-bright underline-offset-2 hover:text-vermillion"
          >
            {line.work.title}
          </Link>
        ) : line.title ? (
          <span className="text-bone">
            {line.title}
            {line.year ? <span className="ml-1.5 font-mono text-xs text-bone-faint">{line.year}</span> : null}
          </span>
        ) : null}
        {line.status === 'candidate' && !line.checked ? (
          <span className="block text-xs text-bone-faint">{w.notChecked}</span>
        ) : null}
      </span>
      <span className="col-start-2 sm:col-start-auto">
        <Chip tone={tone}>{w.counts[line.status === 'needs_kind' ? 'needs_kind' : line.status]}</Chip>
      </span>
    </li>
  )
}

/* ── The options ──────────────────────────────────────────────────────────── */

function Options({
  preview,
  language,
  setLanguage,
  lists,
  setLists,
  enabled,
  setEnabled,
  starting,
  error,
  onBack,
  onStart,
}: {
  preview: ImportPreview
  language: string
  setLanguage: (next: string) => void
  lists: string[]
  setLists: (next: string[]) => void
  enabled: boolean
  setEnabled: (next: boolean) => void
  starting: boolean
  error: string | undefined
  onBack: () => void
  onStart: () => void
}) {
  const { t, locale } = useI18n()
  const w = t.admin.wizard
  const ids = useId()
  const selections = useQuery({
    queryKey: ['admin-lists'],
    queryFn: () => api.get<CuratedLists>('/lists'),
  })
  const manual = (selections.data?.lists ?? []).filter((list) => list.mode === 'manual')

  return (
    <div className="mt-4 space-y-6">
      <div className="max-w-sm">
        <FormField label={w.language} htmlFor={`${ids}-language`} hint={w.languageHint}>
          <Select id={`${ids}-language`} value={language} onChange={(event) => setLanguage(event.target.value)}>
            <option value="">{w.catalogueOnly}</option>
            {TEXT_LANGUAGES.map((code) => (
              <option key={code} value={code}>
                {languageName(code, locale) ?? code}
              </option>
            ))}
          </Select>
        </FormField>
      </div>

      <fieldset className="space-y-2">
        <legend className="label">{w.lists}</legend>
        <p className="text-xs text-bone-faint">{w.listsHint}</p>
        {manual.length === 0 ? (
          <p className="text-sm text-bone-faint">{w.noLists}</p>
        ) : (
          <ul className="grid gap-1 sm:grid-cols-2">
            {manual.map((list) => (
              <li key={list.id}>
                <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-card px-2 text-sm text-bone hover:bg-ink-high">
                  <input
                    type="checkbox"
                    className="size-4 accent-vermillion"
                    checked={lists.includes(list.id)}
                    onChange={(event) =>
                      setLists(event.target.checked ? [...lists, list.id] : lists.filter((id) => id !== list.id))
                    }
                  />
                  {list.name}
                  <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.count(list.itemCount, locale)}</span>
                </label>
              </li>
            ))}
          </ul>
        )}
      </fieldset>

      <div className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <label htmlFor={`${ids}-enabled`} className="block text-sm text-bone">
            {w.enabled}
          </label>
          <p className="mt-0.5 max-w-prose text-xs leading-relaxed text-bone-faint">{w.enabledHint}</p>
        </div>
        <Toggle id={`${ids}-enabled`} checked={enabled} label={w.enabled} onChange={setEnabled} />
      </div>

      <div className="flex flex-wrap items-center gap-3 border-t border-rule pt-4">
        <Button onClick={onBack}>
          <Glyph name="chevronLeft" className="size-4" />
          {w.back}
        </Button>
        <Button variant="primary" disabled={starting} onClick={onStart}>
          {starting ? <Spinner className="size-4" /> : <Glyph name="download" className="size-4" />}
          {starting ? w.starting : w.start}
        </Button>
        <p className="text-sm text-bone-dim">{w.summary(preview.counts.candidate, preview.counts.held)}</p>
        {error ? (
          <p role="alert" className="w-full text-sm text-status-error">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  )
}

/* ── The run ──────────────────────────────────────────────────────────────── */

function Follow({ started, onAgain }: { started: BatchStarted; onAgain: () => void }) {
  const { t, locale } = useI18n()
  const w = t.admin.wizard
  const run = useQuery({
    queryKey: ['job', started.jobId],
    queryFn: () => api.get<JobDetail>(`/jobs/${encodeURIComponent(started.jobId)}`),
    refetchInterval: (query) => (query.state.data?.job.status === 'running' || !query.state.data ? 2_000 : false),
  })
  const job = run.data?.job
  const progress = job?.detail ? /^(\d+) of (\d+):/.exec(job.detail) : null
  const done = progress ? Number(progress[1]) : undefined
  const total = progress ? Number(progress[2]) : undefined

  return (
    <div className="mt-4 space-y-5">
      <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{w.started(started.works, started.passedOver)}</p>
      <div role="status" className="space-y-2">
        {job ? (
          <>
            <div className="flex flex-wrap items-center gap-3">
              {job.status === 'running' ? <Lamp tone="brass">{t.admin.runs.running}</Lamp> : <RunStatus status={job.status} />}
              <span className="font-mono text-xs text-bone-dim tabular-nums">
                {describeDetail(job.error ?? job.detail, t, locale)}
              </span>
            </div>
            {job.status === 'running' ? (
              <progress
                aria-label={w.progress}
                value={done}
                max={total}
                className={cn(
                  'block h-1.5 w-full appearance-none overflow-hidden rounded-full bg-ink-high',
                  '[&::-webkit-progress-bar]:bg-ink-high [&::-webkit-progress-value]:bg-brass [&::-moz-progress-bar]:bg-brass',
                  '[&:indeterminate]:animate-pulse [&:indeterminate]:bg-brass/40 [&:indeterminate::-webkit-progress-bar]:bg-brass/40 motion-reduce:[&:indeterminate]:animate-none',
                )}
              />
            ) : null}
          </>
        ) : (
          <Spinner className="size-4" />
        )}
      </div>

      {job ? <JobDetailPanel job={job} /> : null}

      <div className="flex flex-wrap items-center gap-4 border-t border-rule pt-4">
        <Link
          to="/admin/jobs#task-import.batch"
          className="inline-flex min-h-11 items-center gap-1.5 text-sm text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
        >
          {w.followInTasks}
          <Glyph name="chevronRight" className="size-3.5" />
        </Link>
        {job && job.status !== 'running' ? (
          <Button onClick={onAgain}>
            <Glyph name="refresh" className="size-4" />
            {w.another}
          </Button>
        ) : null}
      </div>
    </div>
  )
}
