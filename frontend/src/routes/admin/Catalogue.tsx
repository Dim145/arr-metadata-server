/**
 * Everything this server holds, as a register.
 *
 * A fact table rather than a grid of posters: the questions asked here are
 * "which of these has somebody edited", "what is disabled", "what does this
 * record lack" and "when was this last refreshed". The poster at the head of
 * a row is the width of a thumbnail, there to know the row by at a glance;
 * the public side is where the posters belong.
 *
 * What a record lacks — a poster, an overview, an IMDb id, a first refresh, a
 * picture the media store gave up on — is a mark on its row and a filter
 * above the table, each with how many works it would leave, kept in the
 * address so that the dashboard's to-do list links to one: `?missing=poster`.
 * A work that looks like another — the duplicates the server proposes — is
 * a filter of the same row, `?missing=duplicates`, and a mark on its row that
 * opens the two side by side, to be merged or said not to be one work.
 *
 * The search, the filters and the order are all in the address, so a view
 * of the register is its address: a few are offered above it, and the
 * person keeps their own under a name, with their account. The table is
 * the dense one (`ui/tables.tsx`): its columns ordered by their heads,
 * hidden and shown, its rows walked with the arrow keys — and what is done
 * to the rows chosen, or the one the focus is on, is in the palette too.
 * A row's actions are behind one button, which fits a phone as well as a
 * desk; the row itself opens the editor.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router'

import { useCommands, type Command } from '../../components/CommandPalette'
import { Artwork } from '../../components/media'
import {
  Button,
  Chip,
  DataTable,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  type GlyphName,
  Input,
  Label,
  Menu,
  Panel,
  Segmented,
  Select,
  Skeleton,
  Spinner,
  TableOptions,
  TBody,
  Td,
  Textarea,
  Th,
  THead,
  Tr,
  useDataTable,
  type Column,
  type Sort,
} from '../../components/ui'
import { ApiError, api, query, segment } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useSettled } from '../../lib/debounce'
import { useI18n } from '../../lib/i18n'
import { providerName, statusLabel } from '../../lib/labels'
import { hasSources, isLocked, poster } from '../../lib/media'
import { usePreference } from '../../lib/preferences'
import type {
  CuratedListPage,
  CuratedLists,
  DuplicateList,
  DuplicatePair,
  Facets,
  Gaps,
  ItemPage,
  MediaItem,
  MediaKind,
  ProvenanceReport,
  SourceRules,
} from '../../lib/types'

import { keepRun } from './editor/walk'
import { MergeDialog, type MergeOpening } from './MergeDialog'
import {
  BUILT_IN,
  COLUMN_SORTS,
  GAPS,
  readViews,
  refusal,
  VIEW_NAME,
  VIEWS_MOST,
  viewOf,
  type ColumnSort,
  type Gap,
  type SavedView,
} from './views'

/** What an operator is being asked to confirm, and about which entry. */
type Asking = { item: MediaItem; what: 'delete' | 'disable' }

const GAP_GLYPH: Record<Gap, GlyphName> = {
  poster: 'image',
  overview: 'journal',
  imdb: 'link',
  refresh: 'refresh',
  divergingLock: 'lock',
  duplicates: 'copy',
  mediaFailed: 'download',
}

/** The marks a row carries, of the gaps: those a work's card can say. */
type Mark = 'poster' | 'overview' | 'imdb' | 'refresh'

/** What the merge dialog is opened on: a work, and the other one when a pair says which. */
type Merging = { work: MergeOpening; other?: string; proposed?: boolean; flagged?: boolean }

/** One work's partner in a pair the server proposes: the pair, and the other work. */
type Partner = { pair: DuplicatePair; other: DuplicatePair['first'] }

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

/** How many works the register shows: one page, the editor walks past it. */
const PAGE = 60

export function Catalogue() {
  const { t, locale } = useI18n()
  const w = t.admin.works
  const b = w.bulk
  const c = t.palette.command
  const queryClient = useQueryClient()
  const navigate = useNavigate()

  // The view is the address: the search, the filters, the order. Read from
  // it only, or a filter switched off came back on at the next reload.
  const [params, setParams] = useSearchParams()
  const asked = params.get('missing') ?? ''
  const missing = useMemo(() => {
    const named = asked.split(',').map((gap) => gap.trim())
    return GAPS.filter((gap) => named.includes(gap))
  }, [asked])
  const kindParam = params.get('kind')
  const kind: '' | MediaKind = kindParam === 'series' || kindParam === 'movie' ? kindParam : ''
  const manualOnly = params.get('manual') === '1'
  const failedOnly = params.get('refreshFailed') === '1'
  const sortParam = params.get('sort')
  const sort: 'popularity' | ColumnSort = sortParam && sortParam in COLUMN_SORTS ? (sortParam as ColumnSort) : 'popularity'
  const orderParam = params.get('order')
  const order = orderParam === 'asc' || orderParam === 'desc' ? orderParam : undefined
  const searched = params.get('term') ?? ''
  const view = viewOf(params)

  const address = useCallback(
    (change: (next: URLSearchParams) => void) => {
      const next = new URLSearchParams(window.location.search)
      change(next)
      setParams(next, { replace: true })
    },
    [setParams],
  )
  const flag = (name: string, on: boolean) =>
    address((next) => {
      if (on) next.set(name, '1')
      else next.delete(name)
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
  const chooseView = (query: string) => setParams(new URLSearchParams(query), { replace: true })

  // The search is typed here and settles into the address, which the list
  // reads; a view chosen, or the back button, puts the address's back.
  const [term, setTerm] = useState(searched)
  const settled = useSettled(term)
  useEffect(() => {
    if ((new URLSearchParams(window.location.search).get('term') ?? '') === settled.trim()) return
    address((next) => {
      if (settled.trim()) next.set('term', settled)
      else next.delete('term')
    })
    // Only when what was typed settles: the address answers to the field.
  }, [settled]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    setTerm((typed) => (typed.trim() === searched.trim() ? typed : searched))
  }, [searched])

  const [composing, setComposing] = useState(false)
  const [asking, setAsking] = useState<Asking | null>(null)
  const [merging, setMerging] = useState<Merging | null>(null)
  // What the last merge or dismissal came to, above the register rather than
  // in the selection's bar: that bar goes with the rows, and the pair merged
  // is often the last one the register narrowed to duplicates had.
  const [mergeSaid, setMergeSaid] = useState<string | null>(null)
  // A dialog of its own each time it opens: a pair, then another.
  const [mergings, setMergings] = useState(0)
  const mergeOn = (opening: Merging) => {
    setMergings((n) => n + 1)
    setMergeSaid(null)
    setMerging(opening)
  }

  // The pairs the server proposes, looked for again as they are read: asked
  // while the register is narrowed to them, before the list — which reads
  // what that look wrote.
  const lookingForDuplicates = missing.includes('duplicates')
  const pairs = useQuery({
    queryKey: ['duplicates'],
    queryFn: () => api.get<DuplicateList>('/admin/duplicates'),
    enabled: lookingForDuplicates,
  })
  const partners = useMemo(() => {
    const by = new Map<string, Partner[]>()
    for (const pair of pairs.data?.pairs ?? []) {
      for (const [one, other] of [
        [pair.first, pair.second],
        [pair.second, pair.first],
      ] as const) {
        by.set(one.id, [...(by.get(one.id) ?? []), { pair, other }])
      }
    }
    return by
  }, [pairs.data])

  // How many works each gap would leave, beside its filter.
  const gaps = useQuery({
    queryKey: ['gaps'],
    queryFn: () => api.get<Gaps>('/admin/gaps'),
    staleTime: 60_000,
  })

  // The list's question, without its page: what the editor asks again to
  // walk past the page shown.
  const listQuery = query({
    term: searched,
    kind,
    manualOnly,
    refreshFailed: failedOnly,
    missing: missing.join(','),
    sort,
    order,
    includeDisabled: true,
  }).replace(/^\?/, '')
  const list = useQuery({
    enabled: !lookingForDuplicates || !pairs.isPending,
    queryKey: ['items', searched, kind, manualOnly, failedOnly, missing.join(','), sort, order],
    queryFn: () => api.get<ItemPage>(`/items?${listQuery}&limit=${PAGE}`),
    // The rows shown stay while the next order or filter is asked: the
    // table is not drawn anew, and the focus stays on the head pressed.
    placeholderData: (previous) => previous,
  })

  // The list as it is shown, for the editor's "next".
  useEffect(() => {
    if (!list.data) return
    keepRun({
      query: listQuery,
      ids: list.data.items.map((item) => item.id),
      titles: Object.fromEntries(list.data.items.map((item) => [item.id, item.title])),
      total: list.data.total,
    })
  }, [list.data, listQuery])

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['items'] })
    void queryClient.invalidateQueries({ queryKey: ['stats'] })
    void queryClient.invalidateQueries({ queryKey: ['gaps'] })
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

  // The row the focus is on, or rested on last: what the palette offers to
  // act on when nothing is selected.
  const [current, setCurrent] = useState<string | null>(null)
  const currentWork = shown?.find((item) => item.id === current)

  // A field changed over the selection, which can be undone a moment; see
  // `undo` below.
  const [undoable, setUndoable] = useState<{ field: FieldName; works: MediaItem[] } | null>(null)
  const [heeded, setHeeded] = useState(false)

  /**
   * One action over some works — the selection, unless others are named:
   * each work its own request, four at a time, so one that fails does not
   * take the rest with it — and each its own line in the audit trail.
   * Counted as it goes, and summed up at the end, with what failed and why.
   * The works it was done for are handed back, for an undo.
   */
  const runBulk = async (
    act: (item: MediaItem) => Promise<Outcome | void>,
    works: MediaItem[] = (shown ?? []).filter((item) => selected.has(item.id)),
  ): Promise<MediaItem[]> => {
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

  const refreshEach = (works?: MediaItem[]) =>
    runBulk(async (item) => {
      // A work entered by hand has nothing to be refreshed from: passed over,
      // not failed.
      if (item.isManual) return 'skipped'
      await api.post(`/items/${item.id}/refresh`)
    }, works)
  const enableEach = (enabled: boolean, works?: MediaItem[]) =>
    runBulk(async (item) => {
      if (item.isEnabled === enabled) return 'unchanged'
      await api.patch(`/items/${item.id}`, { isEnabled: enabled })
    }, works)
  // The overview kept as it is, from the next refresh on: locked to what is
  // served now, as an edit of it is. A work entered by hand has nothing to
  // keep it from, and one with no overview nothing to keep.
  const lockOverviews = (works?: MediaItem[]) =>
    runBulk(async (item) => {
      if (!hasSources(item) || !item.overview?.trim()) return 'skipped'
      if (isLocked(item, 'item/overview')) return 'unchanged'
      await api.put(`/items/${segment(item.id)}/overrides`, { scope: 'item', field: 'overview', value: item.overview })
    }, works)

  const addToList = async (listId: string, ids: string[] = [...selected]) => {
    const list = curated.data?.lists.find((l) => l.id === listId)
    if (!list || !ids.length) return
    setProgress({ at: 0, total: 1 })
    setReport(null)
    try {
      const page = await api.get<CuratedListPage>(`/lists/${listId}`)
      const members = [...new Set([...page.items.map((item) => item.id), ...ids])]
      await api.put(`/lists/${listId}/items`, { items: members })
      setReport({ text: b.added(ids.length, list.name), failures: [] })
      setSelected(new Set())
      void queryClient.invalidateQueries({ queryKey: ['admin-lists'] })
      void queryClient.invalidateQueries({ queryKey: ['lists'] })
    } catch (error) {
      setReport({ text: error instanceof Error ? error.message : t.common.actionFailed, failures: [] })
    } finally {
      setProgress(null)
    }
  }

  const filtered = Boolean(searched || kind || manualOnly || failedOnly || missing.length)
  // Narrowed to what records lack and nothing else: an empty list is good news.
  const onlyGaps = missing.length > 0 && !searched && !kind && !manualOnly && !failedOnly
  // Nothing proposed is not something lacking, and is said in its own words.
  const onlyDuplicates = onlyGaps && missing.length === 1 && missing[0] === 'duplicates'

  // ── The table ──────────────────────────────────────────────────────────
  const columns: Column[] = useMemo(
    () => [
      { id: 'select', label: w.colSelect, kind: 'select', always: true, className: 'w-8 pr-0' },
      { id: 'title', label: w.colTitle, kind: 'primary', always: true, sort: 'asc' },
      { id: 'ids', label: w.colIds, from: 'xl', card: false },
      { id: 'marks', label: w.marks.label },
      { id: 'state', label: w.colState },
      { id: 'updated', label: w.colUpdated, align: 'right', from: 'lg', card: false },
      { id: 'refreshed', label: w.colRefreshed, align: 'right', sort: 'desc', hidden: true },
      { id: 'added', label: w.colAdded, align: 'right', sort: 'desc', hidden: true },
      { id: 'actions', label: w.colActions, kind: 'actions', always: true, align: 'right', className: 'w-12 pl-0' },
    ],
    [w],
  )
  const tableSort: Sort | undefined = sort === 'popularity' ? undefined : { column: sort, direction: order ?? COLUMN_SORTS[sort] }
  const table = useDataTable({
    id: 'catalogue',
    columns,
    sort: tableSort,
    onSort: (next) =>
      address((params) => {
        params.set('sort', next.column)
        if (next.direction === COLUMN_SORTS[next.column as ColumnSort]) params.delete('order')
        else params.set('order', next.direction)
      }),
  })

  // ── The palette ────────────────────────────────────────────────────────
  const [savedViews] = usePreference<SavedView[]>('catalogue.views', [], readViews)
  const manualLists = (curated.data?.lists ?? []).filter((one) => one.mode === 'manual')
  const chosen = (shown ?? []).filter((item) => selected.has(item.id))
  const one = chosen.length ? undefined : currentWork
  const subjects = chosen.length ? chosen : one ? [one] : []
  const commands: Command[] = [
    ...(one
      ? [
          { id: 'open', label: c.editor, glyph: 'pencil' as const, note: one.title, run: () => navigate(`/admin/catalogue/${one.id}`) },
          { id: 'public', label: c.publicPage, glyph: 'external' as const, run: () => navigate(`/work/${one.id}`) },
        ]
      : []),
    ...(subjects.length
      ? [
          {
            id: 'refresh',
            label: c.refreshNow,
            glyph: 'refresh' as const,
            keys: 'r',
            disabled: busy || subjects.every((item) => item.isManual),
            hint: subjects.every((item) => item.isManual) ? w.marks.refresh.manual : undefined,
            run: () => (one ? refresh.mutate(one.id) : void refreshEach(subjects)),
          },
          {
            id: 'lock-overview',
            label: c.lockOverview,
            glyph: 'lock' as const,
            disabled: busy,
            run: () => void lockOverviews(subjects),
          },
          {
            id: 'enable',
            label: c.enable,
            glyph: 'power' as const,
            disabled: busy || subjects.every((item) => item.isEnabled),
            run: () => (one ? setEnabled.mutate({ id: one.id, enabled: true }) : void enableEach(true, subjects)),
          },
          {
            id: 'disable',
            label: c.disable,
            glyph: 'power' as const,
            disabled: busy || subjects.every((item) => !item.isEnabled),
            run: () => (one ? setAsking({ item: one, what: 'disable' }) : void enableEach(false, subjects)),
          },
          {
            id: 'add-to-list',
            label: c.addToList,
            glyph: 'list' as const,
            disabled: busy,
            choose: {
              title: t.palette.listsTitle,
              load: () =>
                manualLists.length
                  ? manualLists.map((list) => ({
                      id: list.id,
                      label: list.name,
                      run: () => void addToList(list.id, subjects.map((item) => item.id)),
                    }))
                  : [{ id: 'none', label: t.palette.noLists, disabled: true }],
            },
          },
        ]
      : []),
    ...(chosen.length
      ? [
          {
            id: 'clear',
            label: c.clearSelection,
            glyph: 'close' as const,
            run: () => {
              setSelected(new Set())
              setReport(null)
            },
          },
        ]
      : []),
    { id: 'search', label: c.search, glyph: 'search', keys: '/', run: () => document.getElementById('works-search')?.focus() },
    { id: 'new', label: c.newEntry, glyph: 'plus', keys: 'n', run: () => setComposing(true) },
    ...(shown?.length && !allShown
      ? [{ id: 'select-all', label: c.selectAll, glyph: 'check' as const, run: () => setSelected(new Set(shown.map((item) => item.id))) }]
      : []),
    ...BUILT_IN.map((offered) => ({
      id: `view-${offered.id}`,
      label: c.view(w.views.builtIn[offered.id]),
      glyph: 'list' as const,
      run: () => chooseView(offered.query),
    })),
    ...savedViews.map((saved) => ({
      id: `saved-${saved.name}`,
      label: c.view(saved.name),
      glyph: 'list' as const,
      run: () => chooseView(saved.query),
    })),
  ]
  useCommands({
    id: 'catalogue',
    heading: chosen.length ? b.selected(chosen.length) : one ? t.palette.heading.row : t.palette.heading.catalogue,
    commands,
  })

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
        <Views current={view} onChoose={chooseView} />

        <div className="mt-4 flex flex-wrap items-end gap-3">
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
            <Select
              id="works-kind"
              value={kind}
              onChange={(event) =>
                address((next) => {
                  if (event.target.value) next.set('kind', event.target.value)
                  else next.delete('kind')
                })
              }
            >
              <option value="">{w.allKinds}</option>
              <option value="series">{t.nav.series}</option>
              <option value="movie">{t.nav.films}</option>
            </Select>
          </div>

          <Button variant={manualOnly ? 'primary' : 'ghost'} aria-pressed={manualOnly} onClick={() => flag('manual', !manualOnly)}>
            <Glyph name="lock" className="size-4" />
            {w.manualOnly}
          </Button>

          <Button variant={failedOnly ? 'danger' : 'ghost'} aria-pressed={failedOnly} onClick={() => flag('refreshFailed', !failedOnly)}>
            <Glyph name="alert" className="size-4" />
            {w.failedOnly}
          </Button>

          <div className="min-w-40">
            <label htmlFor="works-sort" className="label mb-1.5 block">
              {t.browse.sort}
            </label>
            <Select
              id="works-sort"
              // The title's two ways are two choices; the head of its column
              // turns the others round.
              value={sort === 'title' && order === 'desc' ? 'title-desc' : sort}
              onChange={(event) =>
                address((next) => {
                  const chosen = event.target.value
                  if (chosen === 'popularity') next.delete('sort')
                  else next.set('sort', chosen === 'title-desc' ? 'title' : chosen)
                  if (chosen === 'title-desc') next.set('order', 'desc')
                  else next.delete('order')
                })
              }
            >
              <option value="popularity">{t.browse.orders.popular}</option>
              <option value="title">{t.browse.orders.title}</option>
              <option value="title-desc">{t.browse.orders.titleDesc}</option>
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
            <TableOptions table={table} />
          </span>
        </div>

        {/* What records lack, each a filter of its own with how many works it
            leaves: pressed together they narrow, to the works lacking every one. */}
        <div role="group" aria-labelledby="works-missing" className="mt-3 flex flex-wrap items-center gap-2">
          <span id="works-missing" className="label mr-1">
            {w.missing.label}
          </span>
          {GAPS.map((gap) => {
            const on = missing.includes(gap)
            const n = gaps.data?.[gap]
            return (
              <Button key={gap} size="sm" variant={on ? 'primary' : 'ghost'} aria-pressed={on} onClick={() => toggleGap(gap)}>
                <Glyph name={GAP_GLYPH[gap]} className="size-3.5" />
                {w.missing[gap]}
                {n === undefined ? null : (
                  <>
                    <span aria-hidden className={cn('font-mono text-xs tabular-nums', on ? '' : 'text-bone-faint')}>
                      {n}
                    </span>
                    <span className="sr-only">, {w.missing.count(n)}</span>
                  </>
                )}
              </Button>
            )
          })}
        </div>
      </div>

      {rowError ? (
        <p role="alert" className="mb-3 text-sm text-status-error">
          {rowError}
        </p>
      ) : null}
      {mergeSaid ? (
        <p role="status" className="mb-3 flex items-start gap-2 text-sm text-bone-dim">
          <Glyph name="copy" className="mt-0.5 size-4 shrink-0 text-brass" />
          {mergeSaid}
        </p>
      ) : null}

      {/* Clipped rather than hidden: a box that hides what overflows is one the
          table's head would stick to, and it never scrolls. */}
      <Panel className="rise overflow-clip" style={{ animationDelay: '60ms' }}>
        {list.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 8 }, (_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : list.isError ? (
          <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-status-error">
            <Glyph name="alert" className="size-4" />
            {list.error instanceof ApiError && list.error.status === 400 ? list.error.message : w.loadFailed}
          </p>
        ) : list.data.items.length === 0 ? (
          <EmptyState
            title={
              onlyDuplicates ? w.missing.noDuplicates : onlyGaps ? w.missing.none : filtered ? w.emptyFiltered : w.empty
            }
            hint={
              onlyDuplicates
                ? w.missing.noDuplicatesHint
                : onlyGaps
                  ? w.missing.noneHint
                  : filtered
                    ? w.emptyFilteredHint
                    : w.emptyHint
            }
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
                    {manualLists.length ? (
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
                        {manualLists.map((list) => (
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
            <DataTable
              table={table}
              label={t.admin.catalogue}
              onOpen={(id) => navigate(`/admin/catalogue/${id}`)}
              onActive={setCurrent}
            >
              <THead>
                <Th column="select">
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
                <Th column="title" />
                <Th column="ids" />
                <Th column="marks" />
                <Th column="state" />
                <Th column="updated" />
                <Th column="refreshed" />
                <Th column="added" />
                <Th column="actions">
                  <span className="sr-only">{w.colActions}</span>
                </Th>
              </THead>
              <TBody>
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
                    partners={partners.get(item.id) ?? []}
                    onMerge={(partner) =>
                      mergeOn({
                        work: { id: item.id, title: item.title, kind: item.kind },
                        other: partner?.other.id,
                        proposed: Boolean(partner),
                        flagged: partner?.pair.confidence === 'flag',
                      })
                    }
                    selected={selected.has(item.id)}
                    onSelect={(on) => toggle(item.id, on)}
                  />
                ))}
              </TBody>
            </DataTable>
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

      {merging ? (
        <MergeDialog
          key={`merge-${mergings}`}
          open
          work={merging.work}
          other={merging.other}
          proposed={merging.proposed}
          flagged={merging.flagged}
          onClose={() => setMerging(null)}
          onDone={(said) => {
            setMerging(null)
            setMergeSaid(said)
          }}
        />
      ) : null}
    </div>
  )
}

/**
 * The views of the register, a chip each: those it offers, then the
 * person's own, kept with their account — and the way to keep the one shown.
 * The chip of the view shown is pressed; a view saved by a person carries
 * the brass of what people decided.
 */
function Views({ current, onChoose }: { current: string; onChoose: (query: string) => void }) {
  const { t } = useI18n()
  const v = t.admin.works.views
  const [saved, setSaved] = usePreference<SavedView[]>('catalogue.views', [], readViews)

  return (
    <div role="group" aria-labelledby="works-views" className="flex flex-wrap items-center gap-2">
      <span id="works-views" className="label mr-1">
        {v.label}
      </span>
      {BUILT_IN.map((offered) => (
        <ViewChip key={offered.id} pressed={current === offered.query} onClick={() => onChoose(offered.query)}>
          {v.builtIn[offered.id]}
        </ViewChip>
      ))}
      {saved.map((view) => (
        <span key={view.name} className="inline-flex items-center">
          <ViewChip pressed={current === view.query} saved onClick={() => onChoose(view.query)}>
            {view.name}
          </ViewChip>
          <button
            type="button"
            aria-label={v.forget(view.name)}
            title={v.forget(view.name)}
            onClick={() => setSaved(saved.filter((one) => one !== view))}
            className="-ml-1 grid size-11 shrink-0 cursor-pointer place-items-center rounded-full text-bone-faint transition-colors duration-150 hover:text-vermillion"
          >
            <Glyph name="close" className="size-3" />
          </button>
        </span>
      ))}
      <SaveView current={current} saved={saved} onSave={(view) => setSaved([...saved, view])} />
    </div>
  )
}

function ViewChip({
  pressed,
  saved,
  onClick,
  children,
}: {
  pressed: boolean
  saved?: boolean
  onClick: () => void
  children: ReactNode
}) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      onClick={onClick}
      className={cn(
        'inline-flex min-h-11 cursor-pointer items-center gap-1.5 rounded-full border px-3.5 text-body-sm transition-colors duration-150',
        pressed
          ? 'border-vermillion bg-vermillion/[0.08] text-bone'
          : 'border-rule-bright text-bone-dim hover:border-bone-faint hover:text-bone',
        'forced-colors:border-[color:ButtonText]',
        pressed && 'forced-colors:bg-[color:Highlight] forced-colors:text-[color:HighlightText]',
      )}
    >
      {saved ? <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-brass" /> : null}
      {children}
    </button>
  )
}

/** The view shown, kept under a name the person gives it: checked as it is typed, said beside the field. */
function SaveView({
  current,
  saved,
  onSave,
}: {
  current: string
  saved: SavedView[]
  onSave: (view: SavedView) => void
}) {
  const { t } = useI18n()
  const v = t.admin.works.views
  const [name, setName] = useState('')
  const [tried, setTried] = useState(false)
  const problem = refusal(name, current, saved)
  const said = !problem
    ? undefined
    : problem.why === 'same'
      ? v.invalid.same(problem.saved ?? (problem.builtIn ? v.builtIn[problem.builtIn] : ''))
      : problem.why === 'long'
        ? v.invalid.long(VIEW_NAME)
        : problem.why === 'many'
          ? v.invalid.many(VIEWS_MOST)
          : v.invalid[problem.why]

  return (
    <Menu
      label={v.saveTitle}
      title={v.saveTitle}
      align="start"
      button={
        <>
          <Glyph name="plus" className="size-3.5" />
          {v.save}
        </>
      }
      buttonClassName={(open) =>
        cn(
          'inline-flex min-h-11 cursor-pointer items-center gap-1.5 rounded-full border border-dashed border-rule-bright px-3.5 text-body-sm text-bone-dim',
          'transition-colors duration-150 hover:border-bone-faint hover:text-bone',
          open && 'border-bone-faint text-bone',
        )
      }
      className="w-80 p-4"
    >
      {(close) => (
        <form
          noValidate
          className="space-y-3"
          onSubmit={(event) => {
            event.preventDefault()
            setTried(true)
            if (problem) return
            onSave({ name: name.trim(), query: current })
            setName('')
            setTried(false)
            close()
          }}
        >
          <FormField label={v.name} htmlFor="view-name" hint={v.nameHint} error={tried || problem?.why === 'same' ? said : undefined}>
            <Input
              id="view-name"
              autoFocus
              autoComplete="off"
              maxLength={VIEW_NAME + 10}
              value={name}
              aria-invalid={tried && problem ? true : undefined}
              onChange={(event) => setName(event.target.value)}
            />
          </FormField>
          <p className="text-xs leading-relaxed text-bone-faint">{v.kept}</p>
          <Button type="submit" size="sm" variant="primary" disabled={problem?.why === 'same'}>
            {v.submit}
          </Button>
        </form>
      )}
    </Menu>
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
  partners,
  onMerge,
  selected,
  onSelect,
}: {
  item: MediaItem
  locale: string
  busy: boolean
  onRefresh: () => void
  onEnable: () => void
  onAsk: (what: Asking['what']) => void
  /** The pairs the server proposes this work in, while the register is narrowed to them. */
  partners: Partner[]
  /** Open the merge dialog: on a pair, or on the work alone. */
  onMerge: (partner?: Partner) => void
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
    <Tr row={item.id} data-work={item.id} className={cn('group', item.isEnabled ? '' : 'opacity-60', selected && 'bg-ink-high')}>
      {/* Above the stretched link, or the box would open the editor. */}
      <Td column="select" className="w-8 pr-0">
        <input
          type="checkbox"
          aria-label={w.bulk.select(item.title)}
          className="relative z-10 size-4 accent-vermillion"
          checked={selected}
          onChange={(event) => onSelect(event.target.checked)}
        />
      </Td>
      <Td column="title" className="w-full max-w-0 min-w-40 py-1.5">
        {/* The cell is the target: the pseudo-element grows the link to the
            height of the row, so a thumb landing anywhere in the column opens
            the editor while the link stays a link for the keyboard. */}
        <Link
          to={`/admin/catalogue/${item.id}`}
          // The year and the kind with the title, as the row shows them — the
          // kind is its glyph's title: whoever speaks to the page says what
          // they see (WCAG 2.5.3).
          aria-label={[w.open(item.title), item.year, item.kind === 'series' ? t.nav.series : t.nav.films].filter(Boolean).join(' ')}
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
            {/* Words apart, as the name has them; the row lays no space. */}{' '}
            {item.year ? (
              <span className="shrink-0 font-mono text-xs text-bone-faint tabular-nums">{item.year}</span>
            ) : null}{' '}
            <Glyph
              name={item.kind === 'series' ? 'tv' : 'film'}
              className="size-3.5 shrink-0 text-bone-faint"
              title={item.kind === 'series' ? t.nav.series : t.nav.films}
            />
          </span>
        </Link>
      </Td>

      {/* From 1280 px only: on one line, the identifiers took the title's
          room beside the sidebar at 1024 px. They are in the editor. */}
      <Td column="ids">
        <span className="flex gap-x-3 font-mono text-label whitespace-nowrap text-bone-faint tabular-nums">
          {ids.tvdb ? <span>tvdb:{ids.tvdb}</span> : null}
          {ids.tmdb ? <span>tmdb:{ids.tmdb}</span> : null}
          {ids.imdb ? <span>{ids.imdb}</span> : null}
        </span>
      </Td>

      <Td column="marks">
        <Marks item={item} locale={locale} />
      </Td>

      <Td column="state">
        <span className="flex flex-wrap items-center gap-1.5">
          <Twins partners={partners} onMerge={onMerge} />
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
          {item.isEnabled && !item.isManual && locks === 0 && partners.length === 0 ? (
            <span className="text-xs whitespace-nowrap text-bone-faint">{w.served}</span>
          ) : null}
        </span>
      </Td>

      <Td column="updated" align="right" className="whitespace-nowrap">
        <span className="font-mono text-xs text-bone-faint tabular-nums" title={fmt.dateTime(item.updatedAt, locale)}>
          {fmt.relative(item.updatedAt, locale)}
        </span>
      </Td>

      <Td column="refreshed" align="right" className="whitespace-nowrap">
        <span className="font-mono text-xs text-bone-faint tabular-nums" title={fmt.dateTime(item.refreshedAt, locale)}>
          {fmt.relative(item.refreshedAt, locale) ?? '—'}
        </span>
      </Td>

      <Td column="added" align="right" className="whitespace-nowrap">
        <span className="font-mono text-xs text-bone-faint tabular-nums" title={fmt.dateTime(item.createdAt, locale)}>
          {fmt.relative(item.createdAt, locale)}
        </span>
      </Td>

      <Td column="actions" align="right" className="w-12 py-0 pl-0">
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
              <MenuButton
                glyph="copy"
                onClick={() => {
                  close()
                  onMerge(partners[0])
                }}
              >
                {w.menu.merge}
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
 * The works one is proposed to be with, each a mark that opens the two side
 * by side: the other's title on it, what makes them look alike to the pointer
 * that rests on it. Only the title, kept short: a sentence around it made the
 * column wide enough to squeeze the titles of the register out of their own.
 * Above the stretched link, which would otherwise open the editor.
 */
function Twins({ partners, onMerge }: { partners: Partner[]; onMerge: (partner: Partner) => void }) {
  const { t } = useI18n()
  const d = t.admin.works.duplicate

  return partners.map((partner) => (
    <button
      key={partner.other.id}
      type="button"
      title={d.confidence[partner.pair.confidence]}
      aria-label={d.open(partner.other.title)}
      onClick={() => onMerge(partner)}
      className="relative z-10 cursor-pointer rounded-full transition-opacity duration-150 hover:opacity-80"
    >
      <Chip tone={partner.pair.confidence === 'certain' ? 'accent' : 'manual'}>
        <Glyph name="copy" className="size-3" />
        <span className="max-w-36 truncate normal-case">{partner.other.title}</span>
      </Chip>
    </button>
  ))
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
  const marks: { gap: Mark; state: 'has' | 'lacks' | 'none'; says: string }[] = [
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
  'flex min-h-11 w-full cursor-pointer items-center gap-2.5 rounded-md px-3 text-left text-sm transition-colors duration-150 ' +
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
        <p role="alert" className="text-sm text-status-error">
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
          <p role="alert" className="flex items-center gap-2 text-sm text-status-error sm:col-span-2">
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
