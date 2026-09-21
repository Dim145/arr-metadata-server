/**
 * The children a person added, alongside the providers'.
 *
 * A manual row survives every refresh; a provider's row cannot be removed here
 * at all, because it would simply come back and read as the delete having
 * failed. So the remove control exists only where it means something, and every
 * row that carries one says whose it is in a word as well as a colour.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState, type ReactNode } from 'react'

import { api } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import type { MediaItem } from '../lib/types'
import {
  Button,
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
}: {
  children: ReactNode
  manual: boolean
  onRemove?: () => void
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
    <Panel className="rise" style={{ animationDelay: '200ms' }}>
      <PanelHead
        title={t.admin.editor.children.credits}
        action={<Count>{credits.length}</Count>}
      />

      {credits.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="max-h-72 divide-y divide-rule overflow-y-auto">
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
    <Panel className="rise" style={{ animationDelay: '240ms' }}>
      <PanelHead title={t.admin.editor.children.titles} action={<Count>{titles.length}</Count>} />
      <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">
        {t.admin.editor.children.titlesHint}
      </p>

      {titles.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="mt-2 max-h-56 divide-y divide-rule overflow-y-auto">
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
  const add = useAdd(item.id, 'images', onDone)

  const [url, setUrl] = useState('')
  const [coverType, setCoverType] = useState('poster')

  const images = item.images ?? []
  const kinds = [
    ['poster', t.admin.editor.children.poster],
    ['fanart', t.admin.editor.children.fanart],
    ['banner', t.admin.editor.children.banner],
    ['clearlogo', t.admin.editor.children.clearlogo],
  ] as const

  return (
    <Panel className="rise" style={{ animationDelay: '280ms' }}>
      <PanelHead title={t.admin.editor.children.artwork} action={<Count>{images.length}</Count>} />
      <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">
        {t.admin.editor.children.artworkHint}
      </p>

      {images.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="mt-2 max-h-56 divide-y divide-rule overflow-y-auto">
          {images.map((image) => (
            <ChildRow
              key={image.id}
              manual={image.isManual}
              onRemove={() => onRemove({ path: `images/${image.id}`, label: image.url })}
            >
              <span className="flex min-w-0 items-center gap-2">
                <span className="shrink-0 font-mono text-[0.6875rem] text-slate">
                  {image.coverType}
                </span>
                <span className="truncate text-xs text-bone-faint">{image.url}</span>
              </span>
            </ChildRow>
          ))}
        </ul>
      )}

      <AddForm
        label={t.admin.editor.children.addImage}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate({ coverType, url, sortOrder: 0 }, { onSuccess: () => setUrl('') })
        }
      >
        <FormField label={t.admin.editor.children.imageKind} htmlFor="image-kind">
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
        <FormField label={t.admin.editor.children.url} htmlFor="image-url">
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
    </Panel>
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
    <Panel className="rise" style={{ animationDelay: '320ms' }}>
      <PanelHead title={t.admin.editor.children.seasons} action={<Count>{seasons.length}</Count>} />

      {seasons.length === 0 ? (
        <Empty>{t.admin.editor.children.none}</Empty>
      ) : (
        <ul className="max-h-56 divide-y divide-rule overflow-y-auto">
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
                  {season.title ?? t.work.season(season.seasonNumber)}
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
    <Panel className="rise lg:col-span-2" style={{ animationDelay: '360ms' }}>
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
        <ul className="max-h-72 divide-y divide-rule overflow-y-auto">
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
