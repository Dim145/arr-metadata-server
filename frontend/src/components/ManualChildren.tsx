/**
 * The children a person added, alongside the providers'.
 *
 * A manual row survives every refresh; a provider's row cannot be removed here
 * at all, because it would simply come back and read as the delete having
 * failed. So the remove control exists only where it means something, and every
 * row that carries one says whose it is in a word as well as a colour.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useRef, useState, type ReactNode } from 'react'

import { api } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import type { MediaItem, Uploaded, WorkMedia, WorkMedium } from '../lib/types'
import { seasonName } from '../lib/media'
import { Artwork as Picture } from './media'
import {
  Button,
  Chip,
  Dialog,
  FormField,
  Glyph,
  IconButton,
  Input,
  Panel,
  PanelHead,
  Provenance,
  Select,
  Spinner,
} from './ui'

/** A row an operator has asked to remove: the route to call, and its name. */
type Target = { path: string; label: string }

export function ManualChildren({ item, onChanged }: { item: MediaItem; onChanged: () => void }) {
  const { t } = useI18n()
  const queryClient = useQueryClient()
  const [removing, setRemoving] = useState<Target | null>(null)

  const done = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', item.id] })
    onChanged()
  }

  const remove = useMutation({
    mutationFn: (path: string) => api.delete(`/items/${item.id}/${path}`),
    onSuccess: () => {
      setRemoving(null)
      done()
    },
  })

  const ask = (target: Target) => setRemoving(target)

  return (
    <>
      <div className="grid gap-6 lg:grid-cols-2">
        <Credits item={item} onDone={done} onRemove={ask} />
        <AlternativeTitles item={item} onDone={done} onRemove={ask} />
        <Artwork item={item} onDone={done} onRemove={ask} />
        {item.kind === 'series' ? <Seasons item={item} onDone={done} onRemove={ask} /> : null}
        {item.kind === 'series' ? <Episodes item={item} onDone={done} onRemove={ask} /> : null}
      </div>

      <Dialog
        open={removing !== null}
        title={t.admin.editor.children.removeTitle}
        onClose={() => setRemoving(null)}
        footer={
          <>
            <Button onClick={() => setRemoving(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={remove.isPending}
              onClick={() => removing && remove.mutate(removing.path)}
            >
              <Glyph name="trash" className="size-4" />
              {t.common.delete}
            </Button>
          </>
        }
      >
        <p className="mb-2 font-mono text-[0.8125rem] text-bone">{removing?.label}</p>
        {t.admin.editor.children.removeBody}
      </Dialog>
    </>
  )
}

/* ── Shared pieces ────────────────────────────────────────────────────────── */

type PanelProps = {
  item: MediaItem
  onDone: () => void
  onRemove: (target: Target) => void
}

function useAdd(itemId: string, path: string, onDone: () => void) {
  return useMutation({
    mutationFn: (body: unknown) => api.post<{ id: string }>(`/items/${itemId}/${path}`, body),
    onSuccess: onDone,
  })
}

/**
 * One child, and whose it is.
 *
 * The brass edge is the same mark the field editor uses for a locked value, so
 * "a person put this here" looks the same wherever it appears — and it is never
 * the only signal: the chip beside it says the word.
 */
function ChildRow({
  children,
  manual,
  onRemove,
  extra,
}: {
  children: ReactNode
  manual: boolean
  onRemove?: () => void
  /** Beside the row's own controls: what is kept of it, and the way to forget it. */
  extra?: ReactNode
}) {
  const { t } = useI18n()

  return (
    <li
      className={cn(
        'flex items-center gap-2 py-1.5 pr-2 pl-5 transition-colors duration-150 hover:bg-ink-high',
        manual ? 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]' : '',
      )}
    >
      <div className="min-w-0 flex-1">{children}</div>
      {extra}
      {manual && onRemove ? (
        <>
          <Provenance manual label={t.admin.editor.children.yours} />
          <IconButton glyph="trash" tone="danger" label={t.common.delete} onClick={onRemove} />
        </>
      ) : null}
    </li>
  )
}

/** A form that stays folded until it is asked for, so the panels stay legible. */
function AddForm({
  label,
  onSubmit,
  pending,
  error,
  children,
}: {
  label: string
  onSubmit: () => void
  pending: boolean
  error?: Error | null
  children: ReactNode
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)

  if (!open) {
    return (
      <div className="border-t border-rule px-5 py-3">
        <Button size="sm" onClick={() => setOpen(true)}>
          <Glyph name="plus" className="size-4" />
          {label}
        </Button>
      </div>
    )
  }

  return (
    <form
      className="flex flex-col gap-4 border-t border-rule px-5 py-4"
      onSubmit={(event) => {
        event.preventDefault()
        onSubmit()
      }}
    >
      {children}

      {error ? (
        <p role="alert" className="flex items-center gap-1.5 text-xs text-vermillion">
          <Glyph name="alert" className="size-3.5 shrink-0" />
          {error.message}
        </p>
      ) : null}

      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="primary" size="sm" disabled={pending}>
          {pending ? <Spinner className="size-4" /> : null}
          {t.common.add}
        </Button>
        <Button type="button" size="sm" onClick={() => setOpen(false)}>
          {t.common.cancel}
        </Button>
      </div>
    </form>
  )
}

function Count({ children }: { children: ReactNode }) {
  return <span className="font-mono text-xs text-bone-faint tabular-nums">{children}</span>
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="px-5 py-4 text-sm text-bone-faint italic">{children}</p>
}

/* ── Credits ──────────────────────────────────────────────────────────────── */

function Credits({ item, onDone, onRemove }: PanelProps) {
  const { t } = useI18n()
  const add = useAdd(item.id, 'credits', onDone)

  const [name, setName] = useState('')
  const [character, setCharacter] = useState('')
  const [role, setRole] = useState('actor')

  const credits = item.credits ?? []
  const roles = [
    ['actor', t.admin.editor.children.actor],
    ['director', t.admin.editor.children.director],
    ['writer', t.admin.editor.children.writer],
    ['producer', t.admin.editor.children.producer],
    ['guest', t.admin.editor.children.guest],
  ] as const

  return (
    <Panel id="credits" className="rise" style={{ animationDelay: '200ms' }}>
      <PanelHead
        title={t.admin.editor.children.credits}
        action={<Count>{credits.length}</Count>}
      />

      {credits.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="max-h-72 divide-y divide-rule overflow-y-auto"
          // Focusable and named: its rows hold no control of their own when
          // they came from a source, so without this a keyboard cannot
          // scroll the part of the list that does not fit.
          tabIndex={0}
          aria-label={t.admin.editor.children.credits}
        >
          {credits.map((credit) => (
            <ChildRow
              key={credit.id}
              manual={credit.isManual}
              onRemove={() =>
                onRemove({ path: `credits/${credit.id}`, label: credit.personName })
              }
            >
              <span className="text-sm text-bone">{credit.personName}</span>
              {credit.characterName ? (
                <span className="text-sm text-bone-faint">
                  {' '}
                  {t.admin.editor.children.as(credit.characterName)}
                </span>
              ) : null}
              {credit.creditType !== 'actor' ? (
                <span className="ml-2 font-mono text-[0.6875rem] text-slate">
                  {credit.creditType}
                </span>
              ) : null}
            </ChildRow>
          ))}
        </ul>
      )}

      <AddForm
        label={t.admin.editor.children.addCredit}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate(
            {
              personName: name,
              characterName: character || undefined,
              creditType: role,
            },
            {
              onSuccess: () => {
                setName('')
                setCharacter('')
              },
            },
          )
        }
      >
        <FormField label={t.admin.editor.children.personName} htmlFor="credit-name">
          <Input
            id="credit-name"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </FormField>
        <FormField label={t.admin.editor.children.character} htmlFor="credit-character">
          <Input
            id="credit-character"
            value={character}
            onChange={(event) => setCharacter(event.target.value)}
          />
        </FormField>
        <FormField label={t.admin.editor.children.role} htmlFor="credit-role">
          <Select
            id="credit-role"
            value={role}
            onChange={(event) => setRole(event.target.value)}
          >
            {roles.map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </Select>
        </FormField>
      </AddForm>
    </Panel>
  )
}

/* ── Alternative titles ───────────────────────────────────────────────────── */

function AlternativeTitles({ item, onDone, onRemove }: PanelProps) {
  const { t } = useI18n()
  const add = useAdd(item.id, 'alternative-titles', onDone)
  const [title, setTitle] = useState('')

  const titles = item.alternativeTitles ?? []

  return (
    <Panel id="titles" className="rise" style={{ animationDelay: '240ms' }}>
      <PanelHead title={t.admin.editor.children.titles} action={<Count>{titles.length}</Count>} />
      <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">
        {t.admin.editor.children.titlesHint}
      </p>

      {titles.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="mt-2 max-h-56 divide-y divide-rule overflow-y-auto"
          // Focusable and named: its rows hold no control of their own when
          // they came from a source, so without this a keyboard cannot
          // scroll the part of the list that does not fit.
          tabIndex={0}
          aria-label={t.admin.editor.children.titles}
        >
          {titles.map((alt) => (
            <ChildRow
              key={alt.id}
              manual={alt.isManual}
              onRemove={() =>
                onRemove({ path: `alternative-titles/${alt.id}`, label: alt.title })
              }
            >
              <span className="text-sm text-bone">{alt.title}</span>
              {alt.language ? (
                <span className="ml-2 font-mono text-[0.6875rem] text-bone-faint uppercase">
                  {alt.language}
                </span>
              ) : null}
            </ChildRow>
          ))}
        </ul>
      )}

      <AddForm
        label={t.admin.editor.children.addTitle}
        pending={add.isPending}
        error={add.error}
        onSubmit={() => add.mutate({ title }, { onSuccess: () => setTitle('') })}
      >
        <FormField label={t.admin.editor.children.titles} htmlFor="alt-title">
          <Input
            id="alt-title"
            required
            value={title}
            onChange={(event) => setTitle(event.target.value)}
          />
        </FormField>
      </AddForm>
    </Panel>
  )
}

/* ── Artwork ──────────────────────────────────────────────────────────────── */

function Artwork({ item, onDone, onRemove }: PanelProps) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const add = useAdd(item.id, 'images', onDone)
  const queryClient = useQueryClient()

  const [url, setUrl] = useState('')
  const [coverType, setCoverType] = useState('poster')

  // What is kept of each address the work points at: shown beside the
  // picture, and the way to forget a copy.
  const media = useQuery({
    queryKey: ['item', item.id, 'media'],
    queryFn: () => api.get<WorkMedia>(`/items/${item.id}/media`),
    refetchInterval: (q) => (q.state.data?.media.some((m) => m.status === 'pending') ? 5_000 : false),
  })
  const keptOf = (imageUrl: string) =>
    media.data?.media.find((m) => m.url === imageUrl || m.origin === imageUrl)
  const storeOn = media.data?.store ?? false

  const done = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', item.id, 'media'] })
    onDone()
  }
  const forget = useMutation({
    mutationFn: (assetId: string) => api.delete(`/items/${item.id}/media/${assetId}`),
    onSuccess: done,
  })

  const images = item.images ?? []
  const kinds = [
    ['poster', c.poster],
    ['fanart', c.fanart],
    ['banner', c.banner],
    ['clearlogo', c.clearlogo],
  ] as const

  // The four offered for adding, plus the two providers also send.
  const coverLabel = (kind: string) =>
    kinds.find(([value]) => value === kind)?.[1] ??
    ({ landscape: c.landscape, clearart: c.clearart } as Record<string, string>)[kind] ??
    kind

  return (
    <Panel id="artwork" className="rise" style={{ animationDelay: '280ms' }}>
      <PanelHead title={c.artwork} action={<Count>{images.length}</Count>} />
      <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">{c.artworkHint}</p>

      {images.length === 0 ? (
        <Empty>{c.none}</Empty>
      ) : (
        <ul className="mt-2 max-h-56 divide-y divide-rule overflow-y-auto"
          // Focusable and named: its rows hold no control of their own when
          // they came from a source, so without this a keyboard cannot
          // scroll the part of the list that does not fit.
          tabIndex={0}
          aria-label={c.artwork}
        >
          {images.map((image) => {
            const kept = keptOf(image.url)
            return (
              <ChildRow
                key={image.id}
                manual={image.isManual}
                onRemove={() => onRemove({ path: `images/${image.id}`, label: image.url })}
                extra={
                  <Kept
                    medium={kept}
                    onForget={forget.mutate}
                    busy={forget.isPending && forget.variables === kept?.assetId}
                  />
                }
              >
                {/* The picture itself, small. Eighty-two lines of URLs said which
                    images existed and nothing about which one was which — the
                    only thing anybody opening this list wants to know. */}
                <span className="flex min-w-0 items-center gap-3">
                  <span
                    className={cn(
                      'shrink-0 overflow-hidden rounded-card border border-rule bg-ink-high',
                      image.coverType === 'poster' ? 'aspect-2/3 w-8' : 'aspect-video w-16',
                    )}
                  >
                    <Picture url={image.url} role="headshot" alt="" className="size-full object-cover" />
                  </span>
                  <span className="min-w-0">
                    <span className="block text-xs text-bone">
                      {coverLabel(image.coverType)}
                      {image.seasonNumber != null ? ` · ${t.work.season(image.seasonNumber)}` : ''}
                    </span>
                    <span className="block truncate font-mono text-[0.6875rem] text-bone-faint" title={kept?.origin ?? image.url}>
                      {kept?.origin.startsWith('upload:')
                        ? c.uploadedBy(kept.uploadedBy ?? '')
                        : hostOf(kept?.origin ?? image.url)}
                    </span>
                  </span>
                </span>
              </ChildRow>
            )
          })}
        </ul>
      )}
      {forget.isError ? (
        <p role="alert" className="px-5 py-2 text-xs text-vermillion">
          {forget.error.message || t.common.actionFailed}
        </p>
      ) : null}

      <AddForm
        label={c.addImage}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate({ coverType, url, sortOrder: 0 }, { onSuccess: () => setUrl('') })
        }
      >
        <FormField label={c.imageKind} htmlFor="image-kind">
          <Select
            id="image-kind"
            value={coverType}
            onChange={(event) => setCoverType(event.target.value)}
          >
            {kinds.map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </Select>
        </FormField>
        <FormField label={c.url} htmlFor="image-url">
          <Input
            id="image-url"
            required
            type="url"
            placeholder="https://…"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
          />
        </FormField>
      </AddForm>

      {storeOn ? (
        <>
          <UploadForm item={item} kind="image" kinds={kinds} onDone={done} />
          <UploadForm item={item} kind="theme" kinds={kinds} onDone={done} />
        </>
      ) : media.data ? (
        <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{c.storeOff}</p>
      ) : null}
    </Panel>
  )
}

/** What is kept of a picture, and the way to forget the copy. */
function Kept({
  medium,
  onForget,
  busy,
}: {
  medium: WorkMedium | undefined
  onForget: (assetId: string) => void
  busy: boolean
}) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  if (!medium || medium.status === 'absent') return null

  if (medium.status === 'stored') {
    return (
      <>
        <Chip tone="provider">
          <Glyph name="database" className="size-3" />
          {c.kept}
        </Chip>
        {medium.assetId && !medium.origin.startsWith('upload:') ? (
          <IconButton
            glyph="cloud"
            label={c.forgetCopy}
            busy={busy}
            onClick={() => onForget(medium.assetId!)}
          />
        ) : null}
      </>
    )
  }
  return (
    <Chip tone={medium.status === 'failed' ? 'accent' : 'neutral'}>
      <Glyph name={medium.status === 'failed' ? 'alert' : 'clock'} className="size-3" />
      {medium.status === 'failed' ? c.keptFailed : c.keptPending}
    </Chip>
  )
}

/**
 * A file put on the work: a picture, for a kind and maybe a season, or its
 * theme. The file goes to the server as it is; the server reads what it is.
 */
function UploadForm({
  item,
  kind,
  kinds,
  onDone,
}: {
  item: MediaItem
  kind: 'image' | 'theme'
  kinds: readonly (readonly [string, string])[]
  onDone: () => void
}) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const [open, setOpen] = useState(false)
  const [coverType, setCoverType] = useState('poster')
  const [season, setSeason] = useState('')
  const [file, setFile] = useState<File | null>(null)
  const input = useRef<HTMLInputElement>(null)
  // The button the form replaces: focus comes back to it on cancel.
  const trigger = useRef<HTMLButtonElement>(null)

  const upload = useMutation({
    mutationFn: (form: FormData) => api.upload<Uploaded>(`/items/${item.id}/media`, form),
    onSuccess: () => {
      setFile(null)
      if (input.current) input.current.value = ''
      onDone()
    },
  })

  const ids = `upload-${kind}`
  if (!open) {
    return (
      <div className="border-t border-rule px-5 py-3">
        <Button ref={trigger} size="sm" onClick={() => setOpen(true)}>
          <Glyph name={kind === 'image' ? 'image' : 'play'} className="size-4" />
          {kind === 'image' ? c.upload : c.uploadTheme}
        </Button>
        {upload.isSuccess ? (
          <span role="status" className="ml-3 text-xs text-moss">
            {c.uploaded}
          </span>
        ) : null}
      </div>
    )
  }

  return (
    <form
      className="border-t border-rule px-5 py-4"
      onSubmit={(event) => {
        event.preventDefault()
        if (!file) return
        const form = new FormData()
        form.set('file', file)
        form.set('kind', kind)
        if (kind === 'image') {
          form.set('coverType', coverType)
          if (season) form.set('seasonNumber', season)
        }
        upload.mutate(form)
      }}
    >
      <p className="mb-3 text-sm text-bone">{kind === 'image' ? c.upload : c.uploadTheme}</p>
      <div className="grid gap-3 sm:grid-cols-2">
        <FormField label={c.uploadFile} htmlFor={`${ids}-file`} hint={kind === 'image' ? c.uploadHint : c.themeHint}>
          <input
            ref={input}
            id={`${ids}-file`}
            type="file"
            required
            autoFocus
            accept={kind === 'image' ? 'image/jpeg,image/png,image/webp,image/gif,image/avif' : 'audio/*'}
            className="block w-full text-sm text-bone-dim file:mr-3 file:rounded-full file:border file:border-rule-bright file:bg-transparent file:px-3 file:py-1.5 file:text-sm file:text-bone"
            onChange={(event) => setFile(event.target.files?.[0] ?? null)}
          />
        </FormField>
        {kind === 'image' ? (
          <>
            <FormField label={c.imageKind} htmlFor={`${ids}-kind`}>
              <Select id={`${ids}-kind`} value={coverType} onChange={(event) => setCoverType(event.target.value)}>
                {kinds.map(([value, label]) => (
                  <option key={value} value={value}>
                    {label}
                  </option>
                ))}
              </Select>
            </FormField>
            {item.seasons?.length ? (
              <FormField label={c.uploadSeason} htmlFor={`${ids}-season`}>
                <Select id={`${ids}-season`} value={season} onChange={(event) => setSeason(event.target.value)}>
                  <option value="">{c.wholeWork}</option>
                  {item.seasons.map((s) => (
                    <option key={s.seasonNumber} value={String(s.seasonNumber)}>
                      {seasonName(s.title, s.seasonNumber, t.work.season)}
                    </option>
                  ))}
                </Select>
              </FormField>
            ) : null}
          </>
        ) : null}
      </div>
      {upload.isError ? (
        <p role="alert" className="mt-3 text-xs text-vermillion">
          {upload.error.message || t.common.actionFailed}
        </p>
      ) : null}
      <div className="mt-3 flex flex-wrap gap-2">
        <Button type="submit" variant="primary" size="sm" disabled={!file || upload.isPending}>
          {upload.isPending ? <Spinner className="size-4" /> : <Glyph name="download" className="size-4" />}
          {upload.isPending ? c.uploading : kind === 'image' ? c.upload : c.uploadTheme}
        </Button>
        <Button
          type="button"
          size="sm"
          onClick={() => {
            setOpen(false)
            requestAnimationFrame(() => trigger.current?.focus())
          }}
        >
          {t.common.cancel}
        </Button>
      </div>
    </form>
  )
}

/* ── Seasons ──────────────────────────────────────────────────────────────── */

function Seasons({ item, onDone, onRemove }: PanelProps) {
  const { t, locale } = useI18n()
  const add = useAdd(item.id, 'seasons', onDone)

  const [number, setNumber] = useState('1')
  const [title, setTitle] = useState('')
  const [airDate, setAirDate] = useState('')

  const seasons = [...(item.seasons ?? [])].sort((a, b) => a.seasonNumber - b.seasonNumber)

  return (
    <Panel id="seasons" className="rise" style={{ animationDelay: '320ms' }}>
      <PanelHead title={t.admin.editor.children.seasons} action={<Count>{seasons.length}</Count>} />

      {seasons.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="max-h-56 divide-y divide-rule overflow-y-auto"
          // Focusable and named: its rows hold no control of their own when
          // they came from a source, so without this a keyboard cannot
          // scroll the part of the list that does not fit.
          tabIndex={0}
          aria-label={t.admin.editor.children.seasons}
        >
          {seasons.map((season) => (
            <ChildRow
              key={season.id}
              manual={season.isManual}
              onRemove={() =>
                onRemove({
                  path: `seasons/${season.seasonNumber}`,
                  label: t.work.season(season.seasonNumber),
                })
              }
            >
              <span className="flex items-baseline gap-2">
                <span className="font-mono text-[0.8125rem] text-bone tabular-nums">
                  S{String(season.seasonNumber).padStart(2, '0')}
                </span>
                <span className="truncate text-sm text-bone-dim">
                  {seasonName(season.title, season.seasonNumber, t.work.season)}
                </span>
                {season.airDate ? (
                  <span className="shrink-0 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                    {fmt.shortDate(season.airDate, locale)}
                  </span>
                ) : null}
              </span>
            </ChildRow>
          ))}
        </ul>
      )}

      <AddForm
        label={t.admin.editor.children.addSeason}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate(
            {
              seasonNumber: Number(number),
              title: title || undefined,
              airDate: airDate || undefined,
            },
            {
              onSuccess: () => {
                setTitle('')
                setAirDate('')
              },
            },
          )
        }
      >
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField label={t.admin.editor.children.seasonNumber} htmlFor="season-number">
            <Input
              id="season-number"
              required
              inputMode="numeric"
              value={number}
              onChange={(event) => setNumber(event.target.value.replace(/\D/g, ''))}
            />
          </FormField>
          <FormField label={t.admin.editor.children.airDate} htmlFor="season-air-date">
            <Input
              id="season-air-date"
              placeholder="2026-01-05"
              value={airDate}
              onChange={(event) => setAirDate(event.target.value)}
            />
          </FormField>
        </div>
        <FormField label={t.admin.editor.children.seasonTitle} htmlFor="season-title">
          <Input
            id="season-title"
            value={title}
            onChange={(event) => setTitle(event.target.value)}
          />
        </FormField>
      </AddForm>
    </Panel>
  )
}

/* ── Episodes ─────────────────────────────────────────────────────────────── */

function Episodes({ item, onDone, onRemove }: PanelProps) {
  const { t, locale } = useI18n()
  const add = useAdd(item.id, 'episodes', onDone)

  const [season, setSeason] = useState('1')
  const [number, setNumber] = useState('1')
  const [title, setTitle] = useState('')
  const [airDate, setAirDate] = useState('')

  const episodes = item.episodes ?? []
  // Only the manual ones: a provider's four hundred episodes are the public
  // page's business, and none of them can be removed from here anyway.
  const mine = episodes.filter((episode) => episode.isManual)

  return (
    <Panel id="episodes" className="rise lg:col-span-2" style={{ animationDelay: '360ms' }}>
      <PanelHead
        title={t.admin.editor.children.episodes}
        action={
          <Count>
            {episodes.length}
            {mine.length > 0 ? ` · ${t.admin.editor.children.yoursCount(mine.length)}` : ''}
          </Count>
        }
      />

      {mine.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="max-h-72 divide-y divide-rule overflow-y-auto"
          // Focusable and named: its rows hold no control of their own when
          // they came from a source, so without this a keyboard cannot
          // scroll the part of the list that does not fit.
          tabIndex={0}
          aria-label={t.admin.editor.children.episodes}
        >
          {mine.map((episode) => (
            <ChildRow
              key={episode.id}
              manual
              onRemove={() =>
                onRemove({
                  path: `episodes/${episode.seasonNumber}/${episode.episodeNumber}`,
                  label: `S${String(episode.seasonNumber).padStart(2, '0')}E${String(
                    episode.episodeNumber,
                  ).padStart(2, '0')} ${episode.title}`.trim(),
                })
              }
            >
              <span className="flex items-baseline gap-2">
                <span className="shrink-0 font-mono text-[0.8125rem] text-bone tabular-nums">
                  S{String(episode.seasonNumber).padStart(2, '0')}E
                  {String(episode.episodeNumber).padStart(2, '0')}
                </span>
                <span className="truncate text-sm text-bone-dim">{episode.title || '—'}</span>
                {episode.airDate ? (
                  <span className="shrink-0 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                    {fmt.shortDate(episode.airDate, locale)}
                  </span>
                ) : null}
              </span>
            </ChildRow>
          ))}
        </ul>
      )}

      <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">
        {t.admin.editor.children.onlyManual}
      </p>

      <AddForm
        label={t.admin.editor.children.addEpisode}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate(
            {
              seasonNumber: Number(season),
              episodeNumber: Number(number),
              title: title || undefined,
              airDate: airDate || undefined,
            },
            {
              onSuccess: () => {
                setTitle('')
                setAirDate('')
              },
            },
          )
        }
      >
        <div className="grid gap-4 sm:grid-cols-4">
          <FormField label={t.admin.editor.children.seasonNumber} htmlFor="episode-season">
            <Input
              id="episode-season"
              required
              inputMode="numeric"
              value={season}
              onChange={(event) => setSeason(event.target.value.replace(/\D/g, ''))}
            />
          </FormField>
          <FormField label={t.admin.editor.children.episodeNumber} htmlFor="episode-number">
            <Input
              id="episode-number"
              required
              inputMode="numeric"
              value={number}
              onChange={(event) => setNumber(event.target.value.replace(/\D/g, ''))}
            />
          </FormField>
          <div className="sm:col-span-2">
            <FormField label={t.admin.editor.children.airDate} htmlFor="episode-air-date">
              <Input
                id="episode-air-date"
                placeholder="2026-01-05"
                value={airDate}
                onChange={(event) => setAirDate(event.target.value)}
              />
            </FormField>
          </div>
        </div>
        <FormField label={t.admin.editor.children.episodeTitle} htmlFor="episode-title">
          <Input
            id="episode-title"
            value={title}
            onChange={(event) => setTitle(event.target.value)}
          />
        </FormField>
      </AddForm>
    </Panel>
  )
}

/** Where an image lives, which is as much of its URL as a person reads. */
function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}
