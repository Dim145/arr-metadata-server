/**
 * The curated lists, composed: a selection shown on the site and served to
 * Sonarr and Radarr as the custom lists their import lists read — by hand,
 * in an order, or by a filter kept current.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Link } from 'react-router'

import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  IconButton,
  Input,
  Label,
  Panel,
  PanelHead,
  Select,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Textarea,
  Th,
  Toggle,
  Tr,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import { useSettled } from '../../lib/debounce'
import { useTitle } from '../../lib/hooks'
import { useI18n } from '../../lib/i18n'
import type {
  CuratedList,
  CuratedListPage,
  CuratedListRequest,
  CuratedLists,
  ItemPage,
  ListFilter,
  ListKind,
  ListMode,
  MediaItem,
} from '../../lib/types'
import { ImportAddresses } from '../Lists'

const KINDS: ListKind[] = ['mixed', 'series', 'movie']
const SORTS = ['popularity', 'rating', 'release', 'title', 'added'] as const

export function Lists() {
  const { t } = useI18n()
  useTitle(t.admin.lists)
  const queryClient = useQueryClient()

  const lists = useQuery({
    queryKey: ['admin-lists'],
    queryFn: () => api.get<CuratedLists>('/lists'),
  })

  const [name, setName] = useState('')
  const [kind, setKind] = useState<ListKind>('mixed')
  const [mode, setMode] = useState<ListMode>('manual')
  const [editing, setEditing] = useState<string | null>(null)
  const [asking, setAsking] = useState<CuratedList | null>(null)

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['admin-lists'] })
    void queryClient.invalidateQueries({ queryKey: ['lists'] })
    void queryClient.invalidateQueries({ queryKey: ['list'] })
    void queryClient.invalidateQueries({ queryKey: ['work-lists'] })
  }

  const create = useMutation({
    mutationFn: () =>
      api.post<CuratedList>('/lists', {
        name,
        kind,
        mode,
        isPublic: true,
        filter: mode === 'filter' ? {} : undefined,
      } satisfies CuratedListRequest),
    onSuccess: (list) => {
      setName('')
      invalidate()
      setEditing(list.id)
    },
  })

  const remove = useMutation({
    mutationFn: (id: string) => api.delete(`/lists/${id}`),
    onSuccess: () => {
      setAsking(null)
      setEditing((current) => (current === asking?.id ? null : current))
      invalidate()
    },
  })

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.lists}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.admin.curated.lead}</p>
      </header>

      <Panel className="rise mb-6" style={{ animationDelay: '60ms' }}>
        <PanelHead title={t.admin.curated.new} />
        <form
          className="flex flex-wrap items-start gap-5 p-5"
          onSubmit={(event) => {
            event.preventDefault()
            create.mutate()
          }}
        >
          <div className="min-w-48 flex-1">
            <FormField
              label={t.admin.curated.name}
              htmlFor="list-name"
              error={create.isError ? create.error.message : undefined}
            >
              <Input id="list-name" required value={name} onChange={(event) => setName(event.target.value)} />
            </FormField>
          </div>
          <div className="min-w-36">
            <FormField label={t.admin.curated.kind} htmlFor="list-kind">
              <Select id="list-kind" value={kind} onChange={(event) => setKind(event.target.value as ListKind)}>
                {KINDS.map((option) => (
                  <option key={option} value={option}>
                    {t.lists.kind[option]}
                  </option>
                ))}
              </Select>
            </FormField>
          </div>
          <div className="min-w-36">
            <FormField label={t.admin.curated.mode} htmlFor="list-mode">
              <Select id="list-mode" value={mode} onChange={(event) => setMode(event.target.value as ListMode)}>
                <option value="manual">{t.admin.curated.modeManual}</option>
                <option value="filter">{t.admin.curated.modeFilter}</option>
              </Select>
            </FormField>
          </div>
          <Button type="submit" variant="primary" className="sm:mt-6" disabled={create.isPending || !name.trim()}>
            {create.isPending ? <Spinner className="size-4" /> : <Glyph name="list" className="size-4" />}
            {t.admin.curated.create}
          </Button>
        </form>
      </Panel>

      <Panel className="rise mb-6 overflow-hidden" style={{ animationDelay: '100ms' }}>
        <PanelHead title={t.admin.lists} />
        {lists.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 3 }, (_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : lists.isError ? (
          <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {t.admin.curated.loadFailed}
          </p>
        ) : lists.data.lists.length === 0 ? (
          <EmptyState title={t.admin.curated.empty} hint={t.admin.curated.emptyHint} />
        ) : (
          <TableScroll label={t.admin.lists}>
            <table className="w-full min-w-[20rem] border-collapse text-left">
              <thead>
                <tr>
                  <Th>{t.admin.curated.colName}</Th>
                  <Th className="hidden sm:table-cell">{t.admin.curated.colKind}</Th>
                  <Th className="hidden md:table-cell">{t.admin.curated.colMode}</Th>
                  <Th align="right" className="hidden sm:table-cell">
                    {t.admin.curated.colCount}
                  </Th>
                  <Th align="right">{t.admin.curated.colActions}</Th>
                </tr>
              </thead>
              <tbody>
                {lists.data.lists.map((list) => (
                  <Tr key={list.id} className={editing === list.id ? 'bg-ink-high' : ''}>
                    <Td className="w-full max-w-0">
                      <span className="flex items-center gap-2">
                        <button
                          type="button"
                          onClick={() => setEditing(list.id)}
                          className="truncate text-left text-sm font-medium text-bone underline-offset-4 hover:underline"
                        >
                          {list.name}
                        </button>
                        {list.isPublic ? null : <Chip tone="accent">{t.admin.curated.private}</Chip>}
                      </span>
                      <span className="mt-0.5 block truncate font-mono text-[0.6875rem] text-bone-faint">
                        /lists/{list.slug}
                      </span>
                    </Td>
                    <Td className="hidden sm:table-cell">{t.lists.kind[list.kind]}</Td>
                    <Td className="hidden md:table-cell">
                      {list.mode === 'filter' ? t.admin.curated.modeFilter : t.admin.curated.modeManual}
                    </Td>
                    <Td align="right" className="hidden font-mono tabular-nums sm:table-cell">
                      {list.mode === 'filter' ? '—' : list.itemCount}
                    </Td>
                    <Td align="right">
                      <span className="flex items-center justify-end gap-1">
                        <IconButton
                          glyph="pencil"
                          label={`${t.admin.curated.edit}: ${list.name}`}
                          onClick={() => setEditing(list.id)}
                        />
                        <Link
                          to={`/lists/${list.slug}`}
                          aria-label={t.admin.curated.open}
                          title={t.admin.curated.open}
                          className="inline-flex size-11 items-center justify-center rounded-full text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
                        >
                          <Glyph name="link" className="size-4" />
                        </Link>
                        <IconButton
                          glyph="trash"
                          label={`${t.admin.curated.delete}: ${list.name}`}
                          tone="danger"
                          onClick={() => setAsking(list)}
                        />
                      </span>
                    </Td>
                  </Tr>
                ))}
              </tbody>
            </table>
          </TableScroll>
        )}
      </Panel>

      {editing ? (
        <Editor key={editing} id={editing} onClose={() => setEditing(null)} onSaved={invalidate} />
      ) : null}

      <Dialog
        open={asking !== null}
        title={t.admin.curated.deleteTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button variant="danger" disabled={remove.isPending} onClick={() => asking && remove.mutate(asking.id)}>
              <Glyph name="trash" className="size-4" />
              {t.admin.curated.delete}
            </Button>
          </>
        }
      >
        {asking ? t.admin.curated.deleteBody(asking.name) : null}
        {remove.isError ? (
          <p role="alert" className="mt-3 text-sm text-vermillion">
            {remove.error.message}
          </p>
        ) : null}
      </Dialog>
    </div>
  )
}

/** The filter as a form holds it: text in every field, parsed on save. */
interface FilterForm {
  term: string
  genres: string
  keyword: string
  yearFrom: string
  yearTo: string
  minRating: string
  status: string
  originalLanguage: string
  network: string
  sort: string
  order: '' | 'asc' | 'desc'
  limit: string
  /** Kept as it was: the form has no field for it. */
  collection?: number
}

interface Form {
  name: string
  description: string
  kind: ListKind
  mode: ListMode
  isPublic: boolean
  filter: FilterForm
}

function toForm(list: CuratedList): Form {
  const f = list.filter ?? {}
  return {
    name: list.name,
    description: list.description ?? '',
    kind: list.kind,
    mode: list.mode,
    isPublic: list.isPublic,
    filter: {
      term: f.term ?? '',
      genres: (f.genres ?? []).join(', '),
      keyword: f.keyword ?? '',
      yearFrom: f.yearFrom?.toString() ?? '',
      yearTo: f.yearTo?.toString() ?? '',
      minRating: f.minRating?.toString() ?? '',
      status: f.status ?? '',
      originalLanguage: f.originalLanguage ?? '',
      network: f.network ?? '',
      sort: f.sort ?? '',
      // Absent and null are the same thing here: the sort's own direction.
      order: f.descending == null ? '' : f.descending ? 'desc' : 'asc',
      limit: f.limit?.toString() ?? '',
      collection: f.collection,
    },
  }
}

function toFilter(form: FilterForm): ListFilter {
  const text = (value: string) => (value.trim() ? value.trim() : undefined)
  const int = (value: string) => (value.trim() ? Number.parseInt(value, 10) : undefined)
  const num = (value: string) => (value.trim() ? Number.parseFloat(value) : undefined)
  return {
    term: text(form.term),
    genres: form.genres
      .split(',')
      .map((g) => g.trim())
      .filter(Boolean),
    keyword: text(form.keyword),
    yearFrom: int(form.yearFrom),
    yearTo: int(form.yearTo),
    minRating: num(form.minRating),
    status: text(form.status),
    originalLanguage: text(form.originalLanguage),
    network: text(form.network),
    sort: text(form.sort),
    descending: form.order === '' ? undefined : form.order === 'desc',
    limit: int(form.limit),
    collection: form.collection,
  }
}

function Editor({ id, onClose, onSaved }: { id: string; onClose: () => void; onSaved: () => void }) {
  const { t, lang } = useI18n()
  const queryClient = useQueryClient()

  const detail = useQuery({
    queryKey: ['admin-list', id, lang],
    queryFn: () => api.get<CuratedListPage>(`/lists/${id}${query({ language: lang })}`),
  })

  const [form, setForm] = useState<Form | null>(null)
  const [members, setMembers] = useState<MediaItem[]>([])
  // Whether the members were changed here: only then are they sent, so a
  // save never rewrites a membership it did not touch.
  const [touched, setTouched] = useState(false)
  const [saved, setSaved] = useState(false)
  useEffect(() => {
    if (detail.data) {
      // The form is filled once, from the list as it was loaded; what is
      // typed after is not overwritten by a refetch.
      setForm((current) => current ?? toForm(detail.data.list))
      if (!touched) setMembers(detail.data.items)
    }
  }, [detail.data, touched])

  const save = useMutation({
    mutationFn: () => {
      if (!form) throw new Error('nothing to save')
      const body: CuratedListRequest = {
        name: form.name,
        description: form.description.trim() || undefined,
        kind: form.kind,
        mode: form.mode,
        isPublic: form.isPublic,
        filter: form.mode === 'filter' ? toFilter(form.filter) : undefined,
        items: form.mode === 'manual' && touched ? members.map((m) => m.id) : undefined,
      }
      return api.put<CuratedList>(`/lists/${id}`, body)
    },
    onSuccess: () => {
      setSaved(true)
      window.setTimeout(() => setSaved(false), 2500)
      onSaved()
      void queryClient.invalidateQueries({ queryKey: ['admin-list', id] })
    },
  })

  // Adding a member: the catalogue searched as it is typed, narrowed to the
  // kind the list holds.
  const [term, setTerm] = useState('')
  const settled = useSettled(term)
  const results = useQuery({
    queryKey: ['list-search', settled, form?.kind],
    enabled: settled.trim().length > 0,
    queryFn: () =>
      api.get<ItemPage>(
        `/items${query({ term: settled, kind: form?.kind === 'mixed' ? undefined : form?.kind, limit: 8 })}`,
      ),
  })

  const move = (index: number, by: number) => {
    setTouched(true)
    setMembers((current) => {
      const next = [...current]
      const target = index + by
      if (target < 0 || target >= next.length) return current
      const [moved] = next.splice(index, 1)
      if (moved) next.splice(target, 0, moved)
      return next
    })
  }

  if (detail.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {t.admin.curated.loadFailed}
      </p>
    )
  }
  if (!form || !detail.data) {
    return <Skeleton className="h-64 w-full" />
  }
  const list = detail.data.list
  const set = (patch: Partial<Form>) => setForm({ ...form, ...patch })
  const setFilter = (patch: Partial<FilterForm>) => setForm({ ...form, filter: { ...form.filter, ...patch } })

  return (
    <Panel className="rise mb-6" label={list.name} id="list-editor">
      <PanelHead
        title={list.name}
        action={
          <Button size="sm" onClick={onClose}>
            <Glyph name="close" className="size-3.5" />
            {t.common.close}
          </Button>
        }
      />
      <form
        className="space-y-6 p-5"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate()
        }}
      >
        <div className="grid gap-5 sm:grid-cols-2">
          <FormField label={t.admin.curated.name} htmlFor="edit-name">
            <Input
              id="edit-name"
              required
              autoFocus
              value={form.name}
              onChange={(event) => set({ name: event.target.value })}
            />
          </FormField>
          <FormField label={t.admin.curated.kind} htmlFor="edit-kind">
            <Select id="edit-kind" value={form.kind} onChange={(event) => set({ kind: event.target.value as ListKind })}>
              {KINDS.map((option) => (
                <option key={option} value={option}>
                  {t.lists.kind[option]}
                </option>
              ))}
            </Select>
          </FormField>
          <div className="sm:col-span-2">
            <FormField label={t.admin.curated.description} htmlFor="edit-description">
              <Textarea
                id="edit-description"
                rows={3}
                value={form.description}
                onChange={(event) => set({ description: event.target.value })}
              />
            </FormField>
          </div>
          <FormField label={t.admin.curated.mode} htmlFor="edit-mode">
            <Select id="edit-mode" value={form.mode} onChange={(event) => set({ mode: event.target.value as ListMode })}>
              <option value="manual">{t.admin.curated.modeManual}</option>
              <option value="filter">{t.admin.curated.modeFilter}</option>
            </Select>
          </FormField>
          <div>
            <span className="label mb-1.5 block">{t.admin.curated.visibility}</span>
            <div className="flex min-h-11 items-center gap-3">
              <Toggle
                id="edit-public"
                checked={form.isPublic}
                label={t.admin.curated.visibility}
                onChange={(next) => set({ isPublic: next })}
              />
              <span className="text-sm text-bone-dim">
                {form.isPublic ? t.admin.curated.public : t.admin.curated.private}
              </span>
            </div>
          </div>
        </div>

        {form.mode === 'manual' ? (
          <fieldset className="space-y-3">
            <legend className="label">{t.admin.curated.members}</legend>
            <p className="text-sm text-bone-dim">{t.admin.curated.membersHint}</p>
            <div className="relative">
              <Input
                aria-label={t.admin.curated.search}
                placeholder={t.admin.curated.search}
                value={term}
                onChange={(event) => setTerm(event.target.value)}
                // Enter here picks nothing; it must not save the whole list.
                onKeyDown={(event) => {
                  if (event.key === 'Enter') event.preventDefault()
                }}
              />
              {settled.trim() && results.data ? (
                <ul className="mt-2 divide-y divide-rule rounded-card border border-rule bg-ink">
                  {results.data.items.length === 0 ? (
                    <li className="px-3 py-2 text-sm text-bone-faint">{t.admin.curated.noResults}</li>
                  ) : (
                    results.data.items.map((item) => {
                      const present = members.some((m) => m.id === item.id)
                      return (
                        <li key={item.id} className="flex items-center gap-3 px-3 py-1.5">
                          <span className="min-w-0 flex-1 truncate text-sm text-bone">
                            {item.title}
                            {item.year ? <span className="text-bone-faint"> · {item.year}</span> : null}
                          </span>
                          <Button
                            size="sm"
                            disabled={present}
                            onClick={() => {
                              setTouched(true)
                              setMembers((current) => [...current, item])
                              setTerm('')
                            }}
                          >
                            {t.admin.curated.add}
                          </Button>
                        </li>
                      )
                    })
                  )}
                </ul>
              ) : null}
            </div>
            {members.length === 0 ? (
              <p className="text-sm text-bone-faint">{t.admin.curated.noMembers}</p>
            ) : (
              <ol className="divide-y divide-rule rounded-card border border-rule">
                {members.map((item, index) => (
                  <li key={item.id} className="flex items-center gap-2 px-3 py-1.5">
                    <span className="w-6 font-mono text-xs text-bone-faint tabular-nums">{index + 1}</span>
                    <span className="flex min-w-0 flex-1 items-center gap-2">
                      <span className="min-w-0 truncate text-sm text-bone">
                        {item.title}
                        {item.year ? <span className="text-bone-faint"> · {item.year}</span> : null}
                      </span>
                      {/* What a reader would not be shown: said here, so it
                          can be taken out rather than silently left. */}
                      {item.isEnabled ? null : <Chip tone="accent">{t.admin.works.disabled}</Chip>}
                      {form.kind !== 'mixed' && item.kind !== form.kind ? (
                        <Chip tone="accent">{item.kind === 'series' ? t.lists.kind.series : t.lists.kind.movie}</Chip>
                      ) : null}
                    </span>
                    <IconButton
                      glyph="chevronUp"
                      label={`${t.admin.curated.moveUp}: ${item.title}`}
                      disabled={index === 0}
                      onClick={() => move(index, -1)}
                    />
                    <IconButton
                      glyph="chevronDown"
                      label={`${t.admin.curated.moveDown}: ${item.title}`}
                      disabled={index === members.length - 1}
                      onClick={() => move(index, 1)}
                    />
                    <IconButton
                      glyph="close"
                      label={`${t.admin.curated.remove}: ${item.title}`}
                      tone="danger"
                      onClick={() => {
                        setTouched(true)
                        setMembers((current) => current.filter((m) => m.id !== item.id))
                      }}
                    />
                  </li>
                ))}
              </ol>
            )}
          </fieldset>
        ) : (
          <fieldset className="space-y-4">
            <legend className="label">{t.admin.curated.filter}</legend>
            <p className="text-sm text-bone-dim">
              {t.admin.curated.filterHint} {t.admin.curated.preview(detail.data.total)}
            </p>
            <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
              <div className="sm:col-span-2 lg:col-span-3">
                <FormField label={t.admin.curated.term} htmlFor="filter-term">
                  <Input id="filter-term" value={form.filter.term} onChange={(e) => setFilter({ term: e.target.value })} />
                </FormField>
              </div>
              <div className="sm:col-span-2 lg:col-span-3">
                <FormField label={t.admin.curated.genres} hint={t.admin.curated.genresHint} htmlFor="filter-genres">
                  <Input id="filter-genres" value={form.filter.genres} onChange={(e) => setFilter({ genres: e.target.value })} />
                </FormField>
              </div>
              <FormField label={t.admin.curated.keyword} htmlFor="filter-keyword">
                <Input id="filter-keyword" value={form.filter.keyword} onChange={(e) => setFilter({ keyword: e.target.value })} />
              </FormField>
              <FormField label={t.admin.curated.yearFrom} htmlFor="filter-year-from">
                <Input id="filter-year-from" type="number" inputMode="numeric" value={form.filter.yearFrom} onChange={(e) => setFilter({ yearFrom: e.target.value })} />
              </FormField>
              <FormField label={t.admin.curated.yearTo} htmlFor="filter-year-to">
                <Input id="filter-year-to" type="number" inputMode="numeric" value={form.filter.yearTo} onChange={(e) => setFilter({ yearTo: e.target.value })} />
              </FormField>
              <FormField label={t.admin.curated.minRating} htmlFor="filter-rating">
                <Input id="filter-rating" type="number" step="0.1" min="0" max="10" value={form.filter.minRating} onChange={(e) => setFilter({ minRating: e.target.value })} />
              </FormField>
              <FormField label={t.admin.curated.status} htmlFor="filter-status">
                <Input id="filter-status" value={form.filter.status} onChange={(e) => setFilter({ status: e.target.value })} />
              </FormField>
              <FormField label={t.admin.curated.language} htmlFor="filter-language">
                <Input id="filter-language" value={form.filter.originalLanguage} onChange={(e) => setFilter({ originalLanguage: e.target.value })} />
              </FormField>
              <FormField label={t.admin.curated.network} htmlFor="filter-network">
                <Input id="filter-network" value={form.filter.network} onChange={(e) => setFilter({ network: e.target.value })} />
              </FormField>
              <FormField label={t.admin.curated.sort} htmlFor="filter-sort">
                <Select id="filter-sort" value={form.filter.sort} onChange={(e) => setFilter({ sort: e.target.value })}>
                  {SORTS.map((sort) => (
                    <option key={sort} value={sort === 'popularity' ? '' : sort}>
                      {sort === 'popularity'
                        ? t.admin.curated.sortPopularity
                        : sort === 'rating'
                          ? t.admin.curated.sortRating
                          : sort === 'release'
                            ? t.admin.curated.sortRelease
                            : sort === 'title'
                              ? t.admin.curated.sortTitle
                              : t.admin.curated.sortAdded}
                    </option>
                  ))}
                </Select>
              </FormField>
              <FormField label={t.admin.curated.order} htmlFor="filter-order">
                <Select
                  id="filter-order"
                  value={form.filter.order}
                  onChange={(e) => setFilter({ order: e.target.value as FilterForm['order'] })}
                >
                  <option value="">{t.admin.curated.orderDefault}</option>
                  <option value="desc">{t.admin.curated.orderDesc}</option>
                  <option value="asc">{t.admin.curated.orderAsc}</option>
                </Select>
              </FormField>
              <FormField label={t.admin.curated.limit} htmlFor="filter-limit">
                <Input id="filter-limit" type="number" min="1" max="500" value={form.filter.limit} onChange={(e) => setFilter({ limit: e.target.value })} />
              </FormField>
            </div>
          </fieldset>
        )}

        <div className="flex flex-wrap items-center gap-3">
          <Button type="submit" variant="primary" disabled={save.isPending || !form.name.trim()}>
            {save.isPending ? <Spinner className="size-4" /> : null}
            {t.admin.curated.save}
          </Button>
          {saved ? (
            <span role="status" className="text-sm text-moss">
              {t.admin.curated.saved}
            </span>
          ) : null}
          {save.isError ? (
            <span role="alert" className="text-sm text-vermillion">
              {t.admin.curated.saveFailed} {save.error.message}
            </span>
          ) : null}
        </div>
      </form>

      <div className="border-t border-rule p-5">
        <ImportAddresses list={list} />
      </div>
    </Panel>
  )
}
