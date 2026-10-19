/**
 * Everything this server holds, as a register.
 *
 * A fact table rather than a grid of posters: the questions asked here are
 * "which of these has somebody edited", "what is disabled", "what does this
 * record lack" and "when was this last refreshed". The poster at the head of
 * a row is the width of a thumbnail, there to know the row by at a glance;
 * the public side is where the posters belong.
 *
 * What a record lacks — a poster, an overview, an IMDb id, a first refresh —
 * is four marks on its row and four filters above the table, kept in the
 * address so that the dashboard's to-do list links to one: `?missing=poster`.
 * A row's actions are behind one button, which fits a phone as well as a
 * desk; the row itself opens the editor.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Link, useSearchParams } from 'react-router'

import { Artwork } from '../../components/media'
import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  type GlyphName,
  Input,
  Label,
  Panel,
  Segmented,
  Select,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Textarea,
  Th,
  Tr,
} from '../../components/ui'
import { ApiError, api, query, segment } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useSettled } from '../../lib/debounce'
import { useI18n } from '../../lib/i18n'
import { providerName, statusLabel } from '../../lib/labels'
import { hasSources, poster } from '../../lib/media'
import type {
  CuratedListPage,
  CuratedLists,
  Facets,
  ItemPage,
  MediaItem,
  MediaKind,
  ProvenanceReport,
  SourceRules,
} from '../../lib/types'

/** What an operator is being asked to confirm, and about which entry. */
type Asking = { item: MediaItem; what: 'delete' | 'disable' }

/** What a record can lack, as the server's `missing` filter names it, in the order the filters are drawn. */
const GAPS = ['poster', 'overview', 'imdb', 'refresh', 'divergingLock'] as const
type Gap = (typeof GAPS)[number]

const GAP_GLYPH: Record<Gap, GlyphName> = {
  poster: 'image',
  overview: 'journal',
  imdb: 'link',
  refresh: 'refresh',
  divergingLock: 'lock',
}

/** How one work came out of an action done to the whole selection. */
type Outcome = 'done' | 'unchanged' | 'skipped'

/** What is said once an action over the selection is over. */
type Report = { text: string; failures: { title: string; why: string }[] }

/** The fields a selection can be given one value of, as the override registry names them. */
const FIELDS = ['originalLanguage', 'status', 'network', 'studio', 'genres', 'isAdult', 'contentRating'] as const
type FieldName = (typeof FIELDS)[number]
type FieldAction = 'set' | 'add' | 'remove' | 'unlock'
type FieldChange = { field: FieldName; action: FieldAction; value: string }

/** The longest a value given to many works at once may be: what the server's filters take. */
const VALUE_MOST = 200

export function Catalogue() {
  const { t, locale } = useI18n()
  const w = t.admin.works
  const b = w.bulk
  const queryClient = useQueryClient()

  const [params, setParams] = useSearchParams()
  const [term, setTerm] = useState('')
  const [kind, setKind] = useState<'' | MediaKind>('')
  const [manualOnly, setManualOnly] = useState(false)
  // In the address, where the dashboard links: read from it only, or a filter
  // switched off came back on at the next reload.
  const failedOnly = params.get('refreshFailed') === '1'
  const asked = params.get('missing') ?? ''
  const missing = useMemo(() => {
    const named = asked.split(',').map((gap) => gap.trim())
    return GAPS.filter((gap) => named.includes(gap))
  }, [asked])
  const address = (change: (next: URLSearchParams) => void) => {
    const next = new URLSearchParams(window.location.search)
    change(next)
    setParams(next, { replace: true })
  }
  const setFailedOnly = (on: boolean) =>
    address((next) => {
      if (on) next.set('refreshFailed', '1')
      else next.delete('refreshFailed')
    })
  const toggleGap = (gap: Gap) => {
    const kept = missing.includes(gap) ? missing.filter((g) => g !== gap) : [...missing, gap]
    // In the order they are drawn, so that one view has one address.
    const ordered = GAPS.filter((g) => kept.includes(g))
    address((next) => {
      if (ordered.length) next.set('missing', ordered.join(','))
      else next.delete('missing')
    })
  }
  const [sort, setSort] = useState<'popularity' | 'title' | 'added' | 'refreshed'>('popularity')
  const [composing, setComposing] = useState(false)
  const [asking, setAsking] = useState<Asking | null>(null)

  const settled = useSettled(term)

  const list = useQuery({
    queryKey: ['items', settled, kind, manualOnly, failedOnly, missing.join(','), sort],
    queryFn: () =>
      api.get<ItemPage>(
        `/items${query({
          term: settled,
          kind,
          manualOnly,
          refreshFailed: failedOnly,
          missing: missing.join(','),
          sort,
          limit: 60,
          includeDisabled: true,
        })}`,
      ),
  })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['items'] })
    void queryClient.invalidateQueries({ queryKey: ['stats'] })
  }

  // The mutations live here rather than in the row so that the dialogs can too:
  // a <dialog> parked between table rows is invalid markup, and the top layer
  // does not care where in the document it was declared.
  const refresh = useMutation({
    mutationFn: (id: string) => api.post<MediaItem>(`/items/${id}/refresh`),
    onSuccess: invalidate,
  })

  // The same operation reports itself on the editor screen and used to say
  // nothing at all here, so a refused refresh looked like one that worked.
  const rowError = refresh.isError ? refresh.error.message : null

  const setEnabled = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.patch(`/items/${id}`, { isEnabled: enabled }),
    onSuccess: () => {
      setAsking(null)
      invalidate()
    },
  })

  const remove = useMutation({
    mutationFn: (id: string) => api.delete(`/items/${id}`),
    onSuccess: () => {
      setAsking(null)
      invalidate()
    },
  })

  // A selection across the rows, and what is done to all of it at once.
  const [selected, setSelected] = useState<Set<string>>(() => new Set())
  const [bulkAsk, setBulkAsk] = useState<'delete' | 'field' | 'reask' | null>(null)
  // A dialog of the selection's starts afresh each time it opens, and stays
  // mounted while it closes, so the browser gives the focus back to its button.
  const [openings, setOpenings] = useState(0)
  const ask = (what: 'field' | 'reask') => {
    setOpenings((n) => n + 1)
    setBulkAsk(what)
  }
  const [progress, setProgress] = useState<{ at: number; total: number } | null>(null)
  const [report, setReport] = useState<Report | null>(null)
  const busy = progress !== null
  const curated = useQuery({
    queryKey: ['admin-lists'],
    queryFn: () => api.get<CuratedLists>('/lists'),
    staleTime: 60_000,
  })
  const toggle = (id: string, on: boolean) => {
    setReport(null)
    setSelected((current) => {
      const next = new Set(current)
      if (on) next.add(id)
      else next.delete(id)
      return next
    })
  }
  // The selection is of what is on screen: a work that a new filter or a
  // refetch took off the page leaves it, so nothing is done to what cannot
  // be seen.
  const shown = list.data?.items
  useEffect(() => {
    if (!shown) return
    setSelected((current) => {
      const kept = new Set([...current].filter((id) => shown.some((item) => item.id === id)))
      return kept.size === current.size ? current : kept
    })
  }, [shown])
  // A partial selection shows as such in the box that selects every row.
  const allBox = useRef<HTMLInputElement>(null)
  const allShown = Boolean(shown?.length) && Boolean(shown?.every((item) => selected.has(item.id)))
  useEffect(() => {
    if (allBox.current) allBox.current.indeterminate = selected.size > 0 && !allShown
  }, [selected, allShown])

  // A field changed over the selection, which can be undone a moment; see
  // `undo` below.
  const [undoable, setUndoable] = useState<{ field: FieldName; works: MediaItem[] } | null>(null)
  const [heeded, setHeeded] = useState(false)

  /**
   * One action over the selection: each work its own request, four at a time,
   * so one that fails does not take the rest with it — and each its own line in
   * the audit trail. Counted as it goes, and summed up at the end, with what
   * failed and why. The works it was done for are handed back, for an undo.
   */
  const runBulk = async (act: (item: MediaItem) => Promise<Outcome | void>): Promise<MediaItem[]> => {
    const works = (shown ?? []).filter((item) => selected.has(item.id))
    if (!works.length) return []
    setReport(null)
    setUndoable(null)
    setProgress({ at: 0, total: works.length })
    const tally: Record<Outcome, number> = { done: 0, unchanged: 0, skipped: 0 }
    const failures: Report['failures'] = []
    const done: MediaItem[] = []
    const queue = [...works]
    const worker = async () => {
      for (let item = queue.shift(); item !== undefined; item = queue.shift()) {
        try {
          const outcome = (await act(item)) ?? 'done'
          tally[outcome] += 1
          if (outcome === 'done') done.push(item)
        } catch (error) {
          failures.push({ title: item.title, why: error instanceof Error ? error.message : t.common.actionFailed })
        }
        setProgress((now) => (now ? { ...now, at: now.at + 1 } : now))
      }
    }
    await Promise.all(Array.from({ length: Math.min(4, works.length) }, worker))
    setProgress(null)
    setReport({ text: b.summary(tally.done, tally.unchanged, tally.skipped, failures.length), failures })
    setSelected(new Set())
    invalidate()
    return done
  }

  /**
   * A field changed over the selection can be undone for eight seconds — for
   * as long as the pointer or the focus is on the offer, besides: each work
   * it changed is put back as its journal says it was, the lock it held or
   * none, four at a time. A work changed since by something else is left as
   * it is, and said so.
   */
  useEffect(() => {
    if (!undoable || heeded) return
    const timer = window.setTimeout(() => setUndoable(null), 8_000)
    return () => window.clearTimeout(timer)
  }, [undoable, heeded])
  const undo = async () => {
    if (!undoable) return
    const { field, works } = undoable
    setUndoable(null)
    setHeeded(false)
    setReport(null)
    setProgress({ at: 0, total: works.length })
    let undone = 0
    const failures: Report['failures'] = []
    const queue = [...works]
    const worker = async () => {
      for (let item = queue.shift(); item !== undefined; item = queue.shift()) {
        try {
          await api.post(`/items/${segment(item.id)}/changes/undo`, { scope: 'item', field })
          undone += 1
        } catch (error) {
          failures.push({ title: item.title, why: error instanceof Error ? error.message : t.common.actionFailed })
        }
        setProgress((now) => (now ? { ...now, at: now.at + 1 } : now))
      }
    }
    await Promise.all(Array.from({ length: Math.min(4, works.length) }, worker))
    setProgress(null)
    setReport({ text: b.undone(undone, failures.length), failures })
    invalidate()
  }

  const refreshEach = () =>
    runBulk(async (item) => {
      // A work entered by hand has nothing to be refreshed from: passed over,
      // not failed.
      if (item.isManual) return 'skipped'
      await api.post(`/items/${item.id}/refresh`)
    })
  const enableEach = (enabled: boolean) =>
    runBulk(async (item) => {
      if (item.isEnabled === enabled) return 'unchanged'
      await api.patch(`/items/${item.id}`, { isEnabled: enabled })
    })

  const addToList = async (listId: string) => {
    const list = curated.data?.lists.find((l) => l.id === listId)
    if (!list) return
    setProgress({ at: 0, total: 1 })
    setReport(null)
    try {
      const page = await api.get<CuratedListPage>(`/lists/${listId}`)
      const members = [...new Set([...page.items.map((item) => item.id), ...selected])]
      await api.put(`/lists/${listId}/items`, { items: members })
      setReport({ text: b.added(selected.size, list.name), failures: [] })
      setSelected(new Set())
      void queryClient.invalidateQueries({ queryKey: ['admin-lists'] })
      void queryClient.invalidateQueries({ queryKey: ['lists'] })
    } catch (error) {
      setReport({ text: error instanceof Error ? error.message : t.common.actionFailed, failures: [] })
    } finally {
      setProgress(null)
    }
  }

  const filtered = Boolean(term || kind || manualOnly || failedOnly || missing.length)
  // Narrowed to what records lack and nothing else: an empty list is good news.
  const onlyGaps = missing.length > 0 && !term && !kind && !manualOnly && !failedOnly

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8 flex flex-wrap items-end justify-between gap-4">
        <div className="min-w-0">
          <Label>{t.admin.title}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
            {t.admin.catalogue}
          </h1>
          <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{w.lead}</p>
        </div>

        {/* The way in is the page's one primary action; the way back out of
            the form is not. */}
        <Button variant={composing ? 'ghost' : 'primary'} onClick={() => setComposing((open) => !open)}>
          <Glyph name={composing ? 'close' : 'plus'} className="size-4" />
          {composing ? t.common.cancel : w.newEntry}
        </Button>
      </header>

      {composing ? <Composer onDone={() => setComposing(false)} /> : null}

      <div className="rise mb-6 border-y border-rule py-4">
        <div className="flex flex-wrap items-end gap-3">
          <div className="min-w-44 flex-1">
            <label htmlFor="works-search" className="label mb-1.5 block">
              {w.search}
            </label>
            <div className="relative">
              <Glyph
                name="search"
                className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
              />
              <Input
                id="works-search"
                type="search"
                value={term}
                onChange={(event) => setTerm(event.target.value)}
                className="pl-9"
              />
            </div>
          </div>

          <div className="min-w-36">
            <label htmlFor="works-kind" className="label mb-1.5 block">
              {w.kind}
            </label>
            <Select id="works-kind" value={kind} onChange={(event) => setKind(event.target.value as '' | MediaKind)}>
              <option value="">{w.allKinds}</option>
              <option value="series">{t.nav.series}</option>
              <option value="movie">{t.nav.films}</option>
            </Select>
          </div>

          <Button
            variant={manualOnly ? 'primary' : 'ghost'}
            aria-pressed={manualOnly}
            onClick={() => setManualOnly((was) => !was)}
          >
            <Glyph name="lock" className="size-4" />
            {w.manualOnly}
          </Button>

          <Button
            variant={failedOnly ? 'danger' : 'ghost'}
            aria-pressed={failedOnly}
            onClick={() => setFailedOnly(!failedOnly)}
          >
            <Glyph name="alert" className="size-4" />
            {w.failedOnly}
          </Button>

          <div className="min-w-40">
            <label htmlFor="works-sort" className="label mb-1.5 block">
              {t.browse.sort}
            </label>
            <Select id="works-sort" value={sort} onChange={(event) => setSort(event.target.value as typeof sort)}>
              <option value="popularity">{t.browse.orders.popular}</option>
              <option value="title">{t.browse.orders.title}</option>
              <option value="added">{t.browse.orders.added}</option>
              <option value="refreshed">{w.sortRefreshed}</option>
            </Select>
          </div>

          <span className="ml-auto flex items-center gap-3 self-center">
            {list.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
            {list.data ? (
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {w.showing(list.data.items.length, list.data.total)}
              </span>
            ) : null}
          </span>
        </div>

        {/* What records lack, each a filter of its own: pressed together they
            narrow, to the works lacking every one. */}
        <div role="group" aria-labelledby="works-missing" className="mt-3 flex flex-wrap items-center gap-2">
          <span id="works-missing" className="label mr-1">
            {w.missing.label}
          </span>
          {GAPS.map((gap) => {
            const on = missing.includes(gap)
            return (
              <Button key={gap} size="sm" variant={on ? 'primary' : 'ghost'} aria-pressed={on} onClick={() => toggleGap(gap)}>
                <Glyph name={GAP_GLYPH[gap]} className="size-3.5" />
                {w.missing[gap]}
              </Button>
            )
          })}
        </div>
      </div>

      {rowError ? (
        <p role="alert" className="mb-3 text-sm text-vermillion">
          {rowError}
        </p>
      ) : null}

      <Panel className="rise overflow-hidden" style={{ animationDelay: '60ms' }}>
        {list.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 8 }, (_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : list.isError ? (
          <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {list.error instanceof ApiError && list.error.status === 400 ? list.error.message : w.loadFailed}
          </p>
        ) : list.data.items.length === 0 ? (
          <EmptyState
            title={onlyGaps ? w.missing.none : filtered ? w.emptyFiltered : w.empty}
            hint={onlyGaps ? w.missing.noneHint : filtered ? w.emptyFilteredHint : w.emptyHint}
          />
        ) : (
          <>
            {selected.size > 0 || report || busy ? (
              <div
                role="region"
                aria-label={b.selected(selected.size)}
                className="flex flex-wrap items-center gap-2 border-b border-rule bg-ink-high px-4 py-2"
              >
                <span className="mr-2 font-mono text-xs text-bone tabular-nums">{b.selected(selected.size)}</span>
                {selected.size > 0 ? (
                  <>
                    <Button size="sm" disabled={busy} onClick={() => ask('field')}>
                      <Glyph name="pencil" className="size-3.5" />
                      {b.editField}
                    </Button>
                    <Button size="sm" disabled={busy} onClick={() => void refreshEach()}>
                      <Glyph name="refresh" className="size-3.5" />
                      {b.refresh}
                    </Button>
                    <Button size="sm" disabled={busy} onClick={() => ask('reask')}>
                      <Glyph name="cloud" className="size-3.5" />
                      {b.reask}
                    </Button>
                    <Button size="sm" disabled={busy} onClick={() => void enableEach(true)}>
                      {b.enable}
                    </Button>
                    <Button size="sm" disabled={busy} onClick={() => void enableEach(false)}>
                      {b.disable}
                    </Button>
                    {curated.data?.lists.some((list) => list.mode === 'manual') ? (
                      <Select
                        aria-label={b.addTo}
                        value=""
                        disabled={busy}
                        onChange={(event) => {
                          if (event.target.value) void addToList(event.target.value)
                        }}
                        className="min-h-11 w-auto py-0 text-xs"
                      >
                        <option value="">{b.addTo}</option>
                        {curated.data.lists
                          .filter((list) => list.mode === 'manual')
                          .map((list) => (
                            <option key={list.id} value={list.id}>
                              {list.name}
                            </option>
                          ))}
                      </Select>
                    ) : null}
                    <Button size="sm" variant="danger" disabled={busy} onClick={() => setBulkAsk('delete')}>
                      <Glyph name="trash" className="size-3.5" />
                      {b.delete}
                    </Button>
                    <Button
                      size="sm"
                      variant="quiet"
                      disabled={busy}
                      onClick={() => {
                        setSelected(new Set())
                        setReport(null)
                      }}
                    >
                      {b.clear}
                    </Button>
                  </>
                ) : null}
                {progress ? <Progress at={progress.at} total={progress.total} /> : null}
                {report ? (
                  <span role="status" className="text-xs text-bone-dim">
                    {report.text}
                  </span>
                ) : null}
                {report?.failures.length ? (
                  <details className="w-full text-xs text-bone-dim">
                    <summary className="inline-flex min-h-11 cursor-pointer items-center text-vermillion">
                      {b.failures(report.failures.length)}
                    </summary>
                    <ul className="mb-2 space-y-1">
                      {report.failures.map((failure, index) => (
                        <li key={index}>
                          <span className="text-bone">{failure.title}</span> — {failure.why}
                        </li>
                      ))}
                    </ul>
                  </details>
                ) : null}
              </div>
            ) : null}
            <TableScroll label={t.admin.catalogue}>
              <table className="w-full min-w-[20rem] border-collapse text-left">
                <thead>
                  <tr>
                    <Th className="w-8 pr-0">
                      <input
                        ref={allBox}
                        type="checkbox"
                        aria-label={b.selectAll}
                        className="size-4 accent-vermillion"
                        checked={allShown}
                        onChange={(event) =>
                          setSelected(event.target.checked ? new Set(list.data.items.map((item) => item.id)) : new Set())
                        }
                      />
                    </Th>
                    <Th>{w.colTitle}</Th>
                    <Th className="hidden lg:table-cell">{w.colIds}</Th>
                    <Th className="hidden sm:table-cell">{w.marks.label}</Th>
                    <Th className="hidden sm:table-cell">{w.colState}</Th>
                    <Th align="right" className="hidden md:table-cell">
                      {w.colUpdated}
                    </Th>
                    <Th align="right" className="w-12 pl-0">
                      <span className="sr-only">{w.colActions}</span>
                    </Th>
                  </tr>
                </thead>
                <tbody>
                  {list.data.items.map((item) => (
                    <Row
                      key={item.id}
                      item={item}
                      locale={locale}
                      busy={
                        (refresh.isPending && refresh.variables === item.id) ||
                        (setEnabled.isPending && setEnabled.variables.id === item.id)
                      }
                      onRefresh={() => refresh.mutate(item.id)}
                      onEnable={() => setEnabled.mutate({ id: item.id, enabled: true })}
                      onAsk={(what) => setAsking({ item, what })}
                      selected={selected.has(item.id)}
                      onSelect={(on) => toggle(item.id, on)}
                    />
                  ))}
                </tbody>
              </table>
            </TableScroll>
          </>
        )}
      </Panel>

      <Dialog
        open={asking?.what === 'delete'}
        title={w.deleteTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={remove.isPending} onClick={() => asking && remove.mutate(asking.item.id)}>
              <Glyph name="trash" className="size-4" />
              {t.common.delete}
            </Button>
          </>
        }
      >
        {asking ? w.deleteBody(asking.item.title) : null}
      </Dialog>

      <Dialog
        open={asking?.what === 'disable'}
        title={w.disableTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={setEnabled.isPending}
              onClick={() => asking && setEnabled.mutate({ id: asking.item.id, enabled: false })}
            >
              {w.disable}
            </Button>
          </>
        }
      >
        {asking ? w.disableBody(asking.item.title) : null}
      </Dialog>

      <Dialog
        open={bulkAsk === 'delete'}
        title={b.deleteTitle(selected.size)}
        onClose={() => setBulkAsk(null)}
        footer={
          <>
            <Button onClick={() => setBulkAsk(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={busy}
              onClick={() => {
                setBulkAsk(null)
                void runBulk(async (item) => {
                  await api.delete(`/items/${item.id}`)
                })
              }}
            >
              <Glyph name="trash" className="size-4" />
              {b.delete}
            </Button>
          </>
        }
      >
        {b.deleteBody}
      </Dialog>

      <FieldEditor
        key={`field-${openings}`}
        open={bulkAsk === 'field'}
        count={selected.size}
        onClose={() => setBulkAsk(null)}
        onApply={(change) => {
          setBulkAsk(null)
          void runBulk((item) => changeField(item, change)).then((done) => {
            if (done.length) setUndoable({ field: change.field, works: done })
          })
        }}
      />

      {undoable ? (
        <div
          role="status"
          onMouseEnter={() => setHeeded(true)}
          onMouseLeave={() => setHeeded(false)}
          onFocus={() => setHeeded(true)}
          onBlur={() => setHeeded(false)}
          className="fade-in fixed inset-x-4 bottom-4 z-40 mx-auto flex max-w-md items-center justify-between gap-3 rounded-panel border border-rule-bright bg-ink-top py-2 pr-2 pl-4 text-sm text-bone shadow-[var(--shadow-plate)] sm:right-6 sm:left-auto"
        >
          <span>{b.undoOffer(undoable.works.length)}</span>
          <Button size="sm" onClick={() => void undo()}>
            {b.undo}
          </Button>
        </div>
      ) : null}

      <Reask
        key={`reask-${openings}`}
        open={bulkAsk === 'reask'}
        count={selected.size}
        onClose={() => setBulkAsk(null)}
        onApply={(sources) => {
          setBulkAsk(null)
          void runBulk((item) => reask(item, sources))
        }}
      />
    </div>
  )
}

/** How far an action over the selection has gone: a bar, and the count beside it. */
function Progress({ at, total }: { at: number; total: number }) {
  const { t } = useI18n()

  return (
    <span className="flex items-center gap-2 text-xs text-bone-dim">
      <span
        role="progressbar"
        aria-label={t.admin.works.bulk.progress}
        aria-valuemin={0}
        aria-valuemax={total}
        aria-valuenow={at}
        className="relative block h-1.5 w-24 overflow-hidden rounded-full bg-ink-top"
      >
        <span
          className="absolute inset-y-0 left-0 bg-vermillion transition-[width] duration-200"
          style={{ width: `${total ? (at / total) * 100 : 0}%` }}
        />
      </span>
      <span aria-hidden className="font-mono tabular-nums">
        {at} / {total}
      </span>
    </span>
  )
}

/**
 * One work's share of a field given to the whole selection: its own request,
 * which locks the field — or unlocks it, handing it back to the sources. A
 * genre added that is there already, or taken off one that is not, asks
 * nothing; a lock lifted where there was none is said so.
 */
async function changeField(item: MediaItem, { field, action, value }: FieldChange): Promise<Outcome> {
  if (action === 'unlock') {
    try {
      await api.delete(`/items/${item.id}/overrides/item/${field}`)
      return 'done'
    } catch (error) {
      if (error instanceof ApiError && error.status === 404) return 'unchanged'
      throw error
    }
  }
  if (field === 'genres') {
    const same = (genre: string) => genre.toLocaleLowerCase() === value.toLocaleLowerCase()
    const held = item.genres ?? []
    if (action === 'add' ? held.some(same) : !held.some(same)) return 'unchanged'
    const genres = action === 'add' ? [...held, value] : held.filter((genre) => !same(genre))
    await api.put(`/items/${item.id}/overrides`, { scope: 'item', field, value: genres })
    return 'done'
  }
  await api.put(`/items/${item.id}/overrides`, {
    scope: 'item',
    field,
    value: field === 'isAdult' ? value === 'true' : value,
  })
  return 'done'
}

/**
 * One work's share of a question put again to some sources: those of them
 * that describe it are asked, the rest keep their last answer. A work entered
 * by hand, one not refreshed in full since the server began recording where
 * its values come from, and one none of them describes are passed over.
 */
async function reask(item: MediaItem, sources: Set<string>): Promise<Outcome> {
  if (item.isManual) return 'skipped'
  const report = await api.get<ProvenanceReport>(`/items/${item.id}/provenance`)
  if (!report.provenance) return 'skipped'
  const asking = report.sources
    .filter((source) => sources.has(source.provider) && !source.unavailable)
    .map((source) => source.provider)
  if (!asking.length) return 'skipped'
  await api.post(`/items/${item.id}/sync`, { sources: asking })
  return 'done'
}

function Row({
  item,
  locale,
  busy,
  onRefresh,
  onEnable,
  onAsk,
  selected,
  onSelect,
}: {
  item: MediaItem
  locale: string
  busy: boolean
  onRefresh: () => void
  onEnable: () => void
  onAsk: (what: Asking['what']) => void
  selected: boolean
  onSelect: (on: boolean) => void
}) {
  const { t } = useI18n()
  const w = t.admin.works

  // A work with no source elsewhere has nothing a lock keeps a field from:
  // its edits are its record, and are not shown as locks.
  const locks = hasSources(item) ? (item.lockedFields?.length ?? 0) : 0
  const ids = item.externalIds
  const art = poster(item)

  return (
    <Tr data-work={item.id} className={cn('group', item.isEnabled ? '' : 'opacity-60', selected && 'bg-ink-high')}>
      {/* Above the stretched link, or the box would open the editor. */}
      <Td className="w-8 pr-0">
        <input
          type="checkbox"
          aria-label={w.bulk.select(item.title)}
          className="relative z-10 size-4 accent-vermillion"
          checked={selected}
          onChange={(event) => onSelect(event.target.checked)}
        />
      </Td>
      <Td className="w-full max-w-0 py-1.5">
        {/* The cell is the target: the pseudo-element grows the link to the
            height of the row, so a thumb landing anywhere in the column opens
            the editor while the link stays a link for the keyboard. */}
        <Link
          to={`/admin/catalogue/${item.id}`}
          aria-label={w.open(item.title)}
          className="flex items-center gap-3 after:absolute after:inset-0 after:content-['']"
        >
          {/* To know the row by: the title beside it says what it is, so the
              picture is not described again. */}
          <span className="aspect-2/3 w-6 shrink-0 overflow-hidden rounded-[3px] border border-rule bg-ink-high">
            {art ? <Artwork url={art} role="thumb" sizes="24px" alt="" className="size-full object-cover" /> : null}
          </span>
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
              {item.title}
            </span>
            {item.year ? (
              <span className="shrink-0 font-mono text-xs text-bone-faint tabular-nums">{item.year}</span>
            ) : null}
            <Glyph
              name={item.kind === 'series' ? 'tv' : 'film'}
              className="size-3.5 shrink-0 text-bone-faint"
              title={item.kind === 'series' ? t.nav.series : t.nav.films}
            />
          </span>
        </Link>

        {/* The narrow layout has neither the state nor the completeness
            column, and neither is a thing to discover only after opening the
            entry. */}
        <span className="mt-1 ml-9 flex flex-wrap items-center gap-1.5 sm:hidden">
          {item.isEnabled ? null : <Chip tone="accent">{w.disabled}</Chip>}
          {locks > 0 ? (
            <Chip tone="manual">
              <Glyph name="lock" className="size-3" />
              {locks}
            </Chip>
          ) : null}
          <Marks item={item} locale={locale} />
        </span>
      </Td>

      <Td className="hidden lg:table-cell">
        <span className="flex gap-x-3 font-mono text-[0.6875rem] whitespace-nowrap text-bone-faint tabular-nums">
          {ids.tvdb ? <span>tvdb:{ids.tvdb}</span> : null}
          {ids.tmdb ? <span>tmdb:{ids.tmdb}</span> : null}
          {ids.imdb ? <span>{ids.imdb}</span> : null}
        </span>
      </Td>

      <Td className="hidden sm:table-cell">
        <Marks item={item} locale={locale} />
      </Td>

      <Td className="hidden sm:table-cell">
        <span className="flex flex-wrap items-center gap-1.5">
          {item.isEnabled ? null : <Chip tone="accent">{w.disabled}</Chip>}
          {item.isManual ? (
            <Chip tone="manual">
              <Glyph name="pencil" className="size-3" />
              {t.work.manualEntry}
            </Chip>
          ) : null}
          {locks > 0 ? (
            <Chip tone="manual">
              <Glyph name="lock" className="size-3" />
              {w.locks(locks)}
            </Chip>
          ) : null}
          {/* The ordinary case, said quietly. Showing only the exceptions left
              most rows of this column blank, which reads as broken rather than
              as "nothing unusual here". */}
          {/* On one line: wrapped in a narrow column, it made every row of
              the catalogue three lines tall. */}
          {item.isEnabled && !item.isManual && locks === 0 ? (
            <span className="text-xs whitespace-nowrap text-bone-faint">{w.served}</span>
          ) : null}
        </span>
      </Td>

      <Td align="right" className="hidden whitespace-nowrap md:table-cell">
        <span className="font-mono text-xs text-bone-faint tabular-nums" title={fmt.dateTime(item.updatedAt, locale)}>
          {fmt.relative(item.updatedAt, locale)}
        </span>
      </Td>

      <Td align="right" className="w-12 py-0 pl-0">
        <RowMenu id={`row-actions-${item.id}`} label={w.menu.open(item.title)} busy={busy}>
          {(close) => (
            <>
              <MenuLink to={`/admin/catalogue/${item.id}`} glyph="pencil" onClick={close}>
                {w.menu.editor}
              </MenuLink>
              <MenuLink to={`/work/${item.id}`} glyph="external" onClick={close}>
                {w.menu.publicPage}
              </MenuLink>
              <MenuButton
                glyph="refresh"
                // Refused by the server, and said why rather than tried.
                disabled={item.isManual}
                hint={item.isManual ? w.marks.refresh.manual : undefined}
                onClick={() => {
                  close()
                  onRefresh()
                }}
              >
                {w.menu.refresh}
              </MenuButton>
              <MenuButton
                glyph="power"
                onClick={() => {
                  close()
                  if (item.isEnabled) onAsk('disable')
                  else onEnable()
                }}
              >
                {item.isEnabled ? w.menu.disable : w.menu.enable}
              </MenuButton>
              <hr className="my-1 border-rule" />
              <MenuButton
                glyph="trash"
                tone="danger"
                onClick={() => {
                  close()
                  onAsk('delete')
                }}
              >
                {w.menu.delete}
              </MenuButton>
            </>
          )}
        </RowMenu>
      </Td>
    </Tr>
  )
}

/**
 * What a record has of the four things the register looks for, a mark each:
 * quiet where it is there, brass and struck through where it is not, faded
 * where it cannot be — a work entered by hand is never refreshed. Each says
 * what it means in words, to the pointer that rests on it and to a screen
 * reader alike.
 */
function Marks({ item, locale }: { item: MediaItem; locale: string }) {
  const { t } = useI18n()
  const m = t.admin.works.marks

  const refresh = item.refreshedAt
    ? { state: 'has' as const, says: m.refresh.has(fmt.relative(item.refreshedAt, locale) ?? '') }
    : item.isManual
      ? { state: 'none' as const, says: m.refresh.manual }
      : hasSources(item)
        ? { state: 'lacks' as const, says: m.refresh.lacks }
        : { state: 'none' as const, says: m.refresh.none }
  const marks: { gap: Gap; state: 'has' | 'lacks' | 'none'; says: string }[] = [
    poster(item) ? { gap: 'poster', state: 'has', says: m.poster.has } : { gap: 'poster', state: 'lacks', says: m.poster.lacks },
    item.overview?.trim()
      ? { gap: 'overview', state: 'has', says: m.overview.has }
      : { gap: 'overview', state: 'lacks', says: m.overview.lacks },
    item.externalIds.imdb
      ? { gap: 'imdb', state: 'has', says: m.imdb.has }
      : { gap: 'imdb', state: 'lacks', says: m.imdb.lacks },
    { gap: 'refresh', ...refresh },
  ]

  return (
    <ul aria-label={m.label} className="flex items-center gap-1">
      {marks.map(({ gap, state, says }) => (
        <li
          key={gap}
          data-mark={gap}
          data-state={state}
          className={cn(
            'relative grid size-5 place-items-center',
            state === 'has' ? 'text-bone-faint' : state === 'lacks' ? 'text-brass' : 'text-bone-faint opacity-40',
          )}
        >
          <Glyph name={GAP_GLYPH[gap]} title={says} className="size-3.5" />
          {/* Struck through where it is missing: the colour is not the only sign. */}
          {state === 'lacks' ? (
            <span aria-hidden className="pointer-events-none absolute h-px w-5 -rotate-45 bg-current" />
          ) : null}
        </li>
      ))}
    </ul>
  )
}

/** Whether this engine keeps popovers itself: the top layer, a press elsewhere or Escape to close. */
const POPOVER = typeof HTMLElement !== 'undefined' && 'popover' in HTMLElement.prototype
/** Whether it also places one beside the button that opened it, by anchor positioning. */
const ANCHORED = POPOVER && typeof CSS !== 'undefined' && CSS.supports('position-area', 'block-end')

/**
 * A row's actions, behind one button: a popover of plain buttons and links,
 * not a menu. `role="menu"` promises arrow keys between its items, and these
 * are reached with Tab like any other button — which the popover sends into
 * them straight after the one that opened it.
 *
 * The browser keeps it: in the top layer, above the table's own scroll; shut
 * by a press elsewhere or by Escape, with the focus back on its button. It is
 * placed under that button by anchor positioning where the engine has it, and
 * by the button's own box where it does not; where the engine has no popover
 * at all, it is opened and shut by hand.
 */
function RowMenu({
  id,
  label,
  busy,
  children,
}: {
  id: string
  label: string
  busy: boolean
  children: (close: () => void) => ReactNode
}) {
  const trigger = useRef<HTMLButtonElement>(null)
  const panel = useRef<HTMLDivElement>(null)
  const [open, setOpen] = useState(false)

  const close = () => {
    if (POPOVER) panel.current?.hidePopover()
    setOpen(false)
  }

  // Under the button, the two right edges together — or above it, where the
  // screen ends first. Only where the engine cannot anchor it itself.
  const place = useCallback(() => {
    const button = trigger.current
    const menu = panel.current
    if (ANCHORED || !POPOVER || !button || !menu) return
    const at = button.getBoundingClientRect()
    const gap = 4
    const roomBelow = window.innerHeight - at.bottom - gap
    const above = menu.offsetHeight > roomBelow && at.top > roomBelow
    menu.style.top = `${above ? Math.max(gap, at.top - gap - menu.offsetHeight) : at.bottom + gap}px`
    menu.style.right = `${Math.max(gap, document.documentElement.clientWidth - at.right)}px`
  }, [])

  // Kept beside its button while what is under it scrolls.
  useEffect(() => {
    if (!open || ANCHORED || !POPOVER) return
    window.addEventListener('scroll', place, { capture: true, passive: true })
    window.addEventListener('resize', place, { passive: true })
    return () => {
      window.removeEventListener('scroll', place, { capture: true })
      window.removeEventListener('resize', place)
    }
  }, [open, place])

  // Without popovers: shut by a press elsewhere, or by Escape.
  useEffect(() => {
    if (POPOVER || !open) return
    const away = (event: PointerEvent) => {
      const target = event.target as Node
      if (!panel.current?.contains(target) && !trigger.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setOpen(false)
      trigger.current?.focus()
    }
    document.addEventListener('pointerdown', away)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('pointerdown', away)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  return (
    <span className="relative z-10 inline-flex">
      <button
        ref={trigger}
        type="button"
        aria-label={label}
        title={label}
        aria-expanded={open}
        aria-controls={id}
        disabled={busy}
        popoverTarget={POPOVER ? id : undefined}
        onClick={POPOVER ? undefined : () => setOpen((was) => !was)}
        className={cn(
          'grid size-11 shrink-0 cursor-pointer place-items-center rounded-card text-bone-faint',
          'transition-colors duration-150 hover:bg-ink-high hover:text-bone',
          'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent',
          open && 'bg-ink-high text-bone',
        )}
      >
        {busy ? <Spinner className="size-4" /> : <Glyph name="more" className="size-4" />}
      </button>
      <div
        ref={panel}
        id={id}
        popover={POPOVER ? 'auto' : undefined}
        hidden={POPOVER ? undefined : !open}
        // Placed before it is drawn, under its button, and again once it has
        // a height, in case it has to go above.
        onBeforeToggle={(event) => {
          if (event.newState === 'open') place()
        }}
        onToggle={(event) => {
          const opened = event.newState === 'open'
          setOpen(opened)
          if (opened) place()
        }}
        className={cn(
          'm-0 w-60 rounded-card border border-rule-bright bg-ink-raised p-1 text-left text-bone shadow-[var(--shadow-lift)]',
          POPOVER
            ? ANCHORED
              ? 'inset-auto mt-1 [position-area:block-end_span-inline-start] [position-try-fallbacks:flip-block]'
              : 'inset-auto'
            : 'absolute top-full right-0 z-20 mt-1',
        )}
      >
        {children(close)}
      </div>
    </span>
  )
}

const MENU_ITEM =
  'flex min-h-11 w-full cursor-pointer items-center gap-2.5 rounded-[6px] px-3 text-left text-sm transition-colors duration-150 ' +
  'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent'

function MenuButton({
  glyph,
  tone = 'quiet',
  disabled,
  hint,
  onClick,
  children,
}: {
  glyph: GlyphName
  tone?: 'quiet' | 'danger'
  disabled?: boolean
  /** Why it is unavailable, when it is. */
  hint?: string
  onClick: () => void
  children: ReactNode
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      title={hint}
      onClick={onClick}
      className={cn(
        MENU_ITEM,
        tone === 'danger'
          ? 'text-vermillion hover:bg-vermillion/10'
          : 'text-bone-dim hover:bg-ink-high hover:text-bone',
      )}
    >
      <Glyph name={glyph} className="size-4" />
      {children}
    </button>
  )
}

function MenuLink({
  to,
  glyph,
  onClick,
  children,
}: {
  to: string
  glyph: GlyphName
  onClick: () => void
  children: ReactNode
}) {
  return (
    <Link to={to} onClick={onClick} className={cn(MENU_ITEM, 'text-bone-dim hover:bg-ink-high hover:text-bone')}>
      <Glyph name={glyph} className="size-4" />
      {children}
    </Link>
  )
}

/**
 * One field given to every work of the selection: a value — a genre added or
 * taken off, for the genres — or the lock lifted. What will happen is said in
 * a sentence before anything does.
 */
function FieldEditor({
  open,
  count,
  onClose,
  onApply,
}: {
  open: boolean
  count: number
  onClose: () => void
  onApply: (change: FieldChange) => void
}) {
  const { t } = useI18n()
  const f = t.admin.works.bulk.field
  const [field, setField] = useState<FieldName>('status')
  const [action, setAction] = useState<FieldAction>('set')
  const [value, setValue] = useState('')
  const [tried, setTried] = useState(false)

  // What the catalogue already holds, offered as the value is typed.
  const facets = useQuery({
    queryKey: ['facets', 'admin'],
    queryFn: () => api.get<Facets>(`/facets${query({ includeDisabled: true })}`),
    enabled: open,
    staleTime: 60_000,
  })

  const choose = (next: FieldName) => {
    setField(next)
    setAction(next === 'genres' ? 'add' : 'set')
    setValue(next === 'isAdult' ? 'true' : '')
    setTried(false)
  }

  // A list is added to and taken from; every other field is given a value.
  const actions: FieldAction[] = field === 'genres' ? ['add', 'remove', 'unlock'] : ['set', 'unlock']
  const text = value.trim()
  const valued = action !== 'unlock' && field !== 'isAdult'
  const problem = !valued
    ? undefined
    : !text
      ? f.invalid.empty
      : text.length > VALUE_MOST
        ? f.invalid.long(VALUE_MOST)
        : field === 'originalLanguage' && !/^[a-z]{2,3}(-[a-z0-9]{2,8})?$/i.test(text)
          ? f.invalid.language
          : undefined

  const name = t.labels.fields[field]
  const said =
    field === 'isAdult' ? (value === 'true' ? f.yes : f.no) : field === 'status' ? (statusLabel(text, t) ?? text) : text
  const summary =
    action === 'unlock'
      ? f.unlocks(name, count)
      : action === 'add'
        ? f.adds(text, count)
        : action === 'remove'
          ? f.removes(text, count)
          : f.sets(name, said, count)
  const offered =
    field === 'genres'
      ? facets.data?.genres
      : field === 'network' || field === 'studio'
        ? facets.data?.networks
        : field === 'originalLanguage'
          ? facets.data?.languages
          : field === 'status'
            ? facets.data?.statuses
            : undefined

  const submit = () => {
    setTried(true)
    if (problem) {
      document.getElementById('bulk-field-value')?.focus()
      return
    }
    onApply({ field, action, value: field === 'originalLanguage' ? text.toLowerCase() : field === 'isAdult' ? value : text })
  }

  return (
    <Dialog
      open={open}
      title={f.title(count)}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t.common.cancel}</Button>
          <Button type="submit" form="bulk-field" variant="primary">
            {f.apply(count)}
          </Button>
        </>
      }
    >
      <form
        id="bulk-field"
        noValidate
        className="space-y-4"
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
      >
        <FormField label={f.field} htmlFor="bulk-field-name">
          <Select id="bulk-field-name" value={field} onChange={(event) => choose(event.target.value as FieldName)}>
            {FIELDS.map((option) => (
              <option key={option} value={option}>
                {t.labels.fields[option]}
              </option>
            ))}
          </Select>
        </FormField>

        <div className="space-y-1.5">
          <span className="label block">{f.action}</span>
          <Segmented<FieldAction>
            label={f.action}
            value={action}
            onChange={setAction}
            options={actions.map((option) => ({ value: option, label: f[option] }))}
          />
        </div>

        {action === 'unlock' ? null : field === 'isAdult' ? (
          <div className="space-y-1.5">
            <span className="label block">{name}</span>
            <Segmented
              label={name}
              value={value}
              onChange={setValue}
              options={[
                { value: 'true', label: f.yes },
                { value: 'false', label: f.no },
              ]}
            />
          </div>
        ) : (
          <FormField
            label={field === 'genres' ? f.genre : f.value}
            htmlFor="bulk-field-value"
            hint={field === 'originalLanguage' ? f.languageHint : undefined}
            error={tried ? problem : undefined}
          >
            <Input
              id="bulk-field-value"
              value={value}
              autoComplete="off"
              list={offered?.length ? 'bulk-field-offered' : undefined}
              aria-invalid={tried && problem ? true : undefined}
              onChange={(event) => setValue(event.target.value)}
            />
            {offered?.length ? (
              <datalist id="bulk-field-offered">
                {offered.map((facet) => (
                  <option key={facet.value} value={facet.value}>
                    {field === 'status' ? statusLabel(facet.value, t) : undefined}
                  </option>
                ))}
              </datalist>
            ) : null}
          </FormField>
        )}

        <p className="text-xs leading-relaxed text-bone-dim">{summary}</p>
        {field === 'isAdult' && action === 'set' && value === 'true' ? (
          <p className="text-xs leading-relaxed text-bone-faint">{f.adultHint}</p>
        ) : null}
      </form>
    </Dialog>
  )
}

/**
 * The sources to ask again about every work of the selection, ticked among
 * those switched on. Each work asks those of them that describe it.
 */
function Reask({
  open,
  count,
  onClose,
  onApply,
}: {
  open: boolean
  count: number
  onClose: () => void
  onApply: (sources: Set<string>) => void
}) {
  const { t } = useI18n()
  const r = t.admin.works.bulk.reaskDialog
  const [chosen, setChosen] = useState<Set<string>>(() => new Set())

  const rules = useQuery({
    queryKey: ['sources', 'rules'],
    queryFn: () => api.get<SourceRules>('/sources/rules'),
    enabled: open,
    staleTime: 60_000,
  })
  const on = (rules.data?.providers ?? []).filter((provider) => provider.on)

  const tick = (provider: string, ticked: boolean) =>
    setChosen((held) => {
      const next = new Set(held)
      if (ticked) next.add(provider)
      else next.delete(provider)
      return next
    })

  return (
    <Dialog
      open={open}
      title={r.title(count)}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t.common.cancel}</Button>
          <Button variant="primary" disabled={!chosen.size} onClick={() => onApply(chosen)}>
            {r.apply(chosen.size, count)}
          </Button>
        </>
      }
    >
      <p className="mb-4 text-sm leading-relaxed text-bone-dim">{r.explain}</p>
      {rules.isPending ? (
        <Spinner className="size-4" />
      ) : rules.isError ? (
        <p role="alert" className="text-sm text-vermillion">
          {t.admin.sourcesPage.loadFailed}
        </p>
      ) : on.length ? (
        <fieldset>
          <legend className="label mb-2">{r.choose}</legend>
          <ul className="grid gap-1 sm:grid-cols-2">
            {on.map((provider) => (
              <li key={provider.id}>
                <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-card px-2 text-sm text-bone hover:bg-ink-high">
                  <input
                    type="checkbox"
                    className="size-4 accent-vermillion"
                    checked={chosen.has(provider.id)}
                    onChange={(event) => tick(provider.id, event.target.checked)}
                  />
                  {providerName(provider.id)}
                </label>
              </li>
            ))}
          </ul>
        </fieldset>
      ) : (
        <p className="text-sm text-bone-faint">{r.none}</p>
      )}
    </Dialog>
  )
}

/** The statuses a work entered by hand may have, as its clients spell them: Sonarr's, and Radarr's. */
const STATUSES: Record<MediaKind, string[]> = {
  series: ['continuing', 'ended', 'upcoming'],
  movie: ['tba', 'announced', 'inCinemas', 'released'],
}

/** Genres typed in one line, each once whatever its case. */
function genresOf(text: string): string[] {
  const kept: string[] = []
  for (const genre of text.split(',').map((g) => g.trim())) {
    if (genre && !kept.some((g) => g.toLocaleLowerCase() === genre.toLocaleLowerCase())) kept.push(genre)
  }
  return kept
}

/**
 * An entry that exists on no provider at all.
 *
 * It belongs on this screen rather than behind its own route: the person who
 * has just searched for something and not found it is exactly the person who
 * needs to type it in. Every field the server takes is here, checked as the
 * server checks it, so a refusal is said beside the field before anything is
 * sent.
 */
function Composer({ onDone }: { onDone: () => void }) {
  const { t } = useI18n()
  const c = t.admin.works.compose
  const queryClient = useQueryClient()

  const [kind, setKind] = useState<MediaKind>('series')
  const [title, setTitle] = useState('')
  const [year, setYear] = useState('')
  const [overview, setOverview] = useState('')
  const [status, setStatus] = useState(STATUSES.series[0]!)
  const [runtime, setRuntime] = useState('')
  // The first day it aired, for a series; the day it reached cinemas, for a film.
  const [released, setReleased] = useState('')
  const [network, setNetwork] = useState('')
  const [studio, setStudio] = useState('')
  const [genres, setGenres] = useState('')
  const [tmdb, setTmdb] = useState('')
  const [tvdb, setTvdb] = useState('')
  const [imdb, setImdb] = useState('')
  const [tried, setTried] = useState(false)

  const changeKind = (next: MediaKind) => {
    setKind(next)
    setStatus(STATUSES[next][0]!)
  }

  const listed = genresOf(genres)
  const id = (digits: string) => (digits && !(Number(digits) > 0) ? c.invalid.id : undefined)
  // In the order the fields are laid out, so the first said is the first met.
  const problems: Record<string, string | undefined> = {
    year: year && !(/^\d{4}$/.test(year) && Number(year) >= 1800 && Number(year) <= 2200) ? c.invalid.year : undefined,
    title: !title.trim() ? c.invalid.title : title.trim().length > 500 ? c.invalid.long(500) : undefined,
    runtime: runtime && Number(runtime) > 10_000 ? c.invalid.runtime : undefined,
    network: network.trim().length > 200 ? c.invalid.long(200) : undefined,
    studio: studio.trim().length > 200 ? c.invalid.long(200) : undefined,
    overview: overview.trim().length > 20_000 ? c.invalid.long(20_000) : undefined,
    genres: listed.length > 50 || listed.some((genre) => genre.length > 200) ? c.invalid.genres : undefined,
    tmdb: id(tmdb),
    tvdb: id(tvdb),
    imdb: imdb.trim() && !/^tt\d{7,10}$/i.test(imdb.trim()) ? c.invalid.imdb : undefined,
  }
  const said = (field: string) => (tried ? problems[field] : undefined)
  const invalid = (field: string) => (said(field) ? true : undefined)

  const create = useMutation({
    mutationFn: () =>
      api.post<MediaItem>('/items', {
        kind,
        title: title.trim(),
        year: year ? Number(year) : undefined,
        overview: overview.trim() || undefined,
        status,
        runtime: runtime ? Number(runtime) : undefined,
        firstAired: kind === 'series' && released ? released : undefined,
        inCinemas: kind === 'movie' && released ? released : undefined,
        network: kind === 'series' ? network.trim() || undefined : undefined,
        studio: studio.trim() || undefined,
        genres: listed,
        externalIds: {
          ...(tmdb ? { tmdb: Number(tmdb) } : {}),
          ...(tvdb ? { tvdb: Number(tvdb) } : {}),
          ...(imdb.trim() ? { imdb: imdb.trim().toLowerCase() } : {}),
        },
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['items'] })
      void queryClient.invalidateQueries({ queryKey: ['stats'] })
      onDone()
    },
  })

  return (
    <Panel className="rise mb-6">
      <form
        noValidate
        className="grid gap-4 p-5 sm:grid-cols-2"
        onSubmit={(event) => {
          event.preventDefault()
          setTried(true)
          const first = Object.entries(problems).find(([, problem]) => problem)?.[0]
          if (first) {
            document.getElementById(`compose-${first}`)?.focus()
            return
          }
          create.mutate()
        }}
      >
        <h2 className="font-display text-lg font-medium text-bone sm:col-span-2">{c.title}</h2>

        <FormField label={t.admin.works.kind} htmlFor="compose-kind">
          <Select id="compose-kind" value={kind} onChange={(event) => changeKind(event.target.value as MediaKind)}>
            <option value="series">{t.nav.series}</option>
            <option value="movie">{t.nav.films}</option>
          </Select>
        </FormField>

        <FormField label={c.year} htmlFor="compose-year" hint={c.optional} error={said('year')}>
          <Input
            id="compose-year"
            inputMode="numeric"
            value={year}
            aria-invalid={invalid('year')}
            onChange={(event) => setYear(event.target.value.replace(/\D/g, '').slice(0, 4))}
          />
        </FormField>

        <div className="sm:col-span-2">
          <FormField label={c.workTitle} htmlFor="compose-title" error={said('title')}>
            <Input
              id="compose-title"
              required
              autoFocus
              value={title}
              aria-invalid={invalid('title')}
              onChange={(event) => setTitle(event.target.value)}
            />
          </FormField>
        </div>

        <FormField label={c.status} htmlFor="compose-status">
          <Select id="compose-status" value={status} onChange={(event) => setStatus(event.target.value)}>
            {STATUSES[kind].map((option) => (
              <option key={option} value={option}>
                {statusLabel(option, t)}
              </option>
            ))}
          </Select>
        </FormField>

        <FormField label={c.runtime} htmlFor="compose-runtime" hint={c.optional} error={said('runtime')}>
          <Input
            id="compose-runtime"
            inputMode="numeric"
            value={runtime}
            aria-invalid={invalid('runtime')}
            onChange={(event) => setRuntime(event.target.value.replace(/\D/g, '').slice(0, 5))}
          />
        </FormField>

        <FormField label={kind === 'series' ? c.firstAired : c.inCinemas} htmlFor="compose-released" hint={c.optional}>
          <Input id="compose-released" type="date" value={released} onChange={(event) => setReleased(event.target.value)} />
        </FormField>

        {kind === 'series' ? (
          <FormField label={c.network} htmlFor="compose-network" hint={c.optional} error={said('network')}>
            <Input
              id="compose-network"
              value={network}
              aria-invalid={invalid('network')}
              onChange={(event) => setNetwork(event.target.value)}
            />
          </FormField>
        ) : null}

        <FormField label={c.studio} htmlFor="compose-studio" hint={c.optional} error={said('studio')}>
          <Input
            id="compose-studio"
            value={studio}
            aria-invalid={invalid('studio')}
            onChange={(event) => setStudio(event.target.value)}
          />
        </FormField>

        <div className="sm:col-span-2">
          <FormField label={c.overview} htmlFor="compose-overview" hint={c.optional} error={said('overview')}>
            <Textarea
              id="compose-overview"
              rows={3}
              value={overview}
              aria-invalid={invalid('overview')}
              onChange={(event) => setOverview(event.target.value)}
            />
          </FormField>
        </div>

        <div className="sm:col-span-2">
          <FormField label={c.genres} htmlFor="compose-genres" hint={c.genresHint} error={said('genres')}>
            <Input
              id="compose-genres"
              value={genres}
              aria-invalid={invalid('genres')}
              onChange={(event) => setGenres(event.target.value)}
            />
          </FormField>
        </div>

        <fieldset className="grid gap-4 sm:col-span-2 sm:grid-cols-3">
          <legend className="label mb-2 sm:col-span-3">{c.ids}</legend>
          <FormField label={c.tmdb} htmlFor="compose-tmdb" hint={c.optional} error={said('tmdb')}>
            <Input
              id="compose-tmdb"
              inputMode="numeric"
              value={tmdb}
              aria-invalid={invalid('tmdb')}
              onChange={(event) => setTmdb(event.target.value.replace(/\D/g, ''))}
            />
          </FormField>
          <FormField label={c.tvdb} htmlFor="compose-tvdb" hint={c.optional} error={said('tvdb')}>
            <Input
              id="compose-tvdb"
              inputMode="numeric"
              value={tvdb}
              aria-invalid={invalid('tvdb')}
              onChange={(event) => setTvdb(event.target.value.replace(/\D/g, ''))}
            />
          </FormField>
          <FormField label={c.imdb} htmlFor="compose-imdb" hint={c.optional} error={said('imdb')}>
            <Input
              id="compose-imdb"
              placeholder="tt0903747"
              autoComplete="off"
              value={imdb}
              aria-invalid={invalid('imdb')}
              onChange={(event) => setImdb(event.target.value)}
            />
          </FormField>
        </fieldset>

        <p className="text-xs leading-relaxed text-bone-faint sm:col-span-2">{c.note}</p>

        {create.isError ? (
          <p role="alert" className="flex items-center gap-2 text-sm text-vermillion sm:col-span-2">
            <Glyph name="alert" className="size-4 shrink-0" />
            {create.error.message}
          </p>
        ) : null}

        <div className="flex flex-wrap gap-2 sm:col-span-2">
          <Button type="submit" variant="primary" disabled={create.isPending}>
            {create.isPending ? <Spinner className="size-4" /> : null}
            {c.submit}
          </Button>
          <Button type="button" onClick={onDone}>
            {t.common.cancel}
          </Button>
        </div>
      </form>
    </Panel>
  )
}
