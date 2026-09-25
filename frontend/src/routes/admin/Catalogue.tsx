/**
 * Everything this server holds, as a register.
 *
 * A fact table rather than a grid of posters: the questions asked here are
 * "which of these has somebody edited", "what is disabled" and "when was this
 * last refreshed", and artwork answers none of them. The public side is where
 * the posters belong.
 *
 * Below `md` the per-row controls stand down and the row itself opens the
 * editor, where the same three actions live. Four 44px targets and a title do
 * not fit across a phone, and a table that must be dragged sideways to reach
 * its delete button is worse than one tap more.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useSearchParams } from 'react-router'

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
  Select,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Textarea,
  Th,
  Tr,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useSettled } from '../../lib/debounce'
import { useI18n } from '../../lib/i18n'
import type { ItemPage, MediaItem, MediaKind } from '../../lib/types'

/** What an operator is being asked to confirm, and about which entry. */
type Asking = { item: MediaItem; what: 'delete' | 'disable' }

export function Catalogue() {
  const { t, locale } = useI18n()
  const queryClient = useQueryClient()

  const [params, setParams] = useSearchParams()
  const [term, setTerm] = useState('')
  const [kind, setKind] = useState<'' | MediaKind>('')
  const [manualOnly, setManualOnly] = useState(false)
  // In the address, where the dashboard's count of failed refreshes links:
  // read from it only, a toggle switched off came back on at the next reload.
  const failedOnly = params.get('refreshFailed') === '1'
  const setFailedOnly = (on: boolean) => {
    const next = new URLSearchParams(window.location.search)
    if (on) {
      next.set('refreshFailed', '1')
    } else {
      next.delete('refreshFailed')
    }
    setParams(next, { replace: true })
  }
  const [sort, setSort] = useState<'popularity' | 'title' | 'added' | 'refreshed'>('popularity')
  const [composing, setComposing] = useState(false)
  const [asking, setAsking] = useState<Asking | null>(null)

  const settled = useSettled(term)

  const list = useQuery({
    queryKey: ['items', settled, kind, manualOnly, failedOnly, sort],
    queryFn: () =>
      api.get<ItemPage>(
        `/items${query({
          term: settled,
          kind,
          manualOnly,
          refreshFailed: failedOnly,
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

  const filtered = Boolean(term || kind || manualOnly || failedOnly)

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8 flex flex-wrap items-end justify-between gap-4">
        <div className="min-w-0">
          <Label>{t.admin.title}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
            {t.admin.catalogue}
          </h1>
          <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
            {t.admin.works.lead}
          </p>
        </div>

        {/* The way in is the page's one primary action; the way back out of
            the form is not. */}
        <Button variant={composing ? 'ghost' : 'primary'} onClick={() => setComposing((open) => !open)}>
          <Glyph name={composing ? 'close' : 'plus'} className="size-4" />
          {composing ? t.common.cancel : t.admin.works.newEntry}
        </Button>
      </header>

      {composing ? <Composer onDone={() => setComposing(false)} /> : null}

      <div className="rise mb-6 flex flex-wrap items-end gap-3 border-y border-rule py-4">
        <div className="min-w-44 flex-1">
          <label htmlFor="works-search" className="label mb-1.5 block">
            {t.admin.works.search}
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
            {t.admin.works.kind}
          </label>
          <Select
            id="works-kind"
            value={kind}
            onChange={(event) => setKind(event.target.value as '' | MediaKind)}
          >
            <option value="">{t.admin.works.allKinds}</option>
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
          {t.admin.works.manualOnly}
        </Button>

        <Button
          variant={failedOnly ? 'danger' : 'ghost'}
          aria-pressed={failedOnly}
          onClick={() => setFailedOnly(!failedOnly)}
        >
          <Glyph name="alert" className="size-4" />
          {t.admin.works.failedOnly}
        </Button>

        <div className="min-w-40">
          <label htmlFor="works-sort" className="label mb-1.5 block">
            {t.browse.sort}
          </label>
          <Select
            id="works-sort"
            value={sort}
            onChange={(event) => setSort(event.target.value as typeof sort)}
          >
            <option value="popularity">{t.browse.orders.popular}</option>
            <option value="title">{t.browse.orders.title}</option>
            <option value="added">{t.browse.orders.added}</option>
            <option value="refreshed">{t.admin.works.sortRefreshed}</option>
          </Select>
        </div>

        <span className="ml-auto flex items-center gap-3 self-center">
          {list.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
          {list.data ? (
            <span className="font-mono text-xs text-bone-faint tabular-nums">
              {t.admin.works.showing(list.data.items.length, list.data.total)}
            </span>
          ) : null}
        </span>
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
            {t.admin.works.loadFailed}
          </p>
        ) : list.data.items.length === 0 ? (
          <EmptyState
            title={filtered ? t.admin.works.emptyFiltered : t.admin.works.empty}
            hint={filtered ? t.admin.works.emptyFilteredHint : t.admin.works.emptyHint}
          />
        ) : (
          <TableScroll label={t.admin.catalogue}>
            <table className="w-full min-w-[20rem] border-collapse text-left">
              <thead>
                <tr>
                  <Th>{t.admin.works.colTitle}</Th>
                  <Th className="hidden lg:table-cell">{t.admin.works.colIds}</Th>
                  <Th className="hidden sm:table-cell">{t.admin.works.colState}</Th>
                  <Th align="right" className="hidden md:table-cell">
                    {t.admin.works.colUpdated}
                  </Th>
                  <Th align="right" className="hidden md:table-cell">
                    {t.admin.works.colActions}
                  </Th>
                </tr>
              </thead>
              <tbody>
                {list.data.items.map((item) => (
                  <Row
                    key={item.id}
                    item={item}
                    locale={locale}
                    refreshing={refresh.isPending && refresh.variables === item.id}
                    toggling={setEnabled.isPending && setEnabled.variables.id === item.id}
                    onRefresh={() => refresh.mutate(item.id)}
                    onEnable={() => setEnabled.mutate({ id: item.id, enabled: true })}
                    onAsk={(what) => setAsking({ item, what })}
                  />
                ))}
              </tbody>
            </table>
          </TableScroll>
        )}
      </Panel>

      <Dialog
        open={asking?.what === 'delete'}
        title={t.admin.works.deleteTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={remove.isPending}
              onClick={() => asking && remove.mutate(asking.item.id)}
            >
              <Glyph name="trash" className="size-4" />
              {t.common.delete}
            </Button>
          </>
        }
      >
        {asking ? t.admin.works.deleteBody(asking.item.title) : null}
      </Dialog>

      <Dialog
        open={asking?.what === 'disable'}
        title={t.admin.works.disableTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={setEnabled.isPending}
              onClick={() => asking && setEnabled.mutate({ id: asking.item.id, enabled: false })}
            >
              {t.admin.works.disable}
            </Button>
          </>
        }
      >
        {asking ? t.admin.works.disableBody(asking.item.title) : null}
      </Dialog>
    </div>
  )
}

function Row({
  item,
  locale,
  refreshing,
  toggling,
  onRefresh,
  onEnable,
  onAsk,
}: {
  item: MediaItem
  locale: string
  refreshing: boolean
  toggling: boolean
  onRefresh: () => void
  onEnable: () => void
  onAsk: (what: Asking['what']) => void
}) {
  const { t } = useI18n()

  const locks = item.lockedFields?.length ?? 0
  const ids = item.externalIds

  return (
    <Tr className={cn('group', item.isEnabled ? '' : 'opacity-60')}>
      <Td className="w-full max-w-0">
        {/* The cell is the target: the pseudo-element grows the link to the
            height of the row, so a thumb landing anywhere in the column opens
            the editor while the link stays a link for the keyboard. */}
        <Link
          to={`/admin/catalogue/${item.id}`}
          aria-label={t.admin.works.open(item.title)}
          className="flex items-center gap-2 after:absolute after:inset-0 after:content-['']"
        >
          <span className="truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
            {item.title}
          </span>
          {item.year ? (
            <span className="shrink-0 font-mono text-xs text-bone-faint tabular-nums">
              {item.year}
            </span>
          ) : null}
          <Glyph
            name={item.kind === 'series' ? 'tv' : 'film'}
            className="size-3.5 shrink-0 text-bone-faint"
            title={item.kind === 'series' ? t.nav.series : t.nav.films}
          />
        </Link>

        {/* The narrow layout has no state column, and "disabled" is not a thing
            to discover only after opening the entry. */}
        <span className="mt-1 flex gap-1.5 sm:hidden">
          {item.isEnabled ? null : <Chip tone="accent">{t.admin.works.disabled}</Chip>}
          {locks > 0 ? (
            <Chip tone="manual">
              <Glyph name="lock" className="size-3" />
              {locks}
            </Chip>
          ) : null}
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
        <span className="flex flex-wrap items-center gap-1.5">
          {item.isEnabled ? null : <Chip tone="accent">{t.admin.works.disabled}</Chip>}
          {item.isManual ? (
            <Chip tone="manual">
              <Glyph name="pencil" className="size-3" />
              {t.work.manualEntry}
            </Chip>
          ) : null}
          {locks > 0 ? (
            <Chip tone="manual">
              <Glyph name="lock" className="size-3" />
              {t.admin.works.locks(locks)}
            </Chip>
          ) : null}
          {/* The ordinary case, said quietly. Showing only the exceptions left
              most rows of this column blank, which reads as broken rather than
              as "nothing unusual here". */}
          {/* On one line: wrapped in a narrow column, it made every row of
              the catalogue three lines tall. */}
          {item.isEnabled && !item.isManual && locks === 0 ? (
            <span className="text-xs whitespace-nowrap text-bone-faint">{t.admin.works.served}</span>
          ) : null}
        </span>
      </Td>

      <Td align="right" className="hidden whitespace-nowrap md:table-cell">
        <span
          className="font-mono text-xs text-bone-faint tabular-nums"
          title={fmt.dateTime(item.updatedAt, locale)}
        >
          {fmt.relative(item.updatedAt, locale)}
        </span>
      </Td>

      <Td align="right" className="hidden md:table-cell">
        {/* Above the title's stretched link, or the row would open under
            every press meant for a button. */}
        <span className="relative z-10 flex items-center justify-end gap-0.5">
          <IconButton
            glyph="refresh"
            label={t.common.refresh}
            busy={refreshing}
            onClick={onRefresh}
          />
          <IconButton
            glyph="power"
            label={item.isEnabled ? t.admin.works.disable : t.admin.works.enable}
            busy={toggling}
            onClick={() => (item.isEnabled ? onAsk('disable') : onEnable())}
          />
          <IconButton
            glyph="trash"
            label={t.common.delete}
            tone="danger"
            onClick={() => onAsk('delete')}
          />
        </span>
      </Td>
    </Tr>
  )
}

/**
 * An entry that exists on no provider at all.
 *
 * It belongs on this screen rather than behind its own route: the person who
 * has just searched for something and not found it is exactly the person who
 * needs to type it in.
 */
function Composer({ onDone }: { onDone: () => void }) {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const [kind, setKind] = useState<MediaKind>('series')
  const [title, setTitle] = useState('')
  const [year, setYear] = useState('')
  const [overview, setOverview] = useState('')
  const [tvdb, setTvdb] = useState('')
  const [tmdb, setTmdb] = useState('')

  const create = useMutation({
    mutationFn: () =>
      api.post<MediaItem>('/items', {
        kind,
        title,
        year: year ? Number(year) : undefined,
        overview: overview || undefined,
        status: kind === 'series' ? 'continuing' : 'tba',
        externalIds: {
          ...(tvdb ? { tvdb: Number(tvdb) } : {}),
          ...(tmdb ? { tmdb: Number(tmdb) } : {}),
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
        className="grid gap-4 p-5 sm:grid-cols-2"
        onSubmit={(event) => {
          event.preventDefault()
          create.mutate()
        }}
      >
        <h2 className="font-display text-lg font-medium text-bone sm:col-span-2">
          {t.admin.works.compose.title}
        </h2>

        <FormField label={t.admin.works.kind} htmlFor="compose-kind">
          <Select
            id="compose-kind"
            value={kind}
            onChange={(event) => setKind(event.target.value as MediaKind)}
          >
            <option value="series">{t.nav.series}</option>
            <option value="movie">{t.nav.films}</option>
          </Select>
        </FormField>

        <FormField label={t.admin.works.compose.year} htmlFor="compose-year">
          <Input
            id="compose-year"
            inputMode="numeric"
            value={year}
            onChange={(event) => setYear(event.target.value.replace(/\D/g, '').slice(0, 4))}
          />
        </FormField>

        <div className="sm:col-span-2">
          <FormField
            label={t.admin.works.compose.workTitle}
            htmlFor="compose-title"
            error={create.isError ? create.error.message : undefined}
          >
            <Input
              id="compose-title"
              required
              autoFocus
              value={title}
              onChange={(event) => setTitle(event.target.value)}
            />
          </FormField>
        </div>

        <div className="sm:col-span-2">
          <FormField label={t.admin.works.compose.overview} htmlFor="compose-overview">
            <Textarea
              id="compose-overview"
              rows={3}
              value={overview}
              onChange={(event) => setOverview(event.target.value)}
            />
          </FormField>
        </div>

        <FormField
          label={t.admin.works.compose.tvdb}
          htmlFor="compose-tvdb"
          hint={t.admin.works.compose.optional}
        >
          <Input
            id="compose-tvdb"
            inputMode="numeric"
            value={tvdb}
            onChange={(event) => setTvdb(event.target.value.replace(/\D/g, ''))}
          />
        </FormField>

        <FormField
          label={t.admin.works.compose.tmdb}
          htmlFor="compose-tmdb"
          hint={t.admin.works.compose.optional}
        >
          <Input
            id="compose-tmdb"
            inputMode="numeric"
            value={tmdb}
            onChange={(event) => setTmdb(event.target.value.replace(/\D/g, ''))}
          />
        </FormField>

        <p className="text-xs leading-relaxed text-bone-faint sm:col-span-2">
          {t.admin.works.compose.note}
        </p>

        <div className="flex flex-wrap gap-2 sm:col-span-2">
          <Button type="submit" variant="primary" disabled={create.isPending || !title.trim()}>
            {create.isPending ? <Spinner className="size-4" /> : null}
            {t.admin.works.compose.submit}
          </Button>
          <Button type="button" onClick={onDone}>
            {t.common.cancel}
          </Button>
        </div>
      </form>
    </Panel>
  )
}
