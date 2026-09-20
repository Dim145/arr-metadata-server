import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState, type ReactNode } from 'react'

import { api } from '../lib/api'
import { cn } from '../lib/cn'
import type { MediaItem } from '../lib/types'
import { Alert, Button, Input, Label, Lock, Mono, Panel, PanelHead, Select, Spinner, Tag } from './ui'

/**
 * Children a person added, alongside the provider's.
 *
 * A manual row survives every refresh. A provider row cannot be deleted here at
 * all — it would simply come back, which reads as the delete having failed — so
 * the remove control only appears on rows marked manual.
 */
export function ManualChildren({ item, onChanged }: { item: MediaItem; onChanged: () => void }) {
  return (
    <div className="grid gap-6 lg:grid-cols-2">
      <Credits item={item} onChanged={onChanged} />
      <AlternativeTitles item={item} onChanged={onChanged} />
      <Images item={item} onChanged={onChanged} />
      {item.kind === 'series' && <Episodes item={item} onChanged={onChanged} />}
    </div>
  )
}

/* ── shared pieces ────────────────────────────────────────────────────────── */

function useChild(itemId: string, onChanged: () => void) {
  const queryClient = useQueryClient()

  const done = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', itemId] })
    onChanged()
  }

  const add = useMutation({
    mutationFn: ({ path, body }: { path: string; body: unknown }) =>
      api.post<{ id: string }>(`/items/${itemId}/${path}`, body),
    onSuccess: done,
  })

  const remove = useMutation({
    mutationFn: (path: string) => api.delete(`/items/${itemId}/${path}`),
    onSuccess: done,
  })

  return { add, remove }
}

function Row({
  children,
  manual,
  onRemove,
  removing,
}: {
  children: ReactNode
  manual: boolean
  onRemove?: () => void
  removing?: boolean
}) {
  return (
    <li className={cn('edge flex items-center gap-3 px-5 py-2', manual && 'edge-locked bg-phos/[0.035]')}>
      <div className="min-w-0 flex-1">{children}</div>
      {manual && (
        <>
          <Tag tone="manual">
            <Lock className="h-[10px] w-[10px]" />
            yours
          </Tag>
          <Button variant="danger" onClick={onRemove} disabled={removing} className="px-2 py-1">
            ✕
          </Button>
        </>
      )}
    </li>
  )
}

/** A form that collapses until asked for, so the panels stay readable. */
function AddForm({
  label,
  open,
  setOpen,
  onSubmit,
  pending,
  error,
  children,
}: {
  label: string
  open: boolean
  setOpen: (open: boolean) => void
  onSubmit: () => void
  pending: boolean
  error?: Error | null
  children: ReactNode
}) {
  if (!open) {
    return (
      <div className="border-t border-line px-5 py-3">
        <Button onClick={() => setOpen(true)}>+ {label}</Button>
      </div>
    )
  }

  return (
    <form
      className="flex flex-col gap-3 border-t border-line px-5 py-4"
      onSubmit={(event) => {
        event.preventDefault()
        onSubmit()
      }}
    >
      {children}
      {error && <Alert>{error.message}</Alert>}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" disabled={pending}>
          {pending ? <Spinner className="border-void/40 border-t-void" /> : 'Add'}
        </Button>
        <Button type="button" onClick={() => setOpen(false)}>
          Cancel
        </Button>
      </div>
    </form>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label>{label}</Label>
      {children}
    </div>
  )
}

/* ── credits ──────────────────────────────────────────────────────────────── */

function Credits({ item, onChanged }: { item: MediaItem; onChanged: () => void }) {
  const { add, remove } = useChild(item.id, onChanged)
  const [open, setOpen] = useState(false)
  const [name, setName] = useState('')
  const [character, setCharacter] = useState('')
  const [type, setType] = useState('actor')

  const credits = item.credits ?? []

  return (
    <Panel className="reveal" style={{ animationDelay: '220ms' }}>
      <PanelHead title="Credits" aside={<Mono className="text-faint">{credits.length}</Mono>} />
      <ul className="max-h-72 divide-y divide-line overflow-y-auto">
        {credits.map((credit) => (
          <Row
            key={credit.id}
            manual={credit.isManual}
            removing={remove.isPending}
            onRemove={() => remove.mutate(`credits/${credit.id}`)}
          >
            <span className="text-[14px] text-paper">{credit.personName}</span>
            {credit.characterName && (
              <span className="text-[13px] text-faint"> as {credit.characterName}</span>
            )}
            {credit.creditType !== 'actor' && (
              <Mono className="ml-2 text-[11px] text-signal">{credit.creditType}</Mono>
            )}
          </Row>
        ))}
      </ul>

      <AddForm
        label="Add a credit"
        open={open}
        setOpen={setOpen}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate(
            {
              path: 'credits',
              body: { personName: name, characterName: character || undefined, creditType: type },
            },
            { onSuccess: () => { setName(''); setCharacter(''); setOpen(false) } },
          )
        }
      >
        <Field label="Name">
          <Input required value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field label="Character">
          <Input value={character} onChange={(e) => setCharacter(e.target.value)} />
        </Field>
        <Field label="Role">
          <Select value={type} onChange={(e) => setType(e.target.value)}>
            <option value="actor">actor</option>
            <option value="director">director</option>
            <option value="writer">writer</option>
            <option value="producer">producer</option>
            <option value="guest">guest</option>
          </Select>
        </Field>
      </AddForm>
    </Panel>
  )
}

/* ── alternative titles ───────────────────────────────────────────────────── */

function AlternativeTitles({ item, onChanged }: { item: MediaItem; onChanged: () => void }) {
  const { add, remove } = useChild(item.id, onChanged)
  const [open, setOpen] = useState(false)
  const [title, setTitle] = useState('')

  const titles = item.alternativeTitles ?? []

  return (
    <Panel className="reveal" style={{ animationDelay: '260ms' }}>
      <PanelHead
        title="Alternative titles"
        aside={<Mono className="text-faint">{titles.length}</Mono>}
      />
      <p className="px-5 pt-3 text-[12px] leading-relaxed text-faint">
        Sonarr and Radarr match release names against these. Adding the spelling a release group
        actually uses is often what makes a download get recognised.
      </p>
      <ul className="mt-2 max-h-56 divide-y divide-line overflow-y-auto">
        {titles.map((alt) => (
          <Row
            key={alt.id}
            manual={alt.isManual}
            removing={remove.isPending}
            onRemove={() => remove.mutate(`alternative-titles/${alt.id}`)}
          >
            <span className="text-[14px] text-paper">{alt.title}</span>
            {alt.language && <Mono className="ml-2 text-[11px] text-faint">{alt.language}</Mono>}
          </Row>
        ))}
      </ul>

      <AddForm
        label="Add a title"
        open={open}
        setOpen={setOpen}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate(
            { path: 'alternative-titles', body: { title } },
            { onSuccess: () => { setTitle(''); setOpen(false) } },
          )
        }
      >
        <Field label="Title">
          <Input required value={title} onChange={(e) => setTitle(e.target.value)} />
        </Field>
      </AddForm>
    </Panel>
  )
}

/* ── images ───────────────────────────────────────────────────────────────── */

function Images({ item, onChanged }: { item: MediaItem; onChanged: () => void }) {
  const { add, remove } = useChild(item.id, onChanged)
  const [open, setOpen] = useState(false)
  const [url, setUrl] = useState('')
  const [type, setType] = useState('poster')

  const images = item.images ?? []

  return (
    <Panel className="reveal" style={{ animationDelay: '300ms' }}>
      <PanelHead title="Artwork" aside={<Mono className="text-faint">{images.length}</Mono>} />
      <p className="px-5 pt-3 text-[12px] leading-relaxed text-faint">
        A manual image is added ahead of the provider's, so clients pick it first.
      </p>
      <ul className="mt-2 max-h-56 divide-y divide-line overflow-y-auto">
        {images.map((image) => (
          <Row
            key={image.id}
            manual={image.isManual}
            removing={remove.isPending}
            onRemove={() => remove.mutate(`images/${image.id}`)}
          >
            <div className="flex items-center gap-2">
              <Mono className="text-[11px] text-signal">{image.coverType}</Mono>
              <span className="truncate text-[12px] text-faint">{image.url}</span>
            </div>
          </Row>
        ))}
      </ul>

      <AddForm
        label="Add an image"
        open={open}
        setOpen={setOpen}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate(
            { path: 'images', body: { coverType: type, url, sortOrder: 0 } },
            { onSuccess: () => { setUrl(''); setOpen(false) } },
          )
        }
      >
        <Field label="Kind">
          <Select value={type} onChange={(e) => setType(e.target.value)}>
            <option value="poster">poster</option>
            <option value="fanart">fanart</option>
            <option value="banner">banner</option>
            <option value="clearlogo">clearlogo</option>
          </Select>
        </Field>
        <Field label="URL">
          <Input
            required
            type="url"
            placeholder="https://…"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
        </Field>
      </AddForm>
    </Panel>
  )
}

/* ── episodes ─────────────────────────────────────────────────────────────── */

function Episodes({ item, onChanged }: { item: MediaItem; onChanged: () => void }) {
  const { add, remove } = useChild(item.id, onChanged)
  const [open, setOpen] = useState(false)
  const [season, setSeason] = useState('1')
  const [number, setNumber] = useState('1')
  const [title, setTitle] = useState('')
  const [airDate, setAirDate] = useState('')

  const episodes = item.episodes ?? []
  const manual = episodes.filter((e) => e.isManual)

  return (
    <Panel className="reveal lg:col-span-2" style={{ animationDelay: '340ms' }}>
      <PanelHead
        title="Episodes"
        aside={
          <Mono className="text-faint">
            {episodes.length} total{manual.length > 0 && ` · ${manual.length} yours`}
          </Mono>
        }
      />
      {manual.length > 0 && (
        <ul className="divide-y divide-line">
          {manual.map((episode) => (
            <Row
              key={episode.id}
              manual
              removing={remove.isPending}
              onRemove={() =>
                remove.mutate(`episodes/${episode.seasonNumber}/${episode.episodeNumber}`)
              }
            >
              <Mono className="text-paper">
                S{String(episode.seasonNumber).padStart(2, '0')}E
                {String(episode.episodeNumber).padStart(2, '0')}
              </Mono>
              <span className="ml-2 text-[14px] text-paper">{episode.title || '—'}</span>
              {episode.airDate && (
                <Mono className="ml-2 text-[11px] text-faint">{episode.airDate}</Mono>
              )}
            </Row>
          ))}
        </ul>
      )}

      <AddForm
        label="Add an episode"
        open={open}
        setOpen={setOpen}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate(
            {
              path: 'episodes',
              body: {
                seasonNumber: Number(season),
                episodeNumber: Number(number),
                title: title || undefined,
                airDate: airDate || undefined,
              },
            },
            { onSuccess: () => { setTitle(''); setAirDate(''); setOpen(false) } },
          )
        }
      >
        <div className="grid gap-3 sm:grid-cols-4">
          <Field label="Season">
            <Input
              required
              inputMode="numeric"
              value={season}
              onChange={(e) => setSeason(e.target.value.replace(/\D/g, ''))}
            />
          </Field>
          <Field label="Episode">
            <Input
              required
              inputMode="numeric"
              value={number}
              onChange={(e) => setNumber(e.target.value.replace(/\D/g, ''))}
            />
          </Field>
          <div className="sm:col-span-2">
            <Field label="Air date">
              <Input
                placeholder="2026-01-05"
                value={airDate}
                onChange={(e) => setAirDate(e.target.value)}
              />
            </Field>
          </div>
        </div>
        <Field label="Title">
          <Input value={title} onChange={(e) => setTitle(e.target.value)} />
        </Field>
      </AddForm>
    </Panel>
  )
}
