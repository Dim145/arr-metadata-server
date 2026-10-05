/**
 * One field, and everything a person may claim of it.
 *
 * The lock is the product, so it is the loudest thing on the row: a brass edge
 * down its side, a padlock beside the field's name and the word itself in a
 * chip. Anyone who cannot tell brass from slate can still read which values a
 * person decided and which a provider did, because none of it is said in colour
 * alone.
 *
 * Saving is what locks. That is the server's model — an override row is written
 * into a table the refresh path never touches — and the interface refuses to
 * invent a friendlier one, because an operator who thinks a field is locked
 * when it is not will lose their edit to the next sweep.
 *
 * A value is shown as what it is, not as what is stored: a date as a date, a
 * genre in its colour, an address as a link, a still as the picture — and the
 * stored form beside it in the register's hand, for whoever has to match it
 * against a release log.
 */

import { useMutation } from '@tanstack/react-query'
import { useState, type ReactNode } from 'react'

import { ExternalLink } from '../../../components/elsewhere'
import { Artwork } from '../../../components/media'
import { OriginChip, originNote } from '../../../components/origins'
import {
  Button,
  FormField,
  Genre,
  Glyph,
  Input,
  Label,
  Provenance,
  Spinner,
  Textarea,
  Toggle,
} from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import * as fmt from '../../../lib/format'
import { useI18n, type Dict } from '../../../lib/i18n'
import { languageName, statusLabel, titleOrigin } from '../../../lib/labels'
import { trailerPage } from '../../../lib/links'
import type { FieldDef, Override, ValueSource } from '../../../lib/types'

import { hostOf } from './shared'

/** The types whose shape is worth saying under the name: what to type. */
const HINTED = new Set(['date', 'dateTime', 'timeOfDay', 'textList', 'integer', 'float', 'boolean', 'ids'])

/** Whether a value counts as given, for the "empty" filter and the origin chip. */
export function hasValue(def: FieldDef, value: unknown): boolean {
  if (def.fieldType === 'boolean') return value === true
  if (value === null || value === undefined || value === '') return false
  return !(Array.isArray(value) && value.length === 0)
}

export function FieldRow({
  itemId,
  scope = 'item',
  def,
  value,
  lock,
  origin,
  traced = false,
  labelled = true,
  onEdit,
  onChanged,
}: {
  itemId: string
  /** `item`, `season:3` or `episode:3x7` — what the override addresses. */
  scope?: string
  def: FieldDef
  value: unknown
  lock?: Override
  /** Who gave the value, when the last merge said. */
  origin?: ValueSource
  /** Whether the work's provenance is known at all: without it, a row says nothing of where its value came from. */
  traced?: boolean
  /** Whether the name is shown: a panel that is one field already says it in its head. */
  labelled?: boolean
  /** Opens something of the caller's in place of the row's own editor: a picture's picker. */
  onEdit?: () => void
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const [editing, setEditing] = useState(false)

  const unlock = useMutation({
    mutationFn: () => api.delete(`/items/${itemId}/overrides/${encodeURIComponent(scope)}/${def.name}`),
    onSuccess: onChanged,
  })

  const locked = lock !== undefined
  const given = hasValue(def, value)
  const note = traced ? originNote(def.name, origin, { hasValue: given, locked }, t, locale) : null
  // A long text, or a row whose panel names it: the value runs the width,
  // under the name's line rather than beside a column the width of a date.
  const wide = def.fieldType === 'longText' || !labelled
  const hint = HINTED.has(def.fieldType) ? ((t.labels.fieldTypes as Record<string, string>)[def.fieldType] ?? def.fieldType) : null

  return (
    <li
      // Names the row after the field it edits: useful when reading the DOM to
      // work out why a lock did not take, and stable for tests to hold on to.
      data-field={def.name}
      className={cn(
        'px-5 py-3 transition-colors duration-150',
        locked ? 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]' : '',
        editing ? '' : 'hover:bg-ink-high',
      )}
    >
      {/* On a phone the buttons sit on the label's line, and the value runs
          the width under both. A long text does the same at every width: a
          paragraph in a column the width of a date is a keyhole. */}
      <div
        className={cn(
          'grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-2',
          wide ? '' : 'sm:grid-cols-[11rem_minmax(0,1fr)_auto] sm:items-start sm:gap-4',
        )}
      >
        <div className={cn('min-w-0 sm:pt-1', !labelled && !locked && 'sr-only', !labelled && locked && !editing && 'col-span-2 sm:col-span-1')}>
          <span className="flex items-center gap-1.5">
            {locked ? <Glyph name="lock" className="size-3.5 shrink-0 text-brass" /> : null}
            <Label className={cn(locked && 'text-brass', !labelled && 'sr-only')}>{fieldLabel(def, t)}</Label>
            {!labelled && locked ? <span className="label text-brass">{t.admin.editor.locked}</span> : null}
          </span>
          {hint && !editing && labelled ? (
            <span className="mt-0.5 block font-mono text-[0.6875rem] text-bone-faint">{hint}</span>
          ) : null}
        </div>

        <div className={cn('col-span-2 min-w-0', wide ? '' : 'sm:col-span-1 sm:flex-1', editing && !wide ? 'sm:col-span-2' : '')}>
          {unlock.isError ? (
            <p role="alert" className="mb-2 text-xs text-vermillion">
              {unlock.error.message || t.common.actionFailed}
            </p>
          ) : null}

          {editing ? (
            <Editor
              itemId={itemId}
              scope={scope}
              def={def}
              value={value}
              onDone={() => {
                setEditing(false)
                onChanged()
              }}
              onCancel={() => setEditing(false)}
            />
          ) : (
            <Shown def={def} value={value} locked={locked} />
          )}

          {/* Under the value on a phone, where the name's column is too
              narrow to share with a chip. */}
          {traced && !locked && !editing ? (
            <div className="mt-2 sm:hidden">
              <OriginChip source={origin} hasValue={given} />
            </div>
          ) : null}
          {note && !editing ? <p className="mt-1.5 text-xs leading-relaxed text-bone-faint">{note}</p> : null}
        </div>

        {editing ? null : (
          <div className="col-start-2 row-start-1 flex shrink-0 flex-wrap items-center justify-end gap-2 sm:col-start-auto sm:row-start-auto">
            {locked ? (
              <>
                <Provenance
                  manual
                  label={lock?.updatedAt ? t.admin.editor.lockedOn(fmt.relative(lock.updatedAt, locale) ?? '') : t.admin.editor.locked}
                />
                <Button
                  size="sm"
                  onClick={() => unlock.mutate()}
                  disabled={unlock.isPending}
                  title={lock?.updatedBy ? t.admin.editor.lockedBy(lock.updatedBy) : undefined}
                >
                  {unlock.isPending ? <Spinner className="size-4" /> : <Glyph name="unlock" className="size-4" />}
                  {t.admin.editor.unlock}
                </Button>
              </>
            ) : (
              <>
                {traced ? (
                  <span className="hidden sm:contents">
                    <OriginChip source={origin} hasValue={given} />
                  </span>
                ) : null}
                <Button size="sm" variant="quiet" onClick={() => (onEdit ? onEdit() : setEditing(true))}>
                  <Glyph name="pencil" className="size-4" />
                  {t.common.edit}
                </Button>
              </>
            )}
          </div>
        )}
      </div>
    </li>
  )
}

/* ── The value, shown ─────────────────────────────────────────────────────── */

function Code({ children }: { children: ReactNode }) {
  return <span className="ml-2 font-mono text-xs text-bone-faint tabular-nums">{children}</span>
}

function Shown({ def, value, locked }: { def: FieldDef; value: unknown; locked: boolean }) {
  const { t, locale } = useI18n()
  const tone = locked ? 'text-bone' : 'text-bone-dim'
  const text = cn('text-sm leading-relaxed break-words', tone)

  if (!hasValue(def, value)) {
    return (
      <p className={cn('text-sm', def.fieldType === 'boolean' ? tone : 'text-bone-faint italic')}>
        {def.fieldType === 'boolean' ? t.common.no : t.common.notSet}
      </p>
    )
  }

  switch (def.name) {
    case 'genres':
      return (
        <ul className="flex flex-wrap gap-1.5">
          {(value as string[]).map((genre) => (
            <li key={genre}>
              <Genre name={genre} />
            </li>
          ))}
        </ul>
      )
    case 'keywords':
      return <Keywords list={value as string[]} />
    case 'status':
      return <p className={text}>{statusLabel(String(value), t)}</p>
    case 'originalLanguage':
      return (
        <p className={text}>
          {languageName(String(value), locale)}
          <Code>{String(value)}</Code>
        </p>
      )
    case 'originalCountry':
    case 'contentRatingCountry': {
      const name = titleOrigin(String(value), locale)
      return (
        <p className={text}>
          {name ?? String(value)}
          {name ? <Code>{String(value)}</Code> : null}
        </p>
      )
    }
    case 'homepage':
      return (
        <p className={text}>
          <ExternalLink href={String(value)}>{hostOf(String(value))}</ExternalLink>
          <Code>{String(value)}</Code>
        </p>
      )
    case 'trailerYoutubeId':
      return (
        <p className={text}>
          <ExternalLink href={trailerPage(String(value))}>YouTube</ExternalLink>
          <Code>{String(value)}</Code>
        </p>
      )
    case 'image':
    case 'primaryPoster':
    case 'primaryFanart':
      if (/^https?:\/\//.test(String(value)) || String(value).startsWith('/')) {
        return (
          <p className="flex items-center gap-3">
            <Artwork
              url={String(value)}
              role={def.name === 'primaryPoster' ? 'thumb' : 'still'}
              alt=""
              className={cn(
                'shrink-0 rounded-card border border-rule object-cover',
                def.name === 'primaryPoster' ? 'h-16 w-11' : 'h-14 w-24',
              )}
            />
            <span className="min-w-0 truncate font-mono text-xs text-bone-dim" title={String(value)}>
              {String(value)}
            </span>
          </p>
        )
      }
      break
    case 'themeMusic':
      return (
        <p className={text}>
          <ExternalLink href={String(value)}>{hostOf(String(value))}</ExternalLink>
          <Code>{String(value)}</Code>
        </p>
      )
    case 'slug':
      return <p className={cn('font-mono text-[0.8125rem] break-all', tone)}>{String(value)}</p>
    case 'runtime':
      return <p className={text}>{fmt.runtime(Number(value), locale) ?? String(value)}</p>
    default:
      break
  }

  switch (def.fieldType) {
    case 'boolean':
      return <p className={text}>{value ? t.common.yes : t.common.no}</p>
    case 'date': {
      const raw = String(value)
      const day = /^\d{4}-\d{2}-\d{2}$/.test(raw) ? fmt.longDate(raw, locale) : fmt.dateTime(raw, locale)
      return (
        <p className={text}>
          {day ?? raw}
          {day ? <Code>{raw}</Code> : null}
        </p>
      )
    }
    case 'dateTime': {
      // The instant as stored, which is what a release log and Sonarr hold,
      // then the same moment on the reader's own clock.
      const raw = String(value)
      const local = fmt.dateTime(raw, locale)
      return (
        <>
          <p className={cn('font-mono text-sm tabular-nums', tone)}>{raw}</p>
          {local ? (
            <p className="mt-0.5 text-xs text-bone-faint">
              {local} {t.episode.yourTime}
            </p>
          ) : null}
        </>
      )
    }
    case 'timeOfDay':
      return <p className={cn('font-mono text-sm tabular-nums', tone)}>{String(value)}</p>
    case 'integer':
    case 'float':
      return <p className={cn('font-mono text-sm tabular-nums', tone)}>{String(value)}</p>
    case 'longText':
      return <p className={cn('max-w-prose text-sm leading-relaxed break-words whitespace-pre-line', tone)}>{String(value)}</p>
    case 'textList':
      return (
        <ul className="flex flex-wrap gap-1.5">
          {(value as string[]).map((entry) => (
            <li key={entry} className="rounded-full border border-rule-bright px-2.5 py-0.5 text-xs text-bone-dim">
              {entry}
            </li>
          ))}
        </ul>
      )
    default:
      return <p className={text}>{readable(value, t)}</p>
  }
}

/** The keywords, the first dozen and then the rest a press away. */
function Keywords({ list }: { list: string[] }) {
  const { t } = useI18n()
  const [all, setAll] = useState(false)
  const shown = all ? list : list.slice(0, 12)

  return (
    <ul className="flex flex-wrap gap-1.5">
      {shown.map((keyword) => (
        <li key={keyword} className="rounded-full border border-rule-bright px-2.5 py-0.5 text-xs text-bone-dim">
          # {keyword}
        </li>
      ))}
      {list.length > shown.length ? (
        <li>
          <button
            type="button"
            onClick={() => setAll(true)}
            className="hit cursor-pointer rounded-full border border-dashed border-rule-bright px-2.5 py-0.5 text-xs text-bone-faint transition-colors duration-150 hover:text-bone"
          >
            +{list.length - shown.length} · {t.common.more}
          </button>
        </li>
      ) : null}
    </ul>
  )
}

/* ── The value, edited ────────────────────────────────────────────────────── */

function Editor({
  itemId,
  scope,
  def,
  value,
  onDone,
  onCancel,
}: {
  itemId: string
  scope: string
  def: FieldDef
  value: unknown
  onDone: () => void
  onCancel: () => void
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState(() => readable(value, t))
  const inputId = `field-${scope.replace(/\W/g, '-')}-${def.name}`

  const save = useMutation({
    mutationFn: (parsed: unknown) => api.put(`/items/${itemId}/overrides`, { scope, field: def.name, value: parsed }),
    onSuccess: onDone,
  })

  const submit = () => save.mutate(parse(draft, def))
  const multiline = def.fieldType === 'longText'
  const hint =
    def.fieldType === 'textList'
      ? t.admin.editor.listHint
      : def.fieldType === 'dateTime'
        ? t.admin.editor.dateTimeHint
        : def.fieldType === 'date'
          ? t.labels.fieldTypes.date
          : def.fieldType === 'ids'
            ? t.labels.fieldTypes.ids
            : undefined

  // A plain day gets the browser's own calendar; a provider's date-time, or
  // anything else, stays as text so nothing is thrown away by the control.
  const dayInput = def.fieldType === 'date' && (draft === '' || /^\d{4}-\d{2}-\d{2}$/.test(draft))

  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault()
        submit()
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          onCancel()
        } else if (multiline && event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
          event.preventDefault()
          submit()
        }
      }}
    >
      {/* The row already shows the field's name beside the box; the form's
          own label is kept for the box's accessible name alone. */}
      <FormField label={<span className="sr-only">{fieldLabel(def, t)}</span>} htmlFor={inputId} hint={hint} error={save.isError ? save.error.message : undefined}>
        {multiline ? (
          <Textarea id={inputId} autoFocus rows={5} value={draft} onChange={(event) => setDraft(event.target.value)} />
        ) : def.fieldType === 'boolean' ? (
          <div className="flex items-center gap-3">
            <Toggle
              id={inputId}
              checked={/^(true|yes|oui|1|on)$/i.test(draft.trim())}
              label={fieldLabel(def, t)}
              onChange={(next) => setDraft(next ? 'yes' : 'no')}
            />
            <span className="text-sm text-bone">{/^(true|yes|oui|1|on)$/i.test(draft.trim()) ? t.common.yes : t.common.no}</span>
          </div>
        ) : (
          <Input
            id={inputId}
            autoFocus
            type={
              dayInput
                ? 'date'
                : def.fieldType === 'timeOfDay'
                  ? 'time'
                  : def.fieldType === 'integer' || def.fieldType === 'float'
                    ? 'number'
                    : def.name === 'homepage'
                      ? 'url'
                      : 'text'
            }
            step={def.fieldType === 'float' ? 'any' : def.fieldType === 'integer' ? 1 : undefined}
            inputMode={def.fieldType === 'integer' ? 'numeric' : def.fieldType === 'float' ? 'decimal' : undefined}
            placeholder={def.fieldType === 'dateTime' ? '2009-03-22T21:00:00Z' : def.fieldType === 'date' && !dayInput ? 'YYYY-MM-DD' : undefined}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            className={def.fieldType === 'integer' || def.fieldType === 'float' || def.fieldType === 'timeOfDay' || dayInput ? 'sm:max-w-xs' : undefined}
          />
        )}
      </FormField>

      {def.fieldType === 'textList' && draft.trim() ? (
        <ul aria-hidden className="flex flex-wrap gap-1.5">
          {unlisted(draft).map((part, index) => (
            <li key={`${part}-${index}`} className="rounded-full border border-rule-bright px-2.5 py-0.5 text-xs text-bone-dim">
              {part}
            </li>
          ))}
        </ul>
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" variant="primary" size="sm" disabled={save.isPending}>
          {save.isPending ? <Spinner className="size-4" /> : <Glyph name="lock" className="size-4" />}
          {t.admin.editor.saveAndLock}
        </Button>
        <Button type="button" size="sm" onClick={onCancel}>
          {t.common.cancel}
        </Button>
        {multiline ? <span className="font-mono text-[0.6875rem] text-bone-faint">Ctrl/⌘ ↵ · Esc</span> : null}
      </div>
    </form>
  )
}

/* ── Values ───────────────────────────────────────────────────────────────── */

/**
 * The entries of a list, as one line a person types into: commas between
 * them, and an entry with a comma of its own — or a quote — in quotes, a quote
 * inside doubled. Split on every comma, a keyword like "Hello, World" came
 * back from the box as two, whether or not it had been touched.
 */
function listed(entries: unknown[]): string {
  return entries
    .map((entry) => {
      const text = String(entry)
      return /[",]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text
    })
    .join(', ')
}

/** The entries of what `listed` wrote, or of what was typed in the same way. */
function unlisted(text: string): string[] {
  const entries: string[] = []
  let entry = ''
  let quoted = false
  for (let at = 0; at < text.length; at += 1) {
    const char = text[at]
    if (quoted) {
      if (char !== '"') entry += char
      else if (text[at + 1] === '"') {
        entry += '"'
        at += 1
      } else quoted = false
    } else if (char === '"' && entry.trim() === '') {
      quoted = true
      entry = ''
    } else if (char === ',') {
      entries.push(entry.trim())
      entry = ''
    } else {
      entry += char
    }
  }
  entries.push(entry.trim())
  return entries.filter(Boolean)
}

/** What a stored value looks like in a box a person types into. */
export function readable(value: unknown, t: Dict): string {
  if (value === null || value === undefined) return ''
  if (Array.isArray(value)) return listed(value)
  if (typeof value === 'boolean') return value ? t.common.yes : t.common.no
  return String(value)
}

/** Turn what was typed into the JSON shape the field expects. */
export function parse(draft: string, def: FieldDef): unknown {
  const trimmed = draft.trim()

  // Clearing the box stores an explicit null, which still counts as an edit and
  // still locks the field: "this work has no network" is a decision too.
  if (trimmed === '') return null

  // The form the server keeps a rating's country in, whatever case it was typed in.
  if (def.name === 'contentRatingCountry') return trimmed.toUpperCase()

  switch (def.fieldType) {
    case 'integer': {
      const parsed = Number.parseInt(trimmed, 10)
      return Number.isNaN(parsed) ? trimmed : parsed
    }
    case 'float': {
      const parsed = Number.parseFloat(trimmed)
      return Number.isNaN(parsed) ? trimmed : parsed
    }
    case 'boolean':
      return /^(true|yes|oui|1|on)$/i.test(trimmed)
    case 'textList':
      return unlisted(trimmed)
    case 'dateTime': {
      // Typed by hand, in UTC: the seconds and the zone filled in, so that
      // "2009-03-22T21:00" is the instant the server expects.
      const partial = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2})(:\d{2})?$/.exec(trimmed)
      return partial ? `${partial[1]}${partial[2] ?? ':00'}Z` : trimmed
    }
    default:
      return trimmed
  }
}

/**
 * A field's name in the reader's language.
 *
 * The registry comes from the server, in English, so a French editor listed
 * "SORT TITLE" and "RUNTIME (MINUTES)". Its own label is kept as the fallback
 * for a field added to the server before it is added here.
 */
export function fieldLabel(def: FieldDef, t: Dict): string {
  return (t.labels.fields as Record<string, string>)[def.name] ?? def.label
}
