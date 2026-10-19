/**
 * The orders of a series, in the seasons tab: the aired one, TheTVDB's
 * others, TMDB's episode groups and those made here, each with where it comes
 * from and what Sonarr v5 and Plex are offered of it; a new order made empty
 * or from another; one made here written place by place; and one frozen as
 * the aired numbering, after saying what that does.
 *
 * In the seasons tab rather than beside the identifiers: an order is the
 * episodes this tab lists, numbered otherwise, and writing one is moving them
 * about — what the tab is for. The other numberings panel of the elsewhere
 * tab lists them and leads here.
 *
 * Written whole, as the server keeps an order: a place is an episode the
 * work holds, or a record of the order's own for one it does not. Moving a
 * place up or down swaps what two places hold, their numbers staying where
 * they are; a number is changed by hand. No episode is split or merged.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useEffect, useId, useMemo, useState } from 'react'
import { Link, useLocation } from 'react-router'

import { Button, Chip, Dialog, FormField, Glyph, Input, Panel, PanelHead, Select, Spinner } from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import * as fmt from '../../../lib/format'
import { useOrders } from '../../../lib/hooks'
import { useI18n } from '../../../lib/i18n'
import { episodeCode } from '../../../lib/media'
import type { Episode, EpisodeOrder, MediaItem, Orders } from '../../../lib/types'

import { orderName, orderSeasons, placedEpisode } from './orders'
import { Count, Empty, Facts } from './shared'

/** A place as it is being written: its numbers, and what it holds. */
type Draft = {
  id: string
  seasonNumber: number
  episodeNumber: number
  absoluteNumber?: number
  episodeId?: string
  tvdbId?: number
  tmdbId?: number
  record?: { title: string; overview?: string; aired?: string; runtime?: number; image?: string }
}

/** What two places swap when one moves past the other: what they hold, not their numbers. */
const HELD = ['id', 'episodeId', 'tvdbId', 'tmdbId', 'record'] as const

type Asking = { what: 'freeze' | 'thaw' | 'remove'; order: EpisodeOrder }

export function OrdersPanel({ work, onChanged }: { work: MediaItem; onChanged: () => void }) {
  const { t, locale } = useI18n()
  const o = t.admin.editor.orders
  const queryClient = useQueryClient()
  const orders = useOrders(work.id)
  const [editing, setEditing] = useState<string | null>(null)
  const [asking, setAsking] = useState<Asking | null>(null)
  const [label, setLabel] = useState('')
  const [from, setFrom] = useState('official')
  const ids = { name: useId(), from: useId() }
  const location = useLocation()

  // Arriving from the other numberings of the elsewhere tab, which lead
  // here by the panel's anchor: brought into view once it is drawn.
  useEffect(() => {
    if (location.hash !== '#order-editor') return
    const frame = requestAnimationFrame(() => document.getElementById('order-editor')?.scrollIntoView({ block: 'start' }))
    return () => cancelAnimationFrame(frame)
  }, [location.hash])

  const base = `/items/${encodeURIComponent(work.id)}/orders`
  const list = orders.data?.orders ?? []
  const numbering = orders.data?.numbering
  const offered = new Set(numbering?.offered ?? [])
  const name = (order: EpisodeOrder) => orderName(order, t.season.order)
  const first = work.seasons?.find((s) => s.seasonNumber > 0)?.seasonNumber ?? 1

  const settle = (answer?: Orders) => {
    if (answer) queryClient.setQueryData(['orders', work.id], answer)
    else void queryClient.invalidateQueries({ queryKey: ['orders', work.id] })
  }

  const create = useMutation({
    mutationFn: (body: { label: string; from?: string }) => api.post<EpisodeOrder>(base, body),
    onSuccess: (made) => {
      setLabel('')
      settle()
      // Made to be written: opened at once.
      setEditing(made.key)
    },
  })

  const act = useMutation({
    mutationFn: ({ what, order }: Asking) =>
      what === 'remove'
        ? api.delete<void>(`${base}/${encodeURIComponent(order.key)}`).then(() => undefined)
        : what === 'freeze'
          ? api.post<Orders>(`${base}/${encodeURIComponent(order.key)}/freeze`)
          : api.delete<Orders>(`${base}/${encodeURIComponent(order.key)}/freeze`),
    onSuccess: (answer, { what, order }) => {
      settle(answer ?? undefined)
      if (what === 'remove' && editing === order.key) setEditing(null)
      setAsking(null)
      onChanged()
    },
  })

  /** Made from `key`: the form below takes it, and its name is asked for. */
  const copy = (key: string) => {
    setFrom(key)
    requestAnimationFrame(() => document.getElementById(ids.name)?.focus())
  }

  const being = list.find((order) => order.key === editing && order.source === 'local')

  return (
    <Panel id="order-editor" label={o.title} className="rise lg:col-span-2" style={{ animationDelay: '80ms' }}>
      <PanelHead
        title={
          <span className="flex items-center gap-2">
            {o.title}
            <Count>{list.length + 1}</Count>
          </span>
        }
      />
      <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">{o.lead}</p>

      {orders.isPending ? (
        // A quiet line at the height of a few rows until the orders come,
        // which they do at once and whole: the server asks TMDB nothing for
        // them, its groups being read when the series is refreshed.
        <p role="status" className="mt-3 flex min-h-32 items-center border-t border-rule px-5 text-xs text-bone-faint">
          {o.reading}
        </p>
      ) : (
        <ul className="mt-3 divide-y divide-rule border-t border-rule">
          {/* The aired order: the work's own episodes. */}
          <li className="flex flex-wrap items-center gap-x-3 gap-y-1 px-5 py-2.5">
            <span className="min-w-0 flex-1">
              <span className="flex flex-wrap items-center gap-2">
                <span className="text-sm font-medium text-bone">{o.aired}</span>
                <span className="font-mono text-[0.6875rem] text-bone-faint">official</span>
              </span>
              <Facts
                className="mt-0.5"
                items={[
                  orders.data?.aired ? o.airedBy(o.source[orders.data.aired.source] ?? orders.data.aired.source) : undefined,
                  t.admin.editor.contentValue(new Set((work.episodes ?? []).map((e) => e.seasonNumber)).size, work.episodes?.length ?? 0),
                ]}
              />
            </span>
            <Button size="sm" variant="quiet" aria-label={o.copyNamed(o.aired)} onClick={() => copy('official')}>
              <Glyph name="copy" className="size-4" />
              {o.copy}
            </Button>
          </li>

          {list.map((order) => {
            const seasons = orderSeasons(order)
            const target = seasons.includes(first) ? first : seasons[0]
            const local = order.source === 'local'
            return (
              <li key={order.key} className={cn('px-5 py-2.5', order.isFrozen && 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]')}>
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="text-sm font-medium text-bone">{name(order)}</span>
                      <span className="font-mono text-[0.6875rem] break-all text-bone-faint">{order.key}</span>
                      <Chip tone={local ? 'manual' : 'provider'}>{o.source[order.source] ?? order.source}</Chip>
                      {order.isFrozen ? (
                        <Chip tone="manual">
                          <Glyph name="lock" className="size-3" />
                          {o.frozen}
                        </Chip>
                      ) : null}
                    </span>
                    <Facts
                      className="mt-0.5"
                      items={[
                        order.episodes.length
                          ? t.admin.editor.contentValue(seasons.length, order.episodes.length)
                          : order.announced
                            ? o.unread(order.announced)
                            : undefined,
                        order.updatedAt ? fmt.shortDate(order.updatedAt, locale) : undefined,
                      ]}
                    />
                    {!offered.has(order.key) && (order.episodes.length || order.announced) ? (
                      <span className="mt-0.5 block text-xs text-bone-faint italic">{o.notOffered}</span>
                    ) : null}
                  </span>
                  {/* Named after the order: a list of a dozen orders is a dozen of each button. */}
                  <span className="flex flex-wrap items-center gap-1">
                    {target !== undefined ? (
                      <Link
                        to={`/work/${work.id}/season/${target}?order=${encodeURIComponent(order.key)}`}
                        aria-label={o.seeNamed(name(order))}
                        className="hit inline-flex min-h-11 items-center gap-1 rounded-full px-3 text-xs text-vermillion hover:bg-vermillion/10"
                      >
                        {o.see}
                        <Glyph name="chevronRight" className="size-3" />
                      </Link>
                    ) : null}
                    <Button size="sm" variant="quiet" aria-label={o.copyNamed(name(order))} onClick={() => copy(order.key)}>
                      <Glyph name="copy" className="size-4" />
                      {o.copy}
                    </Button>
                    {local ? (
                      <Button
                        size="sm"
                        variant="quiet"
                        aria-label={o.editNamed(name(order))}
                        aria-expanded={editing === order.key}
                        onClick={() => setEditing(editing === order.key ? null : order.key)}
                      >
                        <Glyph name="pencil" className="size-4" />
                        {o.edit}
                      </Button>
                    ) : null}
                    {order.isFrozen ? (
                      <Button size="sm" variant="quiet" aria-label={o.thawTitle(name(order))} onClick={() => setAsking({ what: 'thaw', order })}>
                        <Glyph name="unlock" className="size-4" />
                        {o.thaw}
                      </Button>
                    ) : order.episodes.length ? (
                      <Button size="sm" variant="quiet" aria-label={o.freezeTitle(name(order))} onClick={() => setAsking({ what: 'freeze', order })}>
                        <Glyph name="lock" className="size-4" />
                        {o.freeze}
                      </Button>
                    ) : null}
                    {local ? (
                      <Button size="sm" variant="quiet" aria-label={o.removeTitle(name(order))} onClick={() => setAsking({ what: 'remove', order })}>
                        <Glyph name="trash" className="size-4" />
                        {o.remove}
                      </Button>
                    ) : null}
                  </span>
                </div>
                {being && being.key === order.key ? (
                  <OrderEditor
                    key={`${being.key}:${being.updatedAt}`}
                    work={work}
                    order={being}
                    onSaved={(saved) => {
                      settle()
                      if (saved.isFrozen) onChanged()
                    }}
                    onClose={() => setEditing(null)}
                  />
                ) : null}
              </li>
            )
          })}
        </ul>
      )}

      {/* A new order: made empty, or from another, and opened to be written. */}
      <form
        className="grid gap-4 border-t border-rule bg-ink/40 px-5 py-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-end"
        onSubmit={(event) => {
          event.preventDefault()
          create.mutate({ label, from: from || undefined })
        }}
      >
        <FormField label={o.name} hint={o.nameHint} htmlFor={ids.name}>
          <Input id={ids.name} required maxLength={80} value={label} onChange={(event) => setLabel(event.target.value)} />
        </FormField>
        <FormField label={o.from} htmlFor={ids.from}>
          <Select id={ids.from} value={from} onChange={(event) => setFrom(event.target.value)}>
            <option value="">{o.fromNothing}</option>
            <option value="official">{o.aired}</option>
            {list.map((order) => (
              <option key={order.key} value={order.key}>
                {name(order)} · {o.source[order.source] ?? order.source}
              </option>
            ))}
          </Select>
        </FormField>
        <Button type="submit" variant="primary" size="sm" disabled={create.isPending || !label.trim()}>
          {create.isPending ? <Spinner className="size-4" /> : <Glyph name="plus" className="size-4" />}
          {o.made}
        </Button>
        {create.error ? (
          <p role="alert" className="flex items-center gap-1.5 text-xs text-vermillion sm:col-span-3">
            <Glyph name="alert" className="size-3.5 shrink-0" />
            {create.error.message || t.common.actionFailed}
          </p>
        ) : null}
      </form>

      <Dialog
        open={asking !== null}
        title={
          asking
            ? asking.what === 'freeze'
              ? o.freezeTitle(name(asking.order))
              : asking.what === 'thaw'
                ? o.thawTitle(name(asking.order))
                : o.removeTitle(name(asking.order))
            : ''
        }
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button size="sm" onClick={() => setAsking(null)}>
              {t.common.cancel}
            </Button>
            <Button
              size="sm"
              variant={asking?.what === 'remove' ? 'danger' : 'primary'}
              disabled={act.isPending}
              onClick={() => asking && act.mutate(asking)}
            >
              {act.isPending ? <Spinner className="size-4" /> : null}
              {asking?.what === 'freeze' ? o.freeze : asking?.what === 'thaw' ? o.thaw : o.remove}
            </Button>
          </>
        }
      >
        {asking?.what === 'freeze' ? (
          <div className="space-y-3">
            <p>{o.freezeWhat}</p>
            <p>{o.freezeXem}</p>
            <p>{o.freezeFiles}</p>
            <p>{o.freezeLater}</p>
          </div>
        ) : asking?.what === 'thaw' ? (
          <p>{o.thawWhat}</p>
        ) : (
          <p>{o.removeWhat}</p>
        )}
        {act.error ? (
          <p role="alert" className="mt-3 flex items-center gap-1.5 text-xs text-vermillion">
            <Glyph name="alert" className="size-3.5 shrink-0" />
            {act.error.message || t.common.actionFailed}
          </p>
        ) : null}
      </Dialog>
    </Panel>
  )
}

/** One order made here, written: its name, and its places one season at a time. */
function OrderEditor({
  work,
  order,
  onSaved,
  onClose,
}: {
  work: MediaItem
  order: EpisodeOrder
  onSaved: (saved: EpisodeOrder) => void
  onClose: () => void
}) {
  const { t } = useI18n()
  const o = t.admin.editor.orders
  const ids = { label: useId(), season: useId(), own: useId(), ownSeason: useId(), ownNumber: useId(), held: useId() }
  const [label, setLabel] = useState(order.label)
  const [drafts, setDrafts] = useState<Draft[]>(() =>
    order.episodes.map((place) => ({
      id: place.id,
      seasonNumber: place.seasonNumber,
      episodeNumber: place.episodeNumber,
      absoluteNumber: place.absoluteNumber,
      episodeId: place.episodeId,
      tvdbId: place.tvdbId,
      tmdbId: place.tmdbId,
      record: place.record
        ? {
            title: place.record.title,
            overview: place.record.overview,
            aired: place.record.aired,
            runtime: place.record.runtime,
            image: place.record.image,
          }
        : undefined,
    })),
  )
  const byId = useMemo(() => new Map((work.episodes ?? []).map((e) => [e.id, e])), [work.episodes])
  const seasons = [...new Set([...drafts.map((d) => d.seasonNumber), ...(work.seasons ?? []).map((s) => s.seasonNumber)])].sort(
    (a, b) => (a === 0 ? 1 : b === 0 ? -1 : a - b),
  )
  const [season, setSeason] = useState<number>(seasons.find((n) => n > 0) ?? seasons[0] ?? 1)
  const [ownTitle, setOwnTitle] = useState('')
  const [saidSaved, setSaidSaved] = useState(false)

  // The places of the season shown, by number, each with where it is kept.
  const shown = drafts
    .map((draft, at) => ({ draft, at }))
    .filter(({ draft }) => draft.seasonNumber === season)
    .sort((a, b) => a.draft.episodeNumber - b.draft.episodeNumber)
  const nextNumber = Math.max(0, ...drafts.filter((d) => d.seasonNumber === season).map((d) => d.episodeNumber)) + 1

  // A season and a number given twice cannot be kept: the first such pair.
  const twice = (() => {
    const seen = new Set<string>()
    for (const draft of drafts) {
      const code = episodeCode(draft)
      if (seen.has(code)) return code
      seen.add(code)
    }
    return undefined
  })()

  const placed = new Set(drafts.map((d) => d.episodeId).filter(Boolean))
  const unplaced = (work.episodes ?? []).filter((e) => !placed.has(e.id))

  const save = useMutation({
    mutationFn: () =>
      api.put<EpisodeOrder>(`/items/${encodeURIComponent(work.id)}/orders/${encodeURIComponent(order.key)}`, {
        label,
        places: drafts.map((draft) => ({
          id: draft.id,
          seasonNumber: draft.seasonNumber,
          episodeNumber: draft.episodeNumber,
          absoluteNumber: draft.absoluteNumber,
          episodeId: draft.episodeId,
          tvdbId: draft.tvdbId,
          tmdbId: draft.tmdbId,
          record: draft.episodeId ? undefined : draft.record,
        })),
      }),
    onSuccess: (saved) => {
      setSaidSaved(true)
      onSaved(saved)
    },
  })

  const change = (next: Draft[]) => {
    setDrafts(next)
    setSaidSaved(false)
  }

  /** Swap what the places `a` and `b` hold, their numbers staying put. */
  const swap = (a: number, b: number) => {
    const next = drafts.map((d) => ({ ...d }))
    const first = next[a]!
    const second = next[b]!
    for (const field of HELD) {
      const held = first[field]
      ;(first as Record<string, unknown>)[field] = second[field]
      ;(second as Record<string, unknown>)[field] = held
    }
    change(next)
  }

  const renumber = (at: number, field: 'seasonNumber' | 'episodeNumber', value: string) => {
    const number = Number(value)
    if (!Number.isInteger(number) || number < 0) return
    change(drafts.map((d, i) => (i === at ? { ...d, [field]: number } : d)))
  }

  const describe = (draft: Draft): { episode?: Episode; own: boolean; title: string } => {
    const found = placedEpisode(
      {
        id: draft.id,
        seasonNumber: draft.seasonNumber,
        episodeNumber: draft.episodeNumber,
        episodeId: draft.episodeId,
        tvdbId: draft.tvdbId,
        tmdbId: draft.tmdbId,
        record: draft.record,
      },
      byId,
    )
    return found
      ? { episode: found.episode, own: found.own, title: found.episode.title || t.episode.untitled(found.episode.episodeNumber) }
      : { own: false, title: o.gone }
  }

  return (
    <div className="mt-3 rounded-card border border-rule bg-ink/40">
      <div className="grid gap-4 border-b border-rule px-4 py-3 sm:grid-cols-[minmax(0,1fr)_12rem] sm:items-end">
        <FormField label={o.name} htmlFor={ids.label}>
          <Input
            id={ids.label}
            required
            maxLength={80}
            value={label}
            onChange={(event) => {
              setLabel(event.target.value)
              setSaidSaved(false)
            }}
          />
        </FormField>
        <FormField label={o.seasonShown} htmlFor={ids.season}>
          <Select id={ids.season} value={season} onChange={(event) => setSeason(Number(event.target.value))}>
            {seasons.map((n) => (
              <option key={n} value={n}>
                {t.work.season(n)}
              </option>
            ))}
          </Select>
        </FormField>
      </div>

      {shown.length ? (
        <ol aria-label={o.editing(label || order.label)} className="divide-y divide-rule">
          {shown.map(({ draft, at }, index) => {
            const { episode, own, title } = describe(draft)
            const code = episodeCode(draft)
            const above = shown[index - 1]
            const below = shown[index + 1]
            return (
              // What the place holds beside its numbers, and under them on a
              // phone: there, beside them, a title had the width of a word.
              <li key={draft.id} className="grid grid-cols-[auto_4.5rem_4.5rem_minmax(0,1fr)_auto] items-center gap-x-2 gap-y-1.5 px-4 py-2">
                <span className="flex flex-col">
                  <button
                    type="button"
                    className="hit grid size-6 cursor-pointer place-items-center rounded-full text-bone-faint hover:bg-ink-high hover:text-bone disabled:cursor-not-allowed disabled:opacity-30"
                    disabled={!above}
                    aria-label={o.moveUp(code)}
                    onClick={() => above && swap(at, above.at)}
                  >
                    <Glyph name="chevronUp" className="size-3.5" />
                  </button>
                  <button
                    type="button"
                    className="hit grid size-6 cursor-pointer place-items-center rounded-full text-bone-faint hover:bg-ink-high hover:text-bone disabled:cursor-not-allowed disabled:opacity-30"
                    disabled={!below}
                    aria-label={o.moveDown(code)}
                    onClick={() => below && swap(at, below.at)}
                  >
                    <Glyph name="chevronDown" className="size-3.5" />
                  </button>
                </span>
                <Input
                  aria-label={`${o.season} · ${code}`}
                  inputMode="numeric"
                  className="font-mono tabular-nums"
                  value={draft.seasonNumber}
                  onChange={(event) => renumber(at, 'seasonNumber', event.target.value.replace(/\D/g, '') || '0')}
                />
                <Input
                  aria-label={`${o.episode} · ${code}`}
                  inputMode="numeric"
                  className="font-mono tabular-nums"
                  value={draft.episodeNumber}
                  onChange={(event) => renumber(at, 'episodeNumber', event.target.value.replace(/\D/g, '') || '0')}
                />
                <span className="col-span-full min-w-0 sm:col-span-1 sm:col-start-4 sm:row-start-1">
                  {own ? (
                    <Input
                      aria-label={`${o.ownTitle} · ${code}`}
                      maxLength={300}
                      value={draft.record?.title ?? ''}
                      onChange={(event) =>
                        change(drafts.map((d, i) => (i === at ? { ...d, record: { ...(d.record ?? { title: '' }), title: event.target.value } } : d)))
                      }
                    />
                  ) : (
                    <span className={cn('block truncate text-sm', episode ? 'text-bone' : 'text-bone-faint italic')}>{title}</span>
                  )}
                  <span className="mt-0.5 flex flex-wrap items-center gap-2">
                    {own ? <Chip tone="manual">{o.own}</Chip> : null}
                    {episode && !own ? (
                      <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">{o.airedAs(episodeCode(episode))}</span>
                    ) : null}
                  </span>
                </span>
                {own || !episode ? (
                  <Button
                    size="sm"
                    variant="quiet"
                    className="col-start-5 row-start-1"
                    aria-label={o.removePlace(code)}
                    onClick={() => change(drafts.filter((_, i) => i !== at))}
                  >
                    <Glyph name="trash" className="size-4" />
                  </Button>
                ) : null}
              </li>
            )
          })}
        </ol>
      ) : (
        <Empty>{o.noPlaces}</Empty>
      )}

      {/* More places: an episode the work holds that the order does not place
          yet, or one of its own, both at the end of the season shown. */}
      <div className="grid gap-4 border-t border-rule px-4 py-3 sm:grid-cols-2">
        <FormField label={o.placeHeld} htmlFor={ids.held} hint={unplaced.length ? undefined : o.placeHeldNone}>
          <Select
            id={ids.held}
            value=""
            disabled={!unplaced.length}
            onChange={(event) => {
              const episode = unplaced.find((e) => e.id === event.target.value)
              if (!episode) return
              change([
                ...drafts,
                {
                  id: crypto.randomUUID(),
                  seasonNumber: season,
                  episodeNumber: nextNumber,
                  episodeId: episode.id,
                  tvdbId: episode.tvdbId,
                  tmdbId: episode.tmdbId,
                },
              ])
            }}
          >
            <option value="">{o.choose}</option>
            {unplaced.map((episode) => (
              <option key={episode.id} value={episode.id}>
                {episodeCode(episode)} · {episode.title || t.episode.untitled(episode.episodeNumber)}
              </option>
            ))}
          </Select>
        </FormField>
        <form
          className="flex items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            const title = ownTitle.trim()
            if (!title) return
            change([...drafts, { id: crypto.randomUUID(), seasonNumber: season, episodeNumber: nextNumber, record: { title } }])
            setOwnTitle('')
          }}
        >
          <div className="min-w-0 flex-1">
            <FormField label={o.addOwn} hint={o.ownHint} htmlFor={ids.own}>
              <Input id={ids.own} maxLength={300} placeholder={o.ownTitle} value={ownTitle} onChange={(event) => setOwnTitle(event.target.value)} />
            </FormField>
          </div>
          <Button type="submit" size="sm" disabled={!ownTitle.trim()} aria-label={o.addOwn}>
            <Glyph name="plus" className="size-4" />
          </Button>
        </form>
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t border-rule px-4 py-3">
        <Button variant="primary" size="sm" disabled={save.isPending || !!twice || !label.trim()} onClick={() => save.mutate()}>
          {save.isPending ? <Spinner className="size-4" /> : <Glyph name="check" className="size-4" />}
          {o.save}
        </Button>
        <Button size="sm" onClick={onClose}>
          {o.close}
        </Button>
        {twice ? (
          <p role="alert" className="flex items-center gap-1.5 text-xs text-vermillion">
            <Glyph name="alert" className="size-3.5 shrink-0" />
            {o.twice(twice)}
          </p>
        ) : save.error ? (
          <p role="alert" className="flex items-center gap-1.5 text-xs text-vermillion">
            <Glyph name="alert" className="size-3.5 shrink-0" />
            {save.error.message || t.common.actionFailed}
          </p>
        ) : saidSaved ? (
          <p role="status" className="text-xs text-moss">
            {o.saved}
          </p>
        ) : null}
      </div>
    </div>
  )
}
