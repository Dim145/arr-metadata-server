/**
 * What each source gives a work, and which wins.
 *
 * The merge's rules, laid out for the people who have to trust its results:
 * a row per thing a work holds, a column per provider in the order the merge
 * consults them, and in each cell the provider's rank for that row — the
 * first with a value gives it, the next fill in what it lacks. Lists gathered
 * from everyone, and authorities that replace the rest, say so instead of a
 * number. Every mark is a word for a screen reader too. Under MyAnimeList,
 * which of its ways in answers, when it is not the first.
 *
 * The order is a setting, `sources.priority`, put in order here by an
 * administrator: a source moves a rank with its arrows, or is dragged. Every
 * merge reads it, so a work takes a new order at its next refresh.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Link } from 'react-router'

import { Button, Glyph, Label, Panel, PanelHead, Skeleton } from '../../components/ui'
import { api } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useMe } from '../../lib/hooks'
import { useI18n, type Dict } from '../../lib/i18n'
import { providerName } from '../../lib/labels'
import type { MalReach, SourceRule, SourceRules } from '../../lib/types'

const GROUPS = ['identity', 'release', 'classification', 'people', 'episodes'] as const

/**
 * Each source's terms of use, where they are published: what the catalogue
 * holds of them is said in a line, and the rest is theirs to say. Sonarr's
 * and Radarr's own services republish TheTVDB and TMDB under those two's
 * terms, and publish none of their own.
 */
const TERMS: { id: string; name?: string; url?: string }[] = [
  { id: 'tmdb', url: 'https://www.themoviedb.org/api-terms-of-use' },
  { id: 'tvdb', url: 'https://thetvdb.com/api-information' },
  { id: 'tvmaze', url: 'https://www.tvmaze.com/api' },
  { id: 'anilist', url: 'https://docs.anilist.co/guide/terms-of-use' },
  { id: 'mal', url: 'https://myanimelist.net/static/apiagreement.html' },
  { id: 'tenrai', name: 'Tenrai', url: 'https://tenrai.org/terms' },
  { id: 'fanart', url: 'https://fanart.tv/' },
  { id: 'imdb', url: 'https://data.imdb.com/non-commercial-datasets/' },
  { id: 'fankai', url: 'https://fankai.fr' },
  { id: 'skyhook' },
  { id: 'radarr' },
]

type Mark =
  | { kind: 'first' }
  | { kind: 'rank'; rank: number }
  | { kind: 'union' }
  | { kind: 'authority' }
  | { kind: 'either' }
  | { kind: 'none' }

/** What a provider is to a row. */
function markOf(row: SourceRule, provider: string): Mark {
  if (row.authorities?.includes(provider)) return { kind: 'authority' }
  const at = row.suppliers.indexOf(provider)
  if (at === -1) return { kind: 'none' }
  if (row.rule === 'union') return { kind: 'union' }
  if (row.rule === 'either') return { kind: 'either' }
  return at === 0 ? { kind: 'first' } : { kind: 'rank', rank: at + 1 }
}

function spoken(mark: Mark, t: Dict): string {
  const c = t.admin.sourcesPage.cell
  switch (mark.kind) {
    case 'first':
      return c.first
    case 'rank':
      return c.rank(mark.rank)
    case 'union':
      return c.union
    case 'authority':
      return c.authority
    case 'either':
      return c.either
    default:
      return c.none
  }
}

/**
 * What is said of how a source is reached: nothing while the first of its
 * ways in answers; which were passed over and which serves when another
 * does; that none answers when none does.
 */
function reachNote(reach: MalReach | undefined, t: Dict): { text: string; none: boolean } | null {
  if (!reach) return null
  const r = t.admin.sourcesPage.reach
  if (!reach.serving) return { text: r.none, none: true }
  const serving = reach.bases.findIndex((b) => b.base === reach.serving)
  const down = reach.bases.slice(0, serving).filter((b) => b.failedAt)
  if (serving <= 0 || !down.length) return null
  return { text: r.serving(down.map((b) => r[b.base]), r[reach.serving]), none: false }
}

/** A cell's mark, drawn: a filled disc for the first, a ring for the rest. */
function MarkGlyph({ mark }: { mark: Mark }) {
  switch (mark.kind) {
    case 'first':
      return (
        <span className="inline-grid size-6 place-items-center rounded-full bg-slate font-mono text-[0.6875rem] font-semibold text-ink">
          1
        </span>
      )
    case 'rank':
      return (
        <span className="inline-grid size-6 place-items-center rounded-full border border-slate-deep font-mono text-[0.6875rem] text-bone-dim">
          {mark.rank}
        </span>
      )
    case 'union':
      return <span className="font-mono text-sm text-slate">∪</span>
    case 'either':
      return <span className="font-mono text-sm text-slate">∨</span>
    case 'authority':
      return <Glyph name="star" className="size-4 text-brass" />
    default:
      return <span className="text-bone-faint">·</span>
  }
}

export function Sources() {
  const { t } = useI18n()
  const p = t.admin.sourcesPage

  const rules = useQuery({
    queryKey: ['sources', 'rules'],
    queryFn: () => api.get<SourceRules>('/sources/rules'),
    // How MyAnimeList is reached moves while the page is open.
    refetchInterval: 60_000,
  })

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.sourcesNav}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      </header>

      {rules.isPending ? (
        <Skeleton className="h-[40rem] w-full" />
      ) : rules.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {p.loadFailed}
        </p>
      ) : (
        <>
          <Order rules={rules.data} />
          <Matrix rules={rules.data} />
        </>
      )}

      <div className="mt-6 grid gap-6 md:grid-cols-2">
        {(
          [
            ['manualTitle', 'manual', 'lock'],
            ['imdbTitle', 'imdb', 'star'],
            ['skyhookTitle', 'skyhook', 'tv'],
            ['fankaiTitle', 'fankai', 'film'],
          ] as const
        ).map(([title, body, glyph], index) => (
          <Panel key={title} className="rise" style={{ animationDelay: `${160 + index * 40}ms` }}>
            <PanelHead
              title={
                <span className="inline-flex items-center gap-2">
                  <Glyph name={glyph} className="size-3.5" />
                  {p.notes[title]}
                </span>
              }
            />
            <p className="px-5 py-4 text-sm leading-relaxed text-bone-dim">{p.notes[body]}</p>
          </Panel>
        ))}
      </div>

      <Terms />
    </div>
  )
}

/** Each source's terms of use, in a line, with the way to the whole of them. */
function Terms() {
  const { t } = useI18n()
  const p = t.admin.sourcesPage.terms

  return (
    <Panel id="terms" label={p.title} className="rise mt-6" style={{ animationDelay: '320ms' }}>
      <PanelHead
        title={
          <span className="inline-flex items-center gap-2">
            <Glyph name="external" className="size-3.5" />
            {p.title}
          </span>
        }
      />
      <p className="px-5 pt-4 pb-2 text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      <ul className="divide-y divide-rule border-t border-rule">
        {TERMS.map(({ id, name, url }) => {
          const source = name ?? providerName(id)
          return (
            <li key={id} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-5 py-2.5 text-sm">
              <span className="w-28 shrink-0 font-medium text-bone">{source}</span>
              <span className="min-w-0 flex-1 text-bone-dim">{p.lines[id]}</span>
              {url ? (
                <a
                  href={url}
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex min-h-9 shrink-0 items-center gap-1 text-xs text-vermillion underline-offset-4 hover:underline"
                >
                  {p.read(source)}
                  <Glyph name="external" className="size-3" />
                </a>
              ) : null}
            </li>
          )
        })}
      </ul>
    </Panel>
  )
}

/** How long a move waits before it is saved: a run of presses is one change in the journal. */
const SETTLE_MS = 700

/** The setting the order is, written whole each time. */
const putOrder = (order: string[]) => api.put('/settings/server/-', { key: 'sources.priority', value: order.join(',') })

const ARROW =
  'grid size-11 shrink-0 cursor-pointer place-items-center rounded-full text-bone-faint transition-colors duration-150 ' +
  'hover:bg-ink-high hover:text-bone disabled:cursor-not-allowed disabled:opacity-30 disabled:hover:bg-transparent'

/**
 * The sources in the order the merge takes them, put in order by hand.
 *
 * A move shows at once and is saved a moment later, so a source walked across
 * five ranks is one change rather than five. The arrow pressed travels with
 * its source, and the focus with it — onto the other arrow at either end —
 * while its new rank is said aloud. Dragging does the same for a pointer,
 * beside the arrows rather than instead of them.
 */
function Order({ rules }: { rules: SourceRules }) {
  const { t } = useI18n()
  const p = t.admin.sourcesPage
  const o = p.reorder
  const queryClient = useQueryClient()
  const me = useMe()
  const admin = Boolean(me.data?.isAdmin)

  const served = rules.providers.map((provider) => provider.id).join(',')
  const [order, setOrder] = useState(() => served.split(','))
  const [moved, setMoved] = useState('')
  const [saving, setSaving] = useState<'idle' | 'waiting' | 'saving' | 'saved' | 'failed'>('idle')
  const timer = useRef<number | undefined>(undefined)
  const unsaved = useRef<string[] | null>(null)
  const focusAfter = useRef<{ id: string; way: 'earlier' | 'later' } | null>(null)
  const [dragging, setDragging] = useState<string | null>(null)
  const [over, setOver] = useState<string | null>(null)

  // The server's order whenever nothing here waits to be saved: a change made
  // in another tab, or this one's own coming back.
  useEffect(() => {
    if (!unsaved.current) setOrder(served.split(','))
  }, [served])

  const save = useMutation({
    mutationKey: ['sources', 'priority'],
    mutationFn: putOrder,
    onMutate: () => setSaving('saving'),
    onSuccess: () => {
      setSaving('saved')
      void queryClient.invalidateQueries({ queryKey: ['sources', 'rules'] })
      void queryClient.invalidateQueries({ queryKey: ['settings'] })
    },
    // Refused: the order shown goes back to the one the merge follows.
    onError: () => {
      setSaving('failed')
      setOrder(served.split(','))
      void queryClient.invalidateQueries({ queryKey: ['sources', 'rules'] })
    },
  })

  // One save at a time: the next waits for the one under way.
  const flush = () => {
    window.clearTimeout(timer.current)
    const next = unsaved.current
    if (!next) return
    if (queryClient.isMutating({ mutationKey: ['sources', 'priority'] })) {
      timer.current = window.setTimeout(flush, SETTLE_MS)
      return
    }
    unsaved.current = null
    save.mutate(next)
  }

  // Saved before the page is left rather than lost with it.
  useEffect(
    () => () => {
      window.clearTimeout(timer.current)
      if (unsaved.current) void putOrder(unsaved.current)
    },
    [],
  )

  const put = (next: string[], id: string) => {
    setOrder(next)
    setMoved(o.moved(providerName(id), next.indexOf(id) + 1, next.length))
    unsaved.current = next
    setSaving('waiting')
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(flush, SETTLE_MS)
  }

  const step = (id: string, by: -1 | 1) => {
    const to = order.indexOf(id) + by
    if (to < 0 || to >= order.length) return
    const next = order.filter((provider) => provider !== id)
    next.splice(to, 0, id)
    focusAfter.current = { id, way: by < 0 ? 'earlier' : 'later' }
    put(next, id)
  }

  // Dropped on another, it takes that one's place.
  const dropOn = (target: string) => {
    const id = dragging
    setDragging(null)
    setOver(null)
    if (!id || id === target) return
    const next = order.filter((provider) => provider !== id)
    next.splice(order.indexOf(target), 0, id)
    put(next, id)
  }

  const started = rules.defaultOrder
  const atStart = order.join(',') === started.join(',')
  const reset = () => {
    setOrder(started)
    setMoved(o.resetDone)
    unsaved.current = started
    flush()
  }

  // The arrow pressed has moved with its source; the focus follows it.
  useLayoutEffect(() => {
    const after = focusAfter.current
    if (!after) return
    focusAfter.current = null
    const pressed = document.getElementById(`order-${after.id}-${after.way}`)
    const other = document.getElementById(`order-${after.id}-${after.way === 'earlier' ? 'later' : 'earlier'}`)
    const target = pressed instanceof HTMLButtonElement && !pressed.disabled ? pressed : other
    target?.focus()
  }, [order])

  const status = [
    moved,
    saving === 'waiting' || saving === 'saving' ? o.saving : saving === 'saved' ? o.saved : saving === 'failed' ? o.failed : '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <Panel label={o.title} className="rise mb-6" style={{ animationDelay: '40ms' }}>
      <PanelHead
        title={o.title}
        action={
          admin ? (
            <Button size="sm" variant="quiet" disabled={atStart} title={o.resetHint} onClick={reset}>
              <Glyph name="refresh" className="size-3.5" />
              {o.reset}
            </Button>
          ) : undefined
        }
      />
      <p className="px-5 pt-4 text-sm leading-relaxed text-bone-dim">{admin ? o.lead : o.adminOnly}</p>
      <ol aria-label={o.title} className="flex flex-wrap gap-2 px-5 py-4">
        {order.map((id, index) => {
          const provider = rules.providers.find((ranked) => ranked.id === id)
          const name = providerName(id)
          return (
            <li
              key={id}
              data-provider={id}
              draggable={admin}
              onDragStart={(event) => {
                setDragging(id)
                event.dataTransfer.effectAllowed = 'move'
                event.dataTransfer.setData('text/plain', id)
              }}
              onDragOver={(event) => {
                if (!dragging) return
                event.preventDefault()
                event.dataTransfer.dropEffect = 'move'
                setOver(id)
              }}
              onDragLeave={() => setOver((was) => (was === id ? null : was))}
              onDrop={(event) => {
                event.preventDefault()
                dropOn(id)
              }}
              onDragEnd={() => {
                setDragging(null)
                setOver(null)
              }}
              className={cn(
                'flex items-center rounded-full border bg-ink transition-colors duration-150',
                over === id && dragging !== id ? 'border-vermillion' : 'border-rule-bright',
                dragging === id && 'opacity-50',
                admin && 'cursor-grab active:cursor-grabbing',
              )}
            >
              {admin ? (
                <button
                  id={`order-${id}-earlier`}
                  type="button"
                  aria-label={o.earlier(name)}
                  title={o.earlier(name)}
                  disabled={index === 0}
                  onClick={() => step(id, -1)}
                  className={ARROW}
                >
                  <Glyph name="chevronLeft" className="size-4" />
                </button>
              ) : null}
              <span className={cn('flex items-center gap-2 text-sm', admin ? 'px-1' : 'min-h-11 px-4')}>
                <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">{index + 1}</span>
                <span aria-hidden className={cn('block size-1.5 rounded-full', provider?.on ? 'bg-moss' : 'bg-bone-faint')} />
                <span className={provider?.on ? 'text-bone' : 'text-bone-dim'}>{name}</span>
                <span className="sr-only">
                  {`, ${o.rank(index + 1, order.length)}, ${provider?.on ? p.on : p.off}`}
                </span>
              </span>
              {admin ? (
                <button
                  id={`order-${id}-later`}
                  type="button"
                  aria-label={o.later(name)}
                  title={o.later(name)}
                  disabled={index === order.length - 1}
                  onClick={() => step(id, 1)}
                  className={ARROW}
                >
                  <Glyph name="chevronRight" className="size-4" />
                </button>
              ) : null}
            </li>
          )
        })}
      </ol>
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 border-t border-rule px-5 py-3 text-xs text-bone-faint">
        <Glyph name="clock" className="size-3.5 shrink-0" />
        <span>{o.next}</span>
        {/* The task's card, on a page only an administrator opens. */}
        {admin ? (
          <Link to="/admin/jobs#task-refresh.all" className="text-vermillion underline-offset-4 hover:underline">
            {o.refreshAll}
          </Link>
        ) : null}
        <span role="status" className={cn('sm:ml-auto', saving === 'failed' && 'text-vermillion')}>
          {status}
        </span>
      </p>
    </Panel>
  )
}

function Matrix({ rules }: { rules: SourceRules }) {
  const { t } = useI18n()
  const p = t.admin.sourcesPage
  const providers = rules.providers

  const label = (row: SourceRule) => p.rows[row.field] ?? row.field
  const only = (row: SourceRule) =>
    row.only ? (
      <span className="ml-2 rounded-full border border-rule-bright px-1.5 py-0.5 font-mono text-[0.625rem] tracking-[0.12em] text-bone-faint uppercase">
        {p.only[row.only]}
      </span>
    ) : null
  // Said only of a source switched on: one switched off is asked nothing.
  const notes = providers.flatMap((provider) => {
    const note = provider.on ? reachNote(provider.reach, t) : null
    return note ? [{ id: provider.id, note }] : []
  })
  const noteOf = (id: string) => notes.find((entry) => entry.id === id)?.note ?? null

  return (
    <Panel className="rise" style={{ animationDelay: '80ms' }}>
      <PanelHead title={p.matrix} action={<span className="label hidden sm:inline">{p.order}</span>} />

      {/* A phone gets each row as a sentence: who gives it, in order. Ten
          columns do not fit a thumb's width, and a table that scrolls
          sideways hides the one it is asked about. */}
      <div className="md:hidden">
        {notes.map(({ id, note }) => (
          <p
            key={id}
            className={cn(
              'flex items-center gap-2 border-b border-rule px-5 py-2.5 text-xs',
              note.none ? 'text-vermillion' : 'text-brass',
            )}
          >
            <Glyph name="alert" className="size-3.5 shrink-0" />
            <span>
              {providerName(id)} — {note.text}
            </span>
          </p>
        ))}
        {GROUPS.map((group) => {
          const rows = rules.rows.filter((row) => row.group === group)
          if (!rows.length) return null
          return (
            <section key={group} aria-label={p.groups[group]}>
              <h2 className="label border-b border-rule bg-ink/40 px-5 py-2.5">{p.groups[group]}</h2>
              <ul className="divide-y divide-rule">
                {rows.map((row) => (
                  <li key={row.field} className="px-5 py-3">
                    <p className="text-sm text-bone">
                      {label(row)}
                      {only(row)}
                    </p>
                    <ol className="mt-2 flex flex-wrap gap-x-3 gap-y-1.5">
                      {[...(row.authorities ?? []), ...row.suppliers.filter((s) => !row.authorities?.includes(s))].map(
                        (provider) => {
                          const mark = markOf(row, provider)
                          const on = providers.find((q) => q.id === provider)?.on ?? false
                          return (
                            <li key={provider} className={cn('flex items-center gap-1.5 text-xs', on ? 'text-bone-dim' : 'text-bone-faint opacity-60')}>
                              <MarkGlyph mark={mark} />
                              {providerName(provider)}
                              <span className="sr-only">
                                {' '}
                                — {spoken(mark, t)}
                                {on ? '' : `, ${p.off}`}
                              </span>
                            </li>
                          )
                        },
                      )}
                    </ol>
                  </li>
                ))}
              </ul>
            </section>
          )
        })}
      </div>

      <div className="hidden overflow-x-auto md:block">
        <table className="w-full min-w-[56rem] border-collapse text-sm">
          <caption className="sr-only">{p.matrix}</caption>
          <thead>
            <tr className="border-b border-rule">
              <th scope="col" className="label px-5 py-3 text-left align-bottom">
                {p.field}
              </th>
              {providers.map((provider) => {
                const note = noteOf(provider.id)
                return (
                  <th key={provider.id} scope="col" className="px-2 py-3 text-center align-bottom font-normal">
                    <span
                      aria-hidden
                      className={cn(
                        'mx-auto mb-2 block size-2 rounded-full',
                        provider.on ? 'bg-moss' : 'bg-bone-faint',
                      )}
                    />
                    <span className={cn('label block', provider.on ? '' : 'opacity-60')}>{providerName(provider.id)}</span>
                    <span className="mt-1 block font-mono text-[0.625rem] text-bone-faint tabular-nums">{provider.rank}</span>
                    <span className="sr-only">{provider.on ? p.on : p.off}</span>
                    {note ? (
                      <span
                        className={cn(
                          'mx-auto mt-1.5 block max-w-[7rem] text-[0.625rem] leading-snug',
                          note.none ? 'text-vermillion' : 'text-brass',
                        )}
                      >
                        {note.text}
                      </span>
                    ) : null}
                  </th>
                )
              })}
            </tr>
          </thead>
          {/* A body per group, its name heading the rows beneath it. */}
          {GROUPS.map((group) => {
            const rows = rules.rows.filter((row) => row.group === group)
            if (!rows.length) return null
            return (
              <tbody key={group}>
                <tr className="border-b border-rule bg-ink/40">
                  <th scope="rowgroup" colSpan={providers.length + 1} className="label px-5 py-2.5 text-left">
                    {p.groups[group]}
                  </th>
                </tr>
                {rows.map((row) => (
                  <tr key={row.field} className="border-b border-rule transition-colors duration-150 hover:bg-ink-high">
                    <th scope="row" className="px-5 py-2.5 text-left font-normal text-bone">
                      {label(row)}
                      {only(row)}
                    </th>
                    {providers.map((provider) => {
                      const mark = markOf(row, provider.id)
                      return (
                        <td key={provider.id} className={cn('px-2 py-2.5 text-center', provider.on ? '' : 'opacity-40')}>
                          <span aria-hidden>
                            <MarkGlyph mark={mark} />
                          </span>
                          {/* The column's header names the provider. */}
                          <span className="sr-only">{spoken(mark, t)}</span>
                        </td>
                      )
                    })}
                  </tr>
                ))}
              </tbody>
            )
          })}
        </table>
      </div>

      <ul className="flex flex-wrap items-center gap-x-5 gap-y-2 border-t border-rule px-5 py-3 text-xs text-bone-faint">
        {(
          [
            [{ kind: 'first' }, p.legend.first],
            [{ kind: 'rank', rank: 3 }, p.legend.rank],
            [{ kind: 'union' }, p.legend.union],
            [{ kind: 'authority' }, p.legend.authority],
            [{ kind: 'either' }, p.legend.either],
          ] as [Mark, string][]
        ).map(([mark, meaning]) => (
          <li key={meaning} className="flex items-center gap-2">
            <span aria-hidden>
              <MarkGlyph mark={mark} />
            </span>
            {meaning}
          </li>
        ))}
        <li className="flex items-center gap-2">
          <span aria-hidden className="block size-2 rounded-full bg-bone-faint" />
          {p.legend.off}
        </li>
      </ul>
    </Panel>
  )
}
