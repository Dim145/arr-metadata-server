import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useParams } from 'react-router'

import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import type { FieldDef, FieldRegistry, MediaItem, Override } from '../lib/types'
import { ManualChildren } from '../components/ManualChildren'
import {
  Alert,
  Button,
  Display,
  Input,
  Label,
  Lock,
  Mono,
  Panel,
  PanelHead,
  Select,
  Spinner,
  Tag,
  Textarea,
} from '../components/ui'

/** Languages worth offering without asking the server what it holds. */
const LANGUAGES = [
  { code: '', label: 'as stored' },
  { code: 'en', label: 'English' },
  { code: 'fr', label: 'Français' },
  { code: 'de', label: 'Deutsch' },
  { code: 'es', label: 'Español' },
  { code: 'it', label: 'Italiano' },
  { code: 'pt', label: 'Português' },
  { code: 'nl', label: 'Nederlands' },
  { code: 'ja', label: '日本語' },
  { code: 'ko', label: '한국어' },
  { code: 'zh', label: '中文' },
  { code: 'ru', label: 'Русский' },
]

export function ItemDetail() {
  const { id = '' } = useParams()
  const queryClient = useQueryClient()
  const [language, setLanguage] = useState('')

  const item = useQuery({
    queryKey: ['item', id, language],
    queryFn: () => api.get<MediaItem>(`/items/${id}${query({ language })}`),
  })
  const registry = useQuery({
    queryKey: ['fields'],
    queryFn: () => api.get<FieldRegistry>('/fields'),
    staleTime: Infinity,
  })
  const overrides = useQuery({
    queryKey: ['overrides', id],
    queryFn: () => api.get<Override[]>(`/items/${id}/overrides`),
  })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', id] })
    void queryClient.invalidateQueries({ queryKey: ['overrides', id] })
    void queryClient.invalidateQueries({ queryKey: ['stats'] })
  }

  const refresh = useMutation({
    mutationFn: () => api.post<MediaItem>(`/items/${id}/refresh`),
    onSuccess: invalidate,
  })

  const unlockAll = useMutation({
    mutationFn: () => api.delete<{ removed: number }>(`/items/${id}/overrides`),
    onSuccess: invalidate,
  })

  if (item.isError) {
    return <Alert>This entry could not be loaded.</Alert>
  }
  // Both are needed to render a single row, so wait for the pair.
  if (!item.data || !registry.data) {
    return <Spinner />
  }

  const work = item.data
  const locks = new Map(
    (overrides.data ?? []).filter((o) => o.scope === 'item').map((o) => [o.field, o]),
  )
  const poster = work.images?.find((image) => image.coverType.toLowerCase() === 'poster')

  return (
    <div className="mx-auto max-w-4xl">
      <Link
        to="/catalogue"
        className="reveal font-mono text-[10px] tracking-[0.14em] text-faint uppercase transition-colors hover:text-phos"
      >
        ← Catalogue
      </Link>

      <header className="reveal mt-4 mb-9 flex gap-6" style={{ animationDelay: '40ms' }}>
        {poster && (
          <img
            src={poster.url}
            alt=""
            loading="lazy"
            className="hidden h-[168px] w-[112px] shrink-0 rounded-[2px] border border-line object-cover sm:block"
          />
        )}

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <Tag tone="neutral">{work.kind}</Tag>
            {work.isManual && <Tag tone="manual">manual</Tag>}
            {work.status && <Tag tone="auto">{work.status}</Tag>}
            {locks.size > 0 && (
              <Tag tone="manual">
                <Lock className="h-[11px] w-[11px]" />
                {locks.size} locked
              </Tag>
            )}
          </div>

          <Display className="mt-3">{work.title}</Display>

          {work.overview && (
            <p className="mt-3 max-w-prose text-[13px] leading-relaxed text-dim">
              {work.overview}
            </p>
          )}

          {language !== '' && (
            <p className="mt-2 font-mono text-[10px] tracking-[0.12em] text-faint uppercase">
              showing {language} — edits still apply to the work itself
            </p>
          )}

          <div className="mt-4 flex flex-wrap items-center gap-2">
            <Button onClick={() => refresh.mutate()} disabled={refresh.isPending}>
              {refresh.isPending ? <Spinner /> : 'Refresh from providers'}
            </Button>
            {locks.size > 0 && (
              <Button variant="danger" onClick={() => unlockAll.mutate()} disabled={unlockAll.isPending}>
                Unlock all
              </Button>
            )}
            <a href={`/api/v1/items/${id}/nfo`} target="_blank" rel="noreferrer">
              <Button title="A Kodi/XBMC document — the route to Plex">.nfo</Button>
            </a>

            <Select
              value={language}
              onChange={(event) => setLanguage(event.target.value)}
              title="Show this work in another language, where a translation is held"
              className="py-1.5"
            >
              {LANGUAGES.map((entry) => (
                <option key={entry.code} value={entry.code}>
                  {entry.label}
                </option>
              ))}
            </Select>
          </div>

          {refresh.isError && (
            <div className="mt-3">
              <Alert>{refresh.error.message}</Alert>
            </div>
          )}
          {work.refreshError && (
            <div className="mt-3">
              <Alert tone="neutral">
                Last refresh failed: <span className="text-rust">{work.refreshError}</span>
              </Alert>
            </div>
          )}
        </div>
      </header>

      <Panel className="reveal" style={{ animationDelay: '100ms' }}>
        <PanelHead
          title="Fields"
          aside={
            <span className="font-mono text-[10px] tracking-[0.1em] text-faint uppercase">
              amber = yours
            </span>
          }
        />
        <ul className="divide-y divide-line">
          {registry.data.item.map((def) => (
            <FieldRow
              key={def.name}
              itemId={id}
              def={def}
              value={(work as unknown as Record<string, unknown>)[def.name]}
              lock={locks.get(def.name)}
              onChanged={invalidate}
            />
          ))}
        </ul>
      </Panel>

      <div className="mt-6">
        <ManualChildren item={work} onChanged={invalidate} />
      </div>

      <div className="mt-6 grid gap-6 lg:grid-cols-2">
        <Panel className="reveal" style={{ animationDelay: '140ms' }}>
          <PanelHead title="Identifiers" />
          <dl className="divide-y divide-line">
            {Object.entries(work.externalIds).map(([source, value]) => (
              <div key={source} className="flex items-baseline justify-between gap-4 px-5 py-2.5">
                <Label>{source}</Label>
                <Mono className="text-paper">
                  {Array.isArray(value) ? value.join(', ') : String(value)}
                </Mono>
              </div>
            ))}
            <div className="flex items-baseline justify-between gap-4 px-5 py-2.5">
              <Label>slug</Label>
              <Mono className="text-dim">{work.slug}</Mono>
            </div>
          </dl>
        </Panel>

        <Panel className="reveal" style={{ animationDelay: '180ms' }}>
          <PanelHead title="Provenance" />
          <dl className="divide-y divide-line">
            <Line label="created" value={formatTime(work.createdAt)} />
            <Line label="updated" value={formatTime(work.updatedAt)} />
            <Line label="refreshed" value={formatTime(work.refreshedAt)} />
            <Line label="next refresh" value={formatTime(work.refreshAfter)} />
            {work.seasons && work.seasons.length > 0 && (
              <Line
                label="content"
                value={`${work.seasons.length} seasons · ${work.episodes?.length ?? 0} episodes`}
              />
            )}
          </dl>
        </Panel>
      </div>
    </div>
  )
}

function Line({ label, value }: { label: string; value?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-4 px-5 py-2.5">
      <Label>{label}</Label>
      <Mono className={value ? 'text-dim' : 'text-faint'}>{value ?? '—'}</Mono>
    </div>
  )
}

/**
 * One editable field.
 *
 * The left edge is the whole story: neutral while a provider owns the value,
 * amber once a person does. Saving locks; the padlock unlocks.
 */
function FieldRow({
  itemId,
  def,
  value,
  lock,
  onChanged,
}: {
  itemId: string
  def: FieldDef
  value: unknown
  lock?: Override
  onChanged: () => void
}) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')

  const save = useMutation({
    mutationFn: (parsed: unknown) =>
      api.put(`/items/${itemId}/overrides`, { scope: 'item', field: def.name, value: parsed }),
    onSuccess: () => {
      setEditing(false)
      onChanged()
    },
  })

  const unlock = useMutation({
    mutationFn: () => api.delete(`/items/${itemId}/overrides/item/${def.name}`),
    onSuccess: onChanged,
  })

  const display = formatValue(value)
  const locked = lock !== undefined
  const multiline = def.fieldType === 'longText'

  const begin = () => {
    setDraft(toDraft(value))
    setEditing(true)
  }

  return (
    <li
      className={cn(
        'edge px-5 py-3 transition-colors',
        locked && 'edge-locked bg-phos/[0.035]',
        !editing && 'hover:bg-raised',
      )}
    >
      <div className="flex items-start gap-4">
        <div className="w-44 shrink-0 pt-1">
          <div className="flex items-center gap-1.5">
            {locked && <Lock className="text-phos" />}
            <Label className={locked ? 'text-phos' : undefined}>{def.label}</Label>
          </div>
          <Mono className="mt-0.5 block text-[10px] text-faint">{def.fieldType}</Mono>
        </div>

        <div className="min-w-0 flex-1">
          {editing ? (
            <form
              className="flex flex-col gap-2"
              onSubmit={(event) => {
                event.preventDefault()
                save.mutate(fromDraft(draft, def))
              }}
            >
              {multiline ? (
                <Textarea
                  autoFocus
                  rows={4}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                />
              ) : (
                <Input
                  autoFocus
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  placeholder={def.fieldType === 'textList' ? 'comma, separated, values' : undefined}
                />
              )}

              {save.isError && <Alert>{save.error.message}</Alert>}

              <div className="flex gap-2">
                <Button type="submit" variant="primary" disabled={save.isPending}>
                  {save.isPending ? (
                    <Spinner className="border-void/40 border-t-void" />
                  ) : (
                    <>
                      <Lock className="h-[11px] w-[11px]" /> Save &amp; lock
                    </>
                  )}
                </Button>
                <Button type="button" onClick={() => setEditing(false)}>
                  Cancel
                </Button>
              </div>
            </form>
          ) : (
            <button
              type="button"
              onClick={begin}
              className="w-full text-left"
              title="Edit and lock this field"
            >
              <span
                className={cn(
                  'text-[14px] leading-relaxed break-words',
                  display ? (locked ? 'text-paper' : 'text-dim') : 'text-faint italic',
                )}
              >
                {display || 'not set'}
              </span>
            </button>
          )}
        </div>

        {!editing && (
          <div className="flex shrink-0 items-center gap-1">
            {locked ? (
              <Button
                variant="ghost"
                className="border-phos/40 text-phos hover:border-phos hover:bg-phos hover:text-void"
                onClick={() => unlock.mutate()}
                disabled={unlock.isPending}
                title={`Locked ${lock?.updatedBy ? `by ${lock.updatedBy} ` : ''}— click to unlock`}
              >
                Unlock
              </Button>
            ) : (
              <Button onClick={begin}>Edit</Button>
            )}
          </div>
        )}
      </div>
    </li>
  )
}

/* ── value conversion ─────────────────────────────────────────────────────── */

function formatValue(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (Array.isArray(value)) return value.join(', ')
  if (typeof value === 'boolean') return value ? 'yes' : 'no'
  return String(value)
}

function toDraft(value: unknown): string {
  return formatValue(value)
}

/** Turn the text in the box into the JSON shape the field expects. */
function fromDraft(draft: string, def: FieldDef): unknown {
  const trimmed = draft.trim()

  // Clearing the box stores an explicit null, which still counts as an edit and
  // still locks the field.
  if (trimmed === '') return null

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
      return /^(true|yes|1|on)$/i.test(trimmed)
    case 'textList':
      return trimmed
        .split(',')
        .map((part) => part.trim())
        .filter(Boolean)
    default:
      return trimmed
  }
}

function formatTime(value?: string): string | undefined {
  if (!value) return undefined
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString()
}
