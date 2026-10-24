/**
 * The people the credits name, each a record of their own.
 *
 * A record is what the catalogue knows of somebody apart from any one work:
 * the ids each source files them under, their name in their own script, the
 * other names they go by. Most are made as the works come in, and most are
 * right; this page is for the rest. Two records of one person — a guest
 * TheTVDB names one way and TMDB another — are merged, the record kept
 * taking the other's credits and names. One record of two people — a credit
 * by name alone taken for the actor of the same name — is split, the credits
 * that are somebody else's going to a new record. And a record is put right
 * by hand, or entered by hand for somebody no source knows.
 *
 * The list leans towards the records worth a second look — those known by
 * no TMDB id, those who share a name with somebody else — with every choice
 * in the address. The record chosen opens beside the list on a wide screen
 * and in its place on a narrower one, at `?id=`; the focus goes with it, and
 * comes back to its row when it closes.
 */

import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from '@tanstack/react-query'
import { useEffect, useId, useRef, useState, type ReactNode, type RefObject } from 'react'
import { Link, useSearchParams, type To } from 'react-router'

import { useAdminTitle } from '../../components/AdminShell'
import { ExternalLink } from '../../components/elsewhere'
import { Artwork } from '../../components/media'
import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  Glyph,
  Input,
  Label,
  Panel,
  PanelHead,
  Segmented,
  Skeleton,
  Spinner,
  Textarea,
} from '../../components/ui'
import { ApiError, api, query, segment } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useSettled } from '../../lib/debounce'
import { useTitle } from '../../lib/hooks'
import { useI18n } from '../../lib/i18n'
import { jobLabel } from '../../lib/labels'
import { personLink } from '../../lib/links'
import { episodeCode } from '../../lib/media'
import type { EnteredPerson, MediaItem, PeoplePage, Person, PersonCard, PersonId, Role } from '../../lib/types'
import { WRONG } from '../fields'

/** A page of the list: the next is a press away. */
const PAGE = 60

/** Where the page lives, for the links that keep its filters and change the record open. */
const HERE = '/admin/people'

/**
 * What a write to a record changes the reading of: the index and this list,
 * the people's pages, the searches, and the works whose credits name them.
 */
export function afterPeopleWrite(client: QueryClient) {
  for (const key of ['people', 'person', 'search', 'item', 'work']) {
    void client.invalidateQueries({ queryKey: [key] })
  }
}

/** The ids somebody is known by, in a line: `TMDB 17419 · IMDb nm0186505`. */
export function idsOf(person: Pick<PersonCard, 'tmdbId' | 'imdbId' | 'tvdbId'>): string {
  return [
    person.tmdbId ? `TMDB ${person.tmdbId}` : undefined,
    person.imdbId ? `IMDb ${person.imdbId}` : undefined,
    person.tvdbId ? `TheTVDB ${person.tvdbId}` : undefined,
  ]
    .filter(Boolean)
    .join(' · ')
}

export function People() {
  const { t } = useI18n()
  const p = t.admin.peoplePage
  useTitle(t.admin.peopleNav)
  const [params, setParams] = useSearchParams()
  const selected = params.get('id') ?? ''
  const all = params.get('all') === '1'
  const withoutTmdb = params.get('withoutTmdb') === '1'
  const homonyms = params.get('homonyms') === '1'
  const sort: 'works' | 'name' = params.get('sort') === 'name' ? 'name' : 'works'
  const term = (params.get('term') ?? '').trim()

  /** The address with these changed and the rest kept; what is empty is left out. */
  const addressWith = (changes: Record<string, string>, from: URLSearchParams = params) => {
    const next = new URLSearchParams(from)
    for (const [name, value] of Object.entries(changes)) {
      if (value) next.set(name, value)
      else next.delete(name)
    }
    const text = next.toString()
    return text ? `?${text}` : ''
  }
  const at = (changes: Record<string, string>): To => ({ pathname: HERE, search: addressWith(changes) })

  /** Write these into the address — from the address as it is now, not as this render saw it. */
  const write = (changes: Record<string, string>, push = false) =>
    setParams(new URLSearchParams(addressWith(changes, new URLSearchParams(window.location.search))), { replace: !push })

  // What is typed, settled into the address: the list is asked for once the
  // typing pauses, not at every letter. The address moving under the field
  // — back, a link — puts its own words in it.
  const [typed, setTyped] = useState(term)
  const [seen, setSeen] = useState(term)
  if (seen !== term) {
    setSeen(term)
    if (typed.trim() !== term) setTyped(term)
  }
  const settled = useSettled(typed.trim(), 250)
  useEffect(() => {
    if (settled !== (new URLSearchParams(window.location.search).get('term') ?? '').trim()) write({ term: settled })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `write` reads the address as it is when it runs: never stale
  }, [settled])

  const list = useInfiniteQuery({
    queryKey: ['people', 'records', term, all, withoutTmdb, homonyms, sort],
    // Each page from where the last one ended, counted in what is loaded.
    queryFn: ({ pageParam }) =>
      api.get<PeoplePage>(
        `/people${query({ term, all, withoutTmdb, homonyms, sort: sort === 'name' ? 'name' : '', limit: PAGE, offset: pageParam })}`,
      ),
    initialPageParam: 0,
    getNextPageParam: (last, pages) => {
      const loaded = pages.reduce((sum, page) => sum + page.people.length, 0)
      return loaded < last.total ? loaded : undefined
    },
    placeholderData: keepPreviousData,
  })
  const people = list.data?.pages.flatMap((page) => page.people) ?? []
  const total = list.data?.pages[0]?.total ?? 0
  const filtered = Boolean(term || all || withoutTmdb || homonyms)

  // What became of the last write, said on the record it left on screen.
  const [notice, setNotice] = useState<{ id: string; text: ReactNode } | null>(null)
  const [creating, setCreating] = useState(false)

  /** Open a record in place of the one open, saying what just became of it. */
  const open = (id: string, how: { push?: boolean; notice?: ReactNode } = {}) => {
    setNotice(how.notice ? { id, text: how.notice } : null)
    write({ id }, how.push)
  }

  // Opened from the list, a record takes the focus, which would otherwise be
  // left on a row a narrow screen no longer shows; closed, it gives the focus
  // back to its row — or to the search, where the row has gone.
  const focusRecord = useRef(false)
  const was = useRef(selected)
  useEffect(() => {
    const before = was.current
    was.current = selected
    if (!before || selected) return
    const row = document.querySelector<HTMLElement>(`[data-person="${CSS.escape(before)}"]`)
    ;(row ?? document.getElementById('people-records-search'))?.focus()
  }, [selected])

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8 flex flex-wrap items-end justify-between gap-4">
        <div className="min-w-0">
          <Label>{t.admin.title}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.peopleNav}</h1>
          <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
        </div>
        <Button variant="primary" onClick={() => setCreating(true)}>
          <Glyph name="plus" className="size-4" />
          {p.create}
        </Button>
      </header>

      <div className="xl:grid xl:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)] xl:items-start xl:gap-6">
        {/* Beside the record on a wide screen, and in its place on a narrower
            one, where the two side by side would each be too narrow to read. */}
        <div className={cn('min-w-0', selected && 'hidden xl:block')}>
          <div className="rise mb-4 space-y-3 border-y border-rule py-4">
            <div>
              <label htmlFor="people-records-search" className="label mb-1.5 block">
                {p.search}
              </label>
              <div className="relative">
                <Glyph
                  name="search"
                  className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
                />
                <Input
                  id="people-records-search"
                  type="search"
                  value={typed}
                  onChange={(event) => setTyped(event.target.value)}
                  autoComplete="off"
                  autoCorrect="off"
                  spellCheck={false}
                  className="pl-9"
                />
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Segmented<'credited' | 'all'>
                label={p.scope}
                value={all ? 'all' : 'credited'}
                onChange={(scope) => write({ all: scope === 'all' ? '1' : '' })}
                options={[
                  { value: 'credited', label: p.credited },
                  { value: 'all', label: p.everybody },
                ]}
              />
              {/* The records worth a second look, each a filter of its own:
                  pressed together, they narrow to those that are both. */}
              <Button
                size="sm"
                variant={withoutTmdb ? 'primary' : 'ghost'}
                aria-pressed={withoutTmdb}
                onClick={() => write({ withoutTmdb: withoutTmdb ? '' : '1' })}
              >
                {p.withoutTmdb}
              </Button>
              <Button
                size="sm"
                variant={homonyms ? 'primary' : 'ghost'}
                aria-pressed={homonyms}
                onClick={() => write({ homonyms: homonyms ? '' : '1' })}
              >
                {p.homonyms}
              </Button>
              <Segmented<'works' | 'name'>
                label={t.people.order}
                value={sort}
                onChange={(next) => write({ sort: next === 'name' ? 'name' : '' })}
                options={[
                  { value: 'works', label: t.people.orders.works },
                  { value: 'name', label: t.people.orders.name },
                ]}
              />
            </div>
          </div>

          <Panel label={t.admin.peopleNav} className="rise overflow-hidden" style={{ animationDelay: '60ms' }}>
            <PanelHead
              title={t.admin.peopleNav}
              action={
                <span className="flex items-center gap-3">
                  {list.isFetching && !list.isFetchingNextPage ? <Spinner className="size-3.5 text-bone-faint" /> : null}
                  {list.isSuccess ? (
                    <span className="font-mono text-xs text-bone-faint tabular-nums">{t.people.count(total)}</span>
                  ) : null}
                </span>
              }
            />
            {list.isPending ? (
              <div className="space-y-2 p-4">
                {Array.from({ length: 8 }, (_, index) => (
                  <Skeleton key={index} className="h-12 w-full" />
                ))}
              </div>
            ) : list.isError ? (
              <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-vermillion">
                <Glyph name="alert" className="size-4" />
                {list.error instanceof ApiError && list.error.status === 400 ? list.error.message : p.loadFailed}
              </p>
            ) : people.length === 0 ? (
              <EmptyState kind={filtered ? 'filtered' : 'empty'} title={p.none} hint={p.noneHint} />
            ) : (
              <>
                <ul className={cn('divide-y divide-rule transition-opacity duration-150', list.isPlaceholderData && 'opacity-60')}>
                  {people.map((person) => (
                    <Row
                      key={person.id}
                      person={person}
                      current={person.id === selected}
                      to={at({ id: person.id })}
                      onOpen={() => {
                        // The record open already keeps what it has; another is opened.
                        if (person.id === selected) return
                        focusRecord.current = true
                        setNotice(null)
                      }}
                    />
                  ))}
                </ul>
                {list.hasNextPage ? (
                  <div className="flex justify-center border-t border-rule px-4 py-3">
                    <Button size="sm" onClick={() => void list.fetchNextPage()} disabled={list.isFetchingNextPage}>
                      {list.isFetchingNextPage ? t.browse.loadingMore : t.browse.loadMore}
                    </Button>
                  </div>
                ) : null}
              </>
            )}
          </Panel>
        </div>

        {selected ? (
          <Detail
            key={selected}
            id={selected}
            focusRef={focusRecord}
            notice={notice?.id === selected ? notice.text : undefined}
            say={(text) => setNotice({ id: selected, text })}
            open={open}
            at={at}
          />
        ) : (
          <Panel className="rise hidden xl:block">
            <EmptyState title={t.admin.peopleNav} hint={p.choose} />
          </Panel>
        )}
      </div>

      <EntryDialog
        open={creating}
        onClose={() => setCreating(false)}
        onSaved={(id) => {
          setCreating(false)
          open(id, { push: true, notice: p.saved })
        }}
      />
    </div>
  )
}

/** Somebody's face at the size of a row: their portrait, or a mark where there is none. */
function Face({ image, className }: { image?: string; className?: string }) {
  return image ? (
    <Artwork
      url={image}
      role="headshot"
      alt=""
      className={cn('size-10 shrink-0 rounded-full border border-rule object-cover', className)}
    />
  ) : (
    <span className={cn('grid size-10 shrink-0 place-items-center rounded-full border border-rule bg-ink-high', className)}>
      <Glyph name="user" className="size-4 text-bone-faint" />
    </span>
  )
}

/**
 * One record in the list: who, the other names they go by, the ids they
 * are known by, how many works credit them and in what jobs. The whole row
 * is the way to it, and says all of that, so two people of one name are
 * two different links to a screen reader too.
 */
function Row({ person, current, to, onOpen }: { person: PersonCard; current: boolean; to: To; onOpen: () => void }) {
  const { t } = useI18n()
  const p = t.admin.peoplePage
  const ids = idsOf(person)
  const aliases = (person.aliases ?? []).filter((alias) => alias !== person.name)

  return (
    <li>
      <Link
        to={to}
        data-person={person.id}
        aria-current={current ? 'true' : undefined}
        onClick={onOpen}
        className={cn(
          'relative flex items-start gap-3 px-4 py-3 transition-colors duration-150 hover:bg-ink-high',
          current && 'bg-ink-high',
        )}
      >
        {/* The record open is marked by a rule at the edge, as the sidebar
            marks where you are: a shape as well as a colour. */}
        <span
          aria-hidden
          className={cn(
            'absolute inset-y-0 left-0 w-0.5 bg-vermillion transition-opacity duration-150',
            current ? 'opacity-100' : 'opacity-0',
          )}
        />
        <Face image={person.image} />
        <span className="min-w-0 flex-1">
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className={cn('text-sm font-medium', current ? 'text-vermillion' : 'text-bone')}>{person.name}</span>
            {person.isManual ? (
              <Chip tone="manual">
                <Glyph name="lock" className="size-3" />
                {p.byHand}
              </Chip>
            ) : null}
          </span>
          {aliases.length ? (
            <span className="block truncate text-xs text-bone-faint">
              {p.also} · {aliases.join(' · ')}
            </span>
          ) : null}
          {ids ? <span className="mt-0.5 block truncate font-mono text-caption text-bone-faint">{ids}</span> : null}
        </span>
        <span className="shrink-0 text-right">
          <span className="block font-mono text-xs text-bone-dim tabular-nums">{t.people.works(person.works)}</span>
          {person.jobs.length ? (
            <span className="block text-caption text-bone-faint">{person.jobs.map((job) => t.person.job[job]).join(' · ')}</span>
          ) : null}
        </span>
      </Link>
    </li>
  )
}

/**
 * The record open: who they are, the ids they are known by, and each of
 * their credits, work by work, to be chosen and split off onto somebody
 * else; and the ways to put the record right and to merge it into another.
 */
function Detail({
  id,
  focusRef,
  notice,
  say,
  open,
  at,
}: {
  id: string
  /** Whether it was opened from the list, which hands it the focus. */
  focusRef: RefObject<boolean>
  notice?: ReactNode
  say: (text: ReactNode) => void
  open: (id: string, how?: { push?: boolean; notice?: ReactNode }) => void
  at: (changes: Record<string, string>) => To
}) {
  const { t } = useI18n()
  const p = t.admin.peoplePage
  const client = useQueryClient()
  const form = useId()
  const heading = useRef<HTMLHeadingElement>(null)
  const [chosen, setChosen] = useState<string[]>([])
  const [name, setName] = useState('')
  const [editing, setEditing] = useState(false)
  const [merging, setMerging] = useState(false)

  const record = useQuery({
    queryKey: ['person', id, 'record'],
    // Without a language: the record's own biography, not a reader's
    // translation of it, which is what a correction is made to.
    queryFn: () => api.get<Person>(`/people/${segment(id)}`),
  })
  const person = record.data
  useAdminTitle(person?.name ?? null)

  // A record merged into another answers as that one: the address follows.
  useEffect(() => {
    if (person && person.id !== id) open(person.id)
  }, [person, id, open])

  useEffect(() => {
    if (!person || !focusRef.current) return
    focusRef.current = false
    heading.current?.focus()
  }, [person, focusRef])

  const split = useMutation({
    mutationFn: () => api.post<PersonId>(`/people/${segment(id)}/split`, { credits: chosen, name: name.trim() || undefined }),
    onSuccess: (made) => {
      const count = chosen.length
      const who = name.trim() || person?.name || ''
      setChosen([])
      setName('')
      afterPeopleWrite(client)
      say(
        <>
          {p.splitDone(count, who)}{' '}
          <Link to={at({ id: made.id })} className="text-vermillion underline-offset-2 hover:underline">
            {p.open}
          </Link>
        </>,
      )
    },
  })

  const merge = useMutation({
    mutationFn: (into: PersonCard) => api.post<PersonId>(`/people/${segment(id)}/merge`, { into: into.id }),
    onSuccess: (kept, into) => {
      setMerging(false)
      afterPeopleWrite(client)
      open(kept.id, { notice: p.merged(into.name) })
    },
  })

  const back = (
    <Link
      to={at({ id: '' })}
      className="inline-flex min-h-11 items-center gap-1.5 px-2 text-sm text-bone-dim transition-colors duration-150 hover:text-bone"
    >
      <Glyph name="arrowLeft" className="size-4" />
      {p.back}
    </Link>
  )

  if (record.isPending) {
    return (
      <Panel className="space-y-3 p-5">
        <Skeleton className="h-10 w-1/2" />
        <Skeleton className="h-4 w-1/3" />
        <Skeleton className="h-40 w-full" />
      </Panel>
    )
  }
  if (record.isError || !person) {
    const gone = record.error instanceof ApiError && record.error.status === 404
    return (
      <Panel>
        <div className="border-b border-rule px-3 py-1">{back}</div>
        <EmptyState
          kind={gone ? 'empty' : 'error'}
          title={gone ? p.gone : t.common.error}
          hint={gone ? p.goneHint : p.loadFailed}
        />
      </Panel>
    )
  }

  const lines = person.works
    .map((work) => ({ work, roles: person.roles.filter((role) => role.workId === work.id) }))
    .filter((line) => line.roles.length)
  const aliases = (person.aliases ?? []).filter((alias) => alias !== person.name && alias !== person.originalName)
  const imdbId = person.details?.imdbId
  const toggle = (credits: string[], on: boolean) =>
    setChosen((was) => (on ? [...new Set([...was, ...credits])] : was.filter((credit) => !credits.includes(credit))))

  return (
    <Panel label={person.name} className="rise">
      <div className="border-b border-rule px-3 py-1">{back}</div>
      {notice ? (
        <p role="status" className="border-b border-rule bg-ink-high px-5 py-2.5 text-sm text-bone">
          {notice}
        </p>
      ) : null}

      <header className="flex items-start gap-4 border-b border-rule p-5">
        <Face image={person.image} className="size-16" />
        <div className="min-w-0 flex-1">
          <h2 ref={heading} tabIndex={-1} className="font-display text-2xl font-medium text-bone outline-none">
            {person.name}
          </h2>
          {person.originalName && person.originalName !== person.name ? (
            <p lang={person.originalNameLang} className="text-base text-bone-dim">
              {person.originalName}
            </p>
          ) : null}
          <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-caption text-bone-faint">
            {person.tmdbId ? <ExternalLink href={personLink(person.tmdbId)}>TMDB {person.tmdbId}</ExternalLink> : null}
            {imdbId ? (
              <ExternalLink href={`https://www.imdb.com/name/${encodeURIComponent(imdbId)}/`}>IMDb {imdbId}</ExternalLink>
            ) : null}
            {person.tvdbId ? <span>TheTVDB {person.tvdbId}</span> : null}
            {person.isManual ? (
              <Chip tone="manual">
                <Glyph name="lock" className="size-3" />
                {p.byHand}
              </Chip>
            ) : null}
          </p>
          {aliases.length ? (
            <p className="mt-2 text-xs text-bone-dim">
              <span className="label mr-2">{p.also}</span>
              {aliases.join(' · ')}
            </p>
          ) : null}
        </div>
      </header>

      <div className="flex flex-wrap items-center gap-2 border-b border-rule px-5 py-3">
        <Button size="sm" onClick={() => setEditing(true)}>
          <Glyph name="pencil" className="size-3.5" />
          {p.edit}
        </Button>
        <Button size="sm" onClick={() => setMerging(true)}>
          <Glyph name="link" className="size-3.5" />
          {p.merge}
        </Button>
        <Link
          to={`/people/${encodeURIComponent(person.id)}`}
          className="inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-body-sm font-medium text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
        >
          <Glyph name="user" className="size-3.5" />
          {p.publicPage}
        </Link>
      </div>

      <section aria-labelledby={`${form}-credits`}>
        <h3 id={`${form}-credits`} className="label px-5 pt-4">
          {p.credits}
        </h3>
        {lines.length ? (
          <ul className="divide-y divide-rule">
            {lines.map((line) => (
              <WorkCredits key={line.work.id} work={line.work} roles={line.roles} chosen={chosen} onToggle={toggle} />
            ))}
          </ul>
        ) : (
          <p className="px-5 pt-2 pb-5 text-sm text-bone-faint">{p.noCredits}</p>
        )}
      </section>

      {/* Credits chosen are split off onto a new record. Held at the foot of
          the screen while the list scrolls past — above the bar a phone
          keeps there — so the way to act on a choice stays in view of it. */}
      {person.roles.length ? (
        <form
          onSubmit={(event) => {
            event.preventDefault()
            if (chosen.length) split.mutate()
          }}
          className="sticky bottom-[calc(3.5rem+env(safe-area-inset-bottom))] z-10 rounded-b-panel border-t border-rule bg-ink-raised px-5 py-3 lg:bottom-0"
        >
          <div className="flex flex-wrap items-end gap-3">
            <div className="min-w-48 flex-1">
              <label htmlFor={`${form}-name`} className="label mb-1.5 block">
                {p.splitName}
              </label>
              <Input
                id={`${form}-name`}
                value={name}
                placeholder={person.name}
                onChange={(event) => setName(event.target.value)}
                aria-describedby={`${form}-hint`}
                autoComplete="off"
              />
            </div>
            <Button type="submit" disabled={!chosen.length || split.isPending}>
              {split.isPending ? <Spinner className="size-4" /> : null}
              {p.split}
            </Button>
          </div>
          <p id={`${form}-hint`} className="mt-1.5 text-xs text-bone-faint">
            {p.splitHint(person.name)}
          </p>
          <div className="mt-1 flex min-h-11 flex-wrap items-center gap-3">
            <span aria-live="polite" className="font-mono text-xs text-bone-dim tabular-nums">
              {p.selected(chosen.length)}
            </span>
            {chosen.length ? (
              <Button size="sm" variant="quiet" onClick={() => setChosen([])}>
                {p.clear}
              </Button>
            ) : null}
          </div>
          {split.isError ? (
            <p role="alert" className="mt-1 text-xs text-vermillion">
              {split.error.message || t.common.actionFailed}
            </p>
          ) : null}
        </form>
      ) : null}

      <EntryDialog
        open={editing}
        person={person}
        onClose={() => setEditing(false)}
        onSaved={() => {
          setEditing(false)
          say(p.saved)
        }}
      />
      <MergeDialog
        open={merging}
        person={person}
        pending={merge.isPending}
        error={merge.error}
        onMerge={(into) => merge.mutate(into)}
        onClose={() => {
          merge.reset()
          setMerging(false)
        }}
      />
    </Panel>
  )
}

/** Whether a credit is on one episode of a series rather than on the series. */
function onEpisode(role: Pick<Role, 'seasonNumber' | 'episodeNumber'>): boolean {
  return typeof role.seasonNumber === 'number' && typeof role.episodeNumber === 'number'
}

/**
 * One work's credits of the record, each a box to choose, and a box for all
 * of them where there are several — a guest's episodes, a director's.
 */
function WorkCredits({
  work,
  roles,
  chosen,
  onToggle,
}: {
  work: MediaItem
  roles: Role[]
  chosen: string[]
  onToggle: (credits: string[], on: boolean) => void
}) {
  const { t, lang } = useI18n()
  const p = t.admin.peoplePage
  const c = t.admin.editor.children
  const title = useId()
  const credits = roles.map((role) => role.creditId)
  const picked = credits.filter((credit) => chosen.includes(credit)).length
  const types: Record<Role['creditType'], string> = {
    actor: c.actor,
    guest: c.guest,
    director: c.director,
    writer: c.writer,
    producer: c.producer,
  }
  // What the credit is, past its type: the episode it is on, and the part
  // played or the job done, as the work's own credits say it.
  const said = (role: Role) =>
    [
      onEpisode(role)
        ? [episodeCode({ seasonNumber: role.seasonNumber ?? 0, episodeNumber: role.episodeNumber ?? 0 }), role.episodeTitle]
            .filter(Boolean)
            .join(' ')
        : undefined,
      role.character
        ? role.creditType === 'actor' || role.creditType === 'guest'
          ? c.as(role.character)
          : jobLabel(role.character, lang)
        : undefined,
    ]
      .filter(Boolean)
      .join(' · ') || '—'

  return (
    <li className="px-5 py-3">
      <div role="group" aria-labelledby={title}>
        <div className="flex min-h-11 items-center gap-3 px-2">
          {roles.length > 1 ? (
            <Box
              checked={picked === credits.length}
              mixed={picked > 0 && picked < credits.length}
              label={p.selectWork(work.title)}
              onChange={(on) => onToggle(credits, on)}
            />
          ) : null}
          <Link
            id={title}
            to={`/admin/catalogue/${encodeURIComponent(work.id)}?tab=people`}
            className="min-w-0 truncate text-sm font-medium text-bone transition-colors duration-150 hover:text-vermillion"
          >
            {work.title}
          </Link>
          <span className="shrink-0 font-mono text-caption text-bone-faint tabular-nums">
            {[work.year, work.kind === 'series' ? t.home.kindSeries : t.home.kindFilm].filter(Boolean).join(' · ')}
          </span>
        </div>
        <ul>
          {roles.map((role) => (
            <li key={role.creditId}>
              <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-card px-2 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high">
                <input
                  type="checkbox"
                  checked={chosen.includes(role.creditId)}
                  onChange={(event) => onToggle([role.creditId], event.target.checked)}
                  className="size-4 shrink-0 cursor-pointer accent-vermillion"
                />
                <span className="w-20 shrink-0 font-mono text-label tracking-[0.08em] text-slate uppercase">
                  {types[role.creditType]}
                </span>
                <span className="min-w-0 truncate">{said(role)}</span>
              </label>
            </li>
          ))}
        </ul>
      </div>
    </li>
  )
}

/** A box that chooses several at once, and says so when some of them are chosen and not all. */
function Box({
  checked,
  mixed,
  label,
  onChange,
}: {
  checked: boolean
  mixed: boolean
  label: string
  onChange: (on: boolean) => void
}) {
  const ref = useRef<HTMLInputElement>(null)
  // The mixed state is the element's own, which no attribute sets.
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = mixed
  }, [mixed])

  return (
    <input
      ref={ref}
      type="checkbox"
      aria-label={label}
      checked={checked}
      onChange={(event) => onChange(event.target.checked)}
      className="size-4 shrink-0 cursor-pointer accent-vermillion"
    />
  )
}

/**
 * Somebody to file something under, found by a part of a name — theirs, or
 * one they go by — among every record, those nothing credits yet included:
 * one just entered by hand is the one most often looked for. Each found is a
 * button that says who it is, by their ids and how much of their work is here,
 * so that two of one name are told apart before one is chosen.
 */
export function PersonPicker({
  id,
  exclude,
  onPick,
  autoFocus,
}: {
  id: string
  /** The record the choice is for, which is no choice. */
  exclude?: string
  onPick: (person: PersonCard) => void
  autoFocus?: boolean
}) {
  const { t } = useI18n()
  const p = t.admin.peoplePage
  const [term, setTerm] = useState('')
  const settled = useSettled(term.trim(), 250)
  const asked = settled.length >= 2

  const found = useQuery({
    queryKey: ['people', 'pick', settled],
    queryFn: () => api.get<PeoplePage>(`/people${query({ term: settled, all: true, limit: 8 })}`),
    enabled: asked,
    placeholderData: keepPreviousData,
    staleTime: 30_000,
  })
  const people = asked ? (found.data?.people ?? []).filter((person) => person.id !== exclude) : []

  return (
    <div>
      <label htmlFor={id} className="label mb-1.5 block">
        {p.find}
      </label>
      <div className="relative">
        <Glyph
          name="search"
          className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
        />
        <Input
          id={id}
          type="search"
          value={term}
          onChange={(event) => setTerm(event.target.value)}
          aria-describedby={`${id}-hint`}
          autoFocus={autoFocus}
          autoComplete="off"
          autoCorrect="off"
          spellCheck={false}
          className="pl-9"
        />
      </div>
      <p id={`${id}-hint`} className="mt-1 text-xs text-bone-faint">
        {p.findHint}
      </p>
      {/* How many were found, said once they are: the list itself is not live. */}
      <p role="status" className="sr-only">
        {asked && found.isSuccess && !found.isPlaceholderData ? (people.length ? p.found(people.length) : p.nobody) : ''}
      </p>
      {!asked ? null : found.isError ? (
        <p role="alert" className="mt-3 text-xs text-vermillion">
          {found.error.message || t.common.actionFailed}
        </p>
      ) : found.isPending ? (
        <Spinner className="mt-3 size-4 text-bone-faint" />
      ) : people.length ? (
        <ul
          className={cn(
            'mt-3 divide-y divide-rule rounded-card border border-rule transition-opacity duration-150',
            found.isPlaceholderData && 'opacity-60',
          )}
        >
          {people.map((person) => (
            <li key={person.id}>
              <button
                type="button"
                onClick={() => onPick(person)}
                className="flex min-h-11 w-full cursor-pointer items-center gap-3 px-3 py-2 text-left transition-colors duration-150 hover:bg-ink-high"
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-bone">{person.name}</span>
                  <span className="block truncate font-mono text-caption text-bone-faint">
                    {[idsOf(person), t.people.works(person.works)].filter(Boolean).join(' · ')}
                  </span>
                </span>
                <Glyph name="chevronRight" className="size-4 shrink-0 text-bone-faint" />
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-3 text-sm text-bone-faint">{p.nobody}</p>
      )}
    </div>
  )
}

/**
 * Merging a record into another: the one to keep found and chosen, then
 * what will happen said, before anything does — it is not undone from here.
 */
function MergeDialog({
  open,
  person,
  pending,
  error,
  onMerge,
  onClose,
}: {
  open: boolean
  person: Person
  pending: boolean
  error: Error | null
  onMerge: (into: PersonCard) => void
  onClose: () => void
}) {
  const { t } = useI18n()
  const p = t.admin.peoplePage
  const [into, setInto] = useState<PersonCard | null>(null)
  // The button that was pressed is gone with the list it stood in: the
  // focus goes to the way back, never to the merge itself.
  const again = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (into) again.current?.focus()
  }, [into])

  const close = () => {
    setInto(null)
    onClose()
  }

  return (
    <Dialog
      open={open}
      title={into ? p.mergeConfirm(person.name, into.name) : p.mergeTitle(person.name)}
      onClose={close}
      footer={
        into ? (
          <>
            <Button ref={again} onClick={() => setInto(null)}>
              {p.mergeBack}
            </Button>
            <Button variant="danger" disabled={pending} onClick={() => onMerge(into)}>
              {pending ? <Spinner className="size-4" /> : null}
              {p.mergeAction}
            </Button>
          </>
        ) : (
          <Button onClick={close}>{t.common.cancel}</Button>
        )
      }
    >
      {into ? (
        <div className="space-y-3">
          <p>{p.mergeBody(person.name, into.name)}</p>
          {idsOf(into) ? <p className="font-mono text-xs text-bone-faint">{idsOf(into)}</p> : null}
          {error ? (
            <p role="alert" className="flex items-center gap-1.5 text-xs text-status-error">
              <Glyph name="alert" className="size-3.5 shrink-0" />
              {error.message || t.common.actionFailed}
            </p>
          ) : null}
        </div>
      ) : (
        <div className="space-y-4">
          <p>{p.mergeLead}</p>
          <PersonPicker id="merge-into" exclude={person.id} onPick={setInto} autoFocus />
        </div>
      )}
    </Dialog>
  )
}

/** The fields of a record, as the form holds them: text, every one. */
type Entry = Record<
  | 'name'
  | 'sortName'
  | 'originalName'
  | 'aliases'
  | 'tmdbId'
  | 'imdbId'
  | 'tvdbId'
  | 'gender'
  | 'birthday'
  | 'deathday'
  | 'placeOfBirth'
  | 'image'
  | 'biography',
  string
>

/** A gender as the form chooses it: TMDB's 1, 2 or 3, or nothing said. */
function genderOf(gender: number | undefined): string {
  return gender !== undefined && [1, 2, 3].includes(gender) ? String(gender) : ''
}

/**
 * A record's fields as the form starts them: the record as it is kept,
 * which the page sends whoever may put it right — the sort name, the
 * portrait's address at its source, and of the other names only those typed
 * by hand, the only ones a save replaces.
 *
 * Where a server sends no record, what the page says of them instead: the
 * portrait only where it is the source's — the page gives this server's copy
 * where one is kept, which a record does not take — and no sort name.
 */
export function entryOf(person?: Person): Entry {
  const record = person?.record
  if (record) {
    return {
      name: record.name,
      sortName: record.sortName ?? '',
      originalName: record.originalName ?? '',
      aliases: (record.aliases ?? [])
        .filter((alias) => alias.source === 'manual')
        .map((alias) => alias.name)
        .join('\n'),
      tmdbId: record.tmdbId ? String(record.tmdbId) : '',
      imdbId: record.imdbId ?? '',
      tvdbId: record.tvdbId ? String(record.tvdbId) : '',
      gender: genderOf(record.gender),
      birthday: record.birthday ?? '',
      deathday: record.deathday ?? '',
      placeOfBirth: record.placeOfBirth ?? '',
      image: record.image ?? '',
      biography: record.biography ?? '',
    }
  }
  const details = person?.details
  const image = person?.image && !person.image.includes('/media/') ? person.image : ''
  return {
    name: person?.name ?? '',
    sortName: '',
    originalName: person?.originalName ?? '',
    aliases: (person?.aliases ?? []).join('\n'),
    tmdbId: person?.tmdbId ? String(person.tmdbId) : '',
    imdbId: details?.imdbId ?? '',
    tvdbId: person?.tvdbId ? String(person.tvdbId) : '',
    gender: genderOf(details?.gender),
    birthday: details?.birthday ?? '',
    deathday: details?.deathday ?? '',
    placeOfBirth: details?.placeOfBirth ?? '',
    image,
    biography: details?.biography ?? '',
  }
}

/** What the form sends: every field, an empty one left out — which clears it — and a name a line for the other names. */
export function enteredOf(entry: Entry): EnteredPerson {
  const text = (value: string) => value.trim() || undefined
  const number = (value: string) => (/^\d+$/.test(value.trim()) ? Number(value.trim()) : undefined)
  return {
    name: entry.name.trim(),
    sortName: text(entry.sortName),
    originalName: text(entry.originalName),
    aliases: [
      ...new Set(
        entry.aliases
          .split('\n')
          .map((alias) => alias.trim())
          .filter(Boolean),
      ),
    ],
    tmdbId: number(entry.tmdbId),
    imdbId: text(entry.imdbId),
    tvdbId: number(entry.tvdbId),
    gender: entry.gender ? Number(entry.gender) : undefined,
    birthday: text(entry.birthday),
    deathday: text(entry.deathday),
    placeOfBirth: text(entry.placeOfBirth),
    image: text(entry.image),
    biography: text(entry.biography),
  }
}

/**
 * A record entered by hand, or put right: every field of it, as a form.
 * Saved whole — a field left empty is cleared — and marked put right by
 * hand from then on, as the form says above its button.
 */
function EntryDialog({
  open,
  person,
  onClose,
  onSaved,
}: {
  open: boolean
  /** The record put right; none for one entered. */
  person?: Person
  onClose: () => void
  onSaved: (id: string) => void
}) {
  const { t } = useI18n()
  const p = t.admin.peoplePage
  const client = useQueryClient()
  const id = person ? 'person-edit' : 'person-new'

  const save = useMutation({
    mutationFn: async (body: EnteredPerson) => {
      if (!person) return (await api.post<PersonId>('/people', body)).id
      await api.put(`/people/${segment(person.id)}`, body)
      return person.id
    },
    onSuccess: (saved) => {
      afterPeopleWrite(client)
      onSaved(saved)
    },
  })

  const close = () => {
    save.reset()
    onClose()
  }

  return (
    <Dialog
      open={open}
      size="lg"
      title={person ? p.editTitle(person.name) : p.createTitle}
      onClose={close}
      footer={
        <>
          <Button onClick={close}>{t.common.cancel}</Button>
          <Button type="submit" form={id} variant="primary" disabled={save.isPending}>
            {save.isPending ? <Spinner className="size-4" /> : null}
            {person ? t.common.save : p.create}
          </Button>
        </>
      }
    >
      <EntryForm id={id} person={person} error={save.error} onSubmit={(body) => save.mutate(body)} />
    </Dialog>
  )
}

/** One field of a record: its label above it, and what it takes said under it, tied to it for a screen reader. */
function Field({
  id,
  label,
  hint,
  children,
}: {
  id: string
  label: string
  hint?: string
  children: (describedBy: string | undefined) => ReactNode
}) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={id} className="label block">
        {label}
      </label>
      {children(hint ? `${id}-hint` : undefined)}
      {hint ? (
        <p id={`${id}-hint`} className="text-xs text-bone-faint">
          {hint}
        </p>
      ) : null}
    </div>
  )
}

/**
 * The form itself, started afresh each time the dialog opens. The browser
 * checks what it can before anything is sent — a name, ids that are ids,
 * days that are days, an address that is one — and the server the rest:
 * a TMDB id another record holds is refused, and said so here by name.
 */
function EntryForm({
  id,
  person,
  error,
  onSubmit,
}: {
  id: string
  person?: Person
  error: Error | null
  onSubmit: (body: EnteredPerson) => void
}) {
  const { t } = useI18n()
  const p = t.admin.peoplePage
  const f = p.fields
  const [entry, setEntry] = useState(() => entryOf(person))
  const set = (field: keyof Entry) => (event: { target: { value: string } }) =>
    setEntry((was) => ({ ...was, [field]: event.target.value }))
  const at = (field: keyof Entry) => `${id}-${field}`
  // The names that came another way than by hand: a save keeps them, so
  // they are shown beside the field rather than in it.
  const kept = (person?.record?.aliases ?? []).filter((alias) => alias.source !== 'manual')
  // Identifiers, not words: nothing to correct as they are typed.
  const code = { autoComplete: 'off', autoCorrect: 'off', autoCapitalize: 'off', spellCheck: false } as const

  return (
    <form
      id={id}
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault()
        onSubmit(enteredOf(entry))
      }}
    >
      <Field id={at('name')} label={f.name}>
        {(described) => (
          <Input
            id={at('name')}
            name="name"
            required
            value={entry.name}
            onChange={set('name')}
            aria-describedby={described}
            autoComplete="off"
            className={WRONG}
          />
        )}
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field id={at('sortName')} label={f.sortName} hint={p.hints.sortName}>
          {(described) => (
            <Input id={at('sortName')} name="sortName" value={entry.sortName} onChange={set('sortName')} aria-describedby={described} autoComplete="off" />
          )}
        </Field>
        <Field id={at('originalName')} label={f.originalName} hint={p.hints.originalName}>
          {(described) => (
            <Input
              id={at('originalName')}
              name="originalName"
              value={entry.originalName}
              onChange={set('originalName')}
              aria-describedby={described}
              autoComplete="off"
            />
          )}
        </Field>
      </div>
      <Field id={at('aliases')} label={f.aliases} hint={p.hints.aliases}>
        {(described) => (
          <Textarea
            id={at('aliases')}
            name="aliases"
            rows={3}
            value={entry.aliases}
            onChange={set('aliases')}
            aria-describedby={described}
            autoComplete="off"
          />
        )}
      </Field>
      {kept.length ? (
        <div className="space-y-1.5">
          <span id={`${at('aliases')}-kept`} className="label block">
            {p.keptAliases}
          </span>
          <ul aria-labelledby={`${at('aliases')}-kept`} className="space-y-0.5 text-sm">
            {kept.map((alias) => (
              <li key={`${alias.source}:${alias.name}`} className="flex flex-wrap items-baseline gap-x-2">
                <span className="text-bone">{alias.name}</span>
                <span className="text-xs text-bone-faint">{p.aliasSources[alias.source] ?? alias.source}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <div className="grid gap-4 sm:grid-cols-3">
        <Field id={at('tmdbId')} label={f.tmdbId} hint={p.hints.tmdbId}>
          {(described) => (
            <Input
              id={at('tmdbId')}
              name="tmdbId"
              inputMode="numeric"
              pattern="[1-9][0-9]*"
              value={entry.tmdbId}
              onChange={set('tmdbId')}
              aria-describedby={described}
              className={cn('font-mono tabular-nums', WRONG)}
              {...code}
            />
          )}
        </Field>
        <Field id={at('imdbId')} label={f.imdbId} hint={p.hints.imdbId}>
          {(described) => (
            <Input
              id={at('imdbId')}
              name="imdbId"
              pattern="nm[0-9]+"
              value={entry.imdbId}
              onChange={set('imdbId')}
              aria-describedby={described}
              className={cn('font-mono', WRONG)}
              {...code}
            />
          )}
        </Field>
        <Field id={at('tvdbId')} label={f.tvdbId}>
          {(described) => (
            <Input
              id={at('tvdbId')}
              name="tvdbId"
              inputMode="numeric"
              pattern="[1-9][0-9]*"
              value={entry.tvdbId}
              onChange={set('tvdbId')}
              aria-describedby={described}
              className={cn('font-mono tabular-nums', WRONG)}
              {...code}
            />
          )}
        </Field>
      </div>
      <div className="space-y-1.5">
        <span className="label block">{f.gender}</span>
        <Segmented<'' | '1' | '2' | '3'>
          label={f.gender}
          value={entry.gender as '' | '1' | '2' | '3'}
          onChange={(gender) => setEntry((was) => ({ ...was, gender }))}
          options={[
            { value: '', label: p.genders.unknown },
            { value: '1', label: p.genders.woman },
            { value: '2', label: p.genders.man },
            { value: '3', label: p.genders.nonBinary },
          ]}
        />
      </div>
      <div className="grid gap-4 sm:grid-cols-3">
        <Field id={at('birthday')} label={f.birthday}>
          {(described) => (
            <Input
              id={at('birthday')}
              name="birthday"
              type="date"
              value={entry.birthday}
              onChange={set('birthday')}
              aria-describedby={described}
              className={WRONG}
            />
          )}
        </Field>
        <Field id={at('deathday')} label={f.deathday}>
          {(described) => (
            <Input
              id={at('deathday')}
              name="deathday"
              type="date"
              min={entry.birthday || undefined}
              value={entry.deathday}
              onChange={set('deathday')}
              aria-describedby={described}
              className={WRONG}
            />
          )}
        </Field>
        <Field id={at('placeOfBirth')} label={f.placeOfBirth}>
          {(described) => (
            <Input
              id={at('placeOfBirth')}
              name="placeOfBirth"
              value={entry.placeOfBirth}
              onChange={set('placeOfBirth')}
              aria-describedby={described}
              autoComplete="off"
            />
          )}
        </Field>
      </div>
      <Field id={at('image')} label={f.image} hint={p.hints.image}>
        {(described) => (
          <Input
            id={at('image')}
            name="image"
            type="url"
            pattern="https?://.+"
            placeholder="https://…"
            value={entry.image}
            onChange={set('image')}
            aria-describedby={described}
            className={WRONG}
            {...code}
          />
        )}
      </Field>
      <Field id={at('biography')} label={f.biography}>
        {(described) => (
          <Textarea
            id={at('biography')}
            name="biography"
            rows={6}
            value={entry.biography}
            onChange={set('biography')}
            aria-describedby={described}
          />
        )}
      </Field>
      <p className="text-xs leading-relaxed text-bone-faint">{p.hints.manual}</p>
      {error ? (
        <p role="alert" className="flex items-center gap-1.5 text-xs text-status-error">
          <Glyph name="alert" className="size-3.5 shrink-0" />
          {error.message || t.common.actionFailed}
        </p>
      ) : null}
    </form>
  )
}
