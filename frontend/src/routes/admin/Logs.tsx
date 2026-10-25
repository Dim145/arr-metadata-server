/**
 * What the server wrote.
 *
 * The lines of its standard output, kept in its memory and read here rather
 * than through `docker logs`: narrowed by how grave they are and by words,
 * followed as they come while the page is in view, downloaded as a file. A
 * line written while a request was served names the request and its trace;
 * pressing the trace keeps that request's lines alone — the same id leads to
 * its spans in Tempo, and to its report in Sentry. How grave a line is says a
 * lamp, as a state does everywhere else here: the word carries the meaning,
 * the colour only helps. Every secret in a line was masked by the server.
 */

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  ButtonLink,
  EmptyState,
  Glyph,
  Input,
  Label,
  Lamp,
  Panel,
  Select,
  Skeleton,
  Spinner,
  Toggle,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import { useSettled } from '../../lib/debounce'
import * as fmt from '../../lib/format'
import { useTitle } from '../../lib/hooks'
import { useI18n } from '../../lib/i18n'
import type { LogEntry, LogLevel, LogResponse } from '../../lib/types'

/** How many of the newest lines a page shows. */
const PAGE = 500

/** The levels the filter offers: each shows itself and every graver one. */
const SHOWN = ['error', 'warn', 'info', 'trace'] as const
type Shown = (typeof SHOWN)[number]

export function Logs() {
  const { t, locale } = useI18n()
  useTitle(t.admin.log.title)

  const [level, setLevel] = useState<Shown>('info')
  const [text, setText] = useState('')
  const [follow, setFollow] = useState(true)
  const settled = useSettled(text.trim())

  const log = useQuery({
    queryKey: ['logs', level, settled],
    queryFn: () => api.get<LogResponse>(`/admin/logs${query({ level, q: settled, limit: PAGE })}`),
    // Read again while followed, and never while the page is out of view:
    // react-query holds a hidden tab's interval.
    refetchInterval: follow ? 5_000 : false,
    // The lines read last stay on screen while the next are read, so a
    // filter typed does not blank the page at every key.
    placeholderData: (previous) => previous,
  })

  const filtered = Boolean(settled) || level !== 'trace'
  const count = (n: number) => fmt.count(n, locale)

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.log.title}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
          {t.admin.log.lead(count(log.data?.capacity ?? 2000))}
        </p>
      </header>

      {log.data ? <Standing log={log.data} /> : null}

      <div className="rise mb-6 flex flex-wrap items-end gap-x-4 gap-y-3 border-y border-rule py-4">
        <div className="min-w-44">
          <label htmlFor="log-level" className="label mb-1.5 block">
            {t.admin.log.level}
          </label>
          <Select id="log-level" value={level} onChange={(event) => setLevel(event.target.value as Shown)}>
            {SHOWN.map((value) => (
              <option key={value} value={value}>
                {t.admin.log.levels[value]}
              </option>
            ))}
          </Select>
        </div>

        <div className="w-full min-w-48 sm:w-auto sm:flex-1">
          <label htmlFor="log-find" className="label mb-1.5 block">
            {t.admin.log.find}
          </label>
          <Input
            id="log-find"
            type="search"
            value={text}
            onChange={(event) => setText(event.target.value)}
            aria-describedby="log-find-hint"
            spellCheck={false}
          />
          <p id="log-find-hint" className="mt-1 text-xs text-bone-faint">
            {t.admin.log.findHint}
          </p>
        </div>

        <div className="flex items-center gap-1">
          <Toggle id="log-follow" checked={follow} label={t.admin.log.follow} onChange={setFollow} />
          {/* The switch is named already: this is its word on the page. */}
          <span aria-hidden className="text-sm text-bone" title={t.admin.log.followHint}>
            {t.admin.log.follow}
          </span>
        </div>

        <span className="ml-auto flex flex-wrap items-center gap-3 self-center">
          {/* Every line the filters match, oldest first, as a file. */}
          <span className="flex items-center gap-1">
            {(['text', 'json'] as const).map((format) => (
              <ButtonLink
                key={format}
                size="sm"
                variant="quiet"
                download
                title={t.admin.log.downloadHint}
                href={`/api/v1/admin/logs${query({ level, q: settled, format })}`}
              >
                <Glyph name="download" className="size-3.5" />
                {t.admin.log.download[format]}
              </ButtonLink>
            ))}
          </span>
          {log.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
          {log.data ? (
            <span className="font-mono text-xs text-bone-faint tabular-nums">
              {t.admin.log.shown(log.data.entries.length)}
            </span>
          ) : null}
        </span>
      </div>

      <Panel className="rise overflow-hidden" style={{ animationDelay: '60ms' }}>
        {log.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 10 }, (_, index) => (
              <Skeleton key={index} className="h-10 w-full" />
            ))}
          </div>
        ) : log.isError ? (
          <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {t.admin.log.loadFailed}
          </p>
        ) : log.data.entries.length === 0 ? (
          <EmptyState
            title={t.admin.log.empty}
            hint={filtered ? t.admin.log.emptyFilteredHint : t.admin.log.emptyHint}
          />
        ) : (
          <ol aria-label={t.admin.log.lines} aria-busy={log.isFetching}>
            {log.data.entries.map((entry) => (
              <Line key={entry.seq} entry={entry} onTrace={setText} />
            ))}
          </ol>
        )}
      </Panel>

      {settled ? (
        <div className="mt-4">
          <Button size="sm" onClick={() => setText('')}>
            <Glyph name="close" className="size-3.5" />
            {t.admin.log.clearFilter}
          </Button>
        </div>
      ) : null}
    </div>
  )
}

/**
 * Which instance's log this is, how full it is, and whether the traces and
 * the error reports leave the server.
 */
function Standing({ log }: { log: LogResponse }) {
  const { t, locale } = useI18n()

  return (
    <dl className="rise mb-6 grid gap-x-8 gap-y-3 text-sm sm:grid-cols-3">
      <div>
        <dt className="label mb-1">{t.admin.log.instance}</dt>
        <dd>
          <span className="font-mono text-body-sm break-all text-bone">{log.instance}</span>
          <p className="mt-1 text-xs text-bone-faint">
            {t.admin.log.held(fmt.count(log.held, locale), fmt.count(log.capacity, locale))}.{' '}
            {t.admin.log.severalHint}
          </p>
        </dd>
      </div>
      <div>
        <dt className="label mb-1">{t.admin.log.traces}</dt>
        <dd>
          <Lamp tone={log.traces ? 'moss' : 'faint'}>{log.traces ? t.admin.log.tracesOn : t.admin.log.off}</Lamp>
          {log.traces ? null : <p className="mt-1 text-xs text-bone-faint">{t.admin.log.tracesHint}</p>}
        </dd>
      </div>
      <div>
        <dt className="label mb-1">{t.admin.log.reports}</dt>
        <dd>
          <Lamp tone={log.reports ? 'moss' : 'faint'}>
            {log.reports ? t.admin.log.reportsOn : t.admin.log.off}
          </Lamp>
          {log.reports ? null : (
            <p className="mt-1 text-xs text-bone-faint">
              {t.admin.log.reportsHint}{' '}
              <Link
                to="/admin/settings#settings-errors"
                className="underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-bone"
              >
                {t.admin.log.reportsLink}
              </Link>
              .
            </p>
          )}
        </dd>
      </div>
    </dl>
  )
}

/** One line: when, how grave, what it said, and where it was said from. */
function Line({ entry, onTrace }: { entry: LogEntry; onTrace: (trace: string) => void }) {
  const { t, locale } = useI18n()
  const trace = entry.traceId

  return (
    <li
      className={
        'grid gap-x-4 gap-y-1 border-b border-rule px-4 py-3 last:border-0 sm:grid-cols-[6.5rem_8.5rem_minmax(0,1fr)] ' +
        // A long log is drawn as it scrolls into view, not all at once.
        '[contain-intrinsic-size:auto_4.5rem] [content-visibility:auto]'
      }
    >
      <time dateTime={entry.at} title={fmt.dateTime(entry.at, locale)} className="font-mono text-xs text-bone-faint tabular-nums">
        {timeOf(entry.at, locale)}
      </time>
      <span>
        <Lamp tone={toneOf(entry.level)}>{t.admin.log.levelName[entry.level]}</Lamp>
      </span>
      <div className="min-w-0">
        <p className="text-sm break-words whitespace-pre-wrap text-bone">{entry.message}</p>
        {entry.fields.length ? (
          <p className="mt-1 font-mono text-xs break-words text-bone-dim">
            {entry.fields.map((field, index) => (
              <span key={`${field.name}-${index}`} className="mr-3 inline-block">
                <span className="text-bone-faint">{field.name}=</span>
                {field.value}
              </span>
            ))}
          </p>
        ) : null}
        <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-xs text-bone-faint">
          <span className="break-all">{entry.target}</span>
          {entry.request ? (
            <span className="break-all">
              <span className="sr-only">{t.admin.log.request} </span>
              {entry.request}
            </span>
          ) : null}
          {trace ? (
            <button
              type="button"
              onClick={() => onTrace(trace)}
              aria-label={t.admin.log.onlyThisTrace(trace)}
              title={t.admin.log.onlyThisTrace(trace)}
              className="inline-flex items-center gap-1 rounded-sm text-bone-dim underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-bone focus-visible:outline-2 focus-visible:outline-vermillion"
            >
              {t.admin.log.trace} {trace.slice(0, 12)}…
            </button>
          ) : null}
        </p>
      </div>
    </li>
  )
}

/** How a level reads as a state: an error fails, a warning wants a look, the rest is said. */
export function toneOf(level: LogLevel): 'vermillion' | 'brass' | 'faint' {
  return level === 'error' ? 'vermillion' : level === 'warn' ? 'brass' : 'faint'
}

/** `21:30:05.123`, in the reader's own timezone: a log is read to the millisecond. */
export function timeOf(at: string, locale: string): string {
  const date = new Date(at)
  if (Number.isNaN(date.getTime())) return at
  return clocks(locale).format(date)
}

const CLOCKS = new Map<string, Intl.DateTimeFormat>()

function clocks(locale: string): Intl.DateTimeFormat {
  let clock = CLOCKS.get(locale)
  if (!clock) {
    clock = new Intl.DateTimeFormat(locale, {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      fractionalSecondDigits: 3,
      hour12: false,
    })
    CLOCKS.set(locale, clock)
  }
  return clock
}
