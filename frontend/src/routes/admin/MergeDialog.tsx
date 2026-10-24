/**
 * Two records of one work, folded into one — or said not to be one.
 *
 * Opened on a pair the catalogue proposed, or on one work and another found
 * here by its title. The two are laid side by side: what each is, which of
 * them stays — the one its sources fill over one entered by hand, then the
 * one a person put something into by hand, else the one filed under more
 * identifiers, else the older — and why, with a way to keep
 * the other instead. Every field whose value differs is a row with a choice
 * between the two, the survivor's chosen to begin with; a value taken from
 * the merged record is locked on the survivor. What becomes of the rest —
 * seasons, episodes, pictures, locks, places in selections — and of the
 * clients that know the merged record by its ids is said in one paragraph
 * before anything is done. A pair that is not one work is dismissed from
 * here, and is not proposed again.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'

import { Artwork } from '../../components/media'
import { Button, Chip, Dialog, Glyph, Input, Skeleton, Spinner } from '../../components/ui'
import { ApiError, api, query, segment } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useSettled } from '../../lib/debounce'
import * as fmt from '../../lib/format'
import { useI18n, type Dict } from '../../lib/i18n'
import { providerName } from '../../lib/labels'
import type {
  FieldRegistry,
  ItemPage,
  MediaKind,
  MergeField,
  MergeOutcome,
  MergePreview,
  MergeSide,
  MergeWork,
} from '../../lib/types'

import { fieldLabel } from './editor/FieldRow'
import { shown } from './editor/journal'

/** The work the dialog was opened on, as the row that opened it knows it. */
export type MergeOpening = { id: string; title: string; kind: MediaKind }

/** Which of the two becomes the other's, and which stays. */
type Roles = { merged: string; into: string }

/** An identifier's source, named as a reader knows it: `tvdb_series` is TheTVDB. */
function sourceName(source: string): string {
  return providerName(source.split('_')[0] ?? source)
}

export function MergeDialog({
  open,
  work,
  other,
  proposed,
  flagged,
  onClose,
  onDone,
}: {
  open: boolean
  work: MergeOpening
  /** The other record, when the dialog is opened on a pair; found here otherwise. */
  other?: string
  /** Whether the pair was proposed, and so may be said not to be one work. */
  proposed?: boolean
  /** A film and a series under one IMDb id: never merged, only put right or dismissed. */
  flagged?: boolean
  onClose: () => void
  /** What came of it, said in a sentence for the page's status line. */
  onDone: (said: string) => void
}) {
  const { t, locale } = useI18n()
  const m = t.admin.merge
  const queryClient = useQueryClient()

  const [chosen, setChosen] = useState<string | undefined>(other)
  const [roles, setRoles] = useState<Roles | undefined>(other ? { merged: work.id, into: other } : undefined)
  // Once the rule has been heard, or the person chose, the sides stay put.
  const [settled, setSettled] = useState(false)
  const [keep, setKeep] = useState<Record<string, MergeSide>>({})

  const choose = (id: string) => {
    setChosen(id)
    setRoles({ merged: work.id, into: id })
    setSettled(false)
    setKeep({})
  }
  const swap = () => {
    setRoles((now) => (now ? { merged: now.into, into: now.merged } : now))
    setSettled(true)
    setKeep({})
  }

  const registry = useQuery({
    queryKey: ['fields'],
    queryFn: () => api.get<FieldRegistry>('/fields'),
    staleTime: Infinity,
    enabled: open,
  })
  const preview = useQuery({
    queryKey: ['merge-preview', roles?.merged, roles?.into],
    queryFn: () => api.get<MergePreview>(`/items/${segment(roles!.merged)}/merge${query({ into: roles!.into })}`),
    enabled: open && Boolean(roles) && !flagged,
    gcTime: 0,
    staleTime: 0,
    retry: false,
  })

  // The rule's survivor, the first time it is heard: the dialog opens on it.
  useEffect(() => {
    const said = preview.data
    if (!said || settled || !roles) return
    setSettled(true)
    if (said.proposed !== roles.into) {
      setRoles({ merged: roles.into, into: roles.merged })
    }
  }, [preview.data, settled, roles])

  const finished = (said: string) => {
    void queryClient.invalidateQueries({ queryKey: ['items'] })
    void queryClient.invalidateQueries({ queryKey: ['duplicates'] })
    void queryClient.invalidateQueries({ queryKey: ['admin', 'todo'] })
    void queryClient.invalidateQueries({ queryKey: ['item'] })
    void queryClient.invalidateQueries({ queryKey: ['work'] })
    onDone(said)
  }

  const merge = useMutation({
    mutationFn: (asked: { roles: Roles; keep: Record<string, MergeSide> }) =>
      api.post<MergeOutcome>(`/items/${segment(asked.roles.merged)}/merge`, {
        into: asked.roles.into,
        keep: asked.keep,
      }),
    onSuccess: (outcome) => {
      const said = preview.data
      finished(
        m.done(
          said?.merged.title ?? '',
          said?.survivor.title ?? '',
          outcome.undoUntil ? fmt.dateTime(outcome.undoUntil, locale) : undefined,
        ),
      )
    },
  })
  const dismiss = useMutation({
    mutationFn: () => api.post('/admin/duplicates/dismiss', { first: work.id, second: chosen }),
    onSuccess: () => finished(m.dismissed),
  })

  const said = preview.data
  const ready = Boolean(said) && settled && said?.proposed !== undefined
  const busy = merge.isPending || dismiss.isPending

  return (
    <Dialog
      open={open}
      size="lg"
      title={m.title}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t.common.cancel}</Button>
          {proposed && chosen ? (
            <Button variant="quiet" disabled={busy} onClick={() => dismiss.mutate()}>
              {dismiss.isPending ? <Spinner className="size-4" /> : <Glyph name="close" className="size-4" />}
              {m.dismiss}
            </Button>
          ) : null}
          <Button
            variant="primary"
            disabled={!ready || busy || !roles || flagged}
            onClick={() => {
              if (!roles) return
              const asked = Object.fromEntries(Object.entries(keep).filter(([, side]) => side === 'merged'))
              merge.mutate({ roles, keep: asked })
            }}
          >
            {merge.isPending ? <Spinner className="size-4" /> : <Glyph name="copy" className="size-4" />}
            {merge.isPending ? m.merging : m.merge}
          </Button>
        </>
      }
    >
      {!chosen ? (
        <Picker work={work} onPick={choose} />
      ) : flagged ? (
        <p className="flex items-start gap-2 text-sm leading-relaxed text-bone-dim">
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-brass" />
          {m.flag}
        </p>
      ) : preview.isError ? (
        <p role="alert" className="flex items-start gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
          {preview.error instanceof ApiError ? preview.error.message : m.loadFailed}
        </p>
      ) : !said || !ready ? (
        <div className="space-y-3" aria-busy="true">
          <div className="grid gap-3 sm:grid-cols-2">
            <Skeleton className="h-28 w-full" />
            <Skeleton className="h-28 w-full" />
          </div>
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-4/5" />
        </div>
      ) : (
        <div className="space-y-5">
          <div className="grid gap-3 sm:grid-cols-2">
            <Side
              card={said.survivor}
              role="kept"
              note={
                said.proposed === said.survivor.id ? m.rule(m.why[said.why]) : m.chosen
              }
            />
            <Side card={said.merged} role="merged" onSwap={busy ? undefined : swap} />
          </div>

          <section aria-labelledby="merge-fields">
            <h3 id="merge-fields" className="label">
              {m.fields}
            </h3>
            {said.fields.length === 0 ? (
              <p className="mt-2 text-sm text-bone-dim">{m.same}</p>
            ) : (
              <>
                <p className="mt-1 text-xs leading-relaxed text-bone-faint">{m.fieldsHint}</p>
                <ul className="mt-2 divide-y divide-rule rounded-card border border-rule">
                  {said.fields.map((field) => (
                    <Choice
                      key={field.field}
                      field={field}
                      registry={registry.data}
                      side={keep[field.field] ?? 'survivor'}
                      onSide={(side) => setKeep((held) => ({ ...held, [field.field]: side }))}
                    />
                  ))}
                </ul>
              </>
            )}
          </section>

          <p className="text-sm leading-relaxed text-bone-dim">
            {m.what(
              said.merged.title,
              said.survivor.title,
              held(said.children, m, locale),
              fmt.list(
                said.aliases
                  .filter((alias) => alias.source !== 'id' && alias.source !== 'slug')
                  .map((alias) => `${sourceName(alias.source)} ${alias.value}`),
                locale,
              ),
              said.undoDays,
            )}
          </p>

          {merge.isError || dismiss.isError ? (
            <p role="alert" className="flex items-start gap-2 text-sm text-vermillion">
              <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
              {(merge.error ?? dismiss.error) instanceof ApiError
                ? (merge.error ?? dismiss.error)?.message
                : t.common.actionFailed}
            </p>
          ) : null}
        </div>
      )}
    </Dialog>
  )
}

/** What the merged record holds, counted in words: "2 seasons, 24 episodes". */
function held(children: Record<string, number>, m: Dict['admin']['merge'], locale: string): string {
  const words = m.children as Record<string, ((n: number) => string) | undefined>
  const said = Object.entries(children)
    .filter(([, n]) => n > 0)
    .map(([kind, n]) => words[kind]?.(n))
    .filter((text): text is string => Boolean(text))
  return said.length ? fmt.list(said, locale) : m.nothingHeld
}

/** One of the two, as the dialog lays them out: what it is, and what becomes of it. */
function Side({
  card,
  role,
  note,
  onSwap,
}: {
  card: MergeWork
  role: 'kept' | 'merged'
  note?: string
  onSwap?: () => void
}) {
  const { t, locale } = useI18n()
  const m = t.admin.merge
  const ids = card.externalIds

  return (
    <article
      className={cn(
        'flex min-w-0 gap-3 rounded-card border p-3',
        role === 'kept' ? 'border-brass-deep bg-brass/[0.04]' : 'border-rule',
      )}
    >
      <span className="aspect-2/3 w-14 shrink-0 overflow-hidden rounded-[3px] border border-rule bg-ink-high">
        {card.poster ? <Artwork url={card.poster} role="thumb" sizes="56px" alt="" className="size-full object-cover" /> : null}
      </span>
      <div className="min-w-0 flex-1">
        <Chip tone={role === 'kept' ? 'manual' : 'neutral'}>{role === 'kept' ? m.kept : m.mergedInto}</Chip>
        <h3 className="mt-1.5 text-sm font-medium break-words text-bone">
          {card.title}
          {card.year ? <span className="ml-1.5 font-mono text-xs text-bone-faint tabular-nums">{card.year}</span> : null}
        </h3>
        <p className="mt-1 flex flex-wrap gap-x-2 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
          {ids.tvdb ? <span>tvdb:{ids.tvdb}</span> : null}
          {ids.tmdb ? <span>tmdb:{ids.tmdb}</span> : null}
          {ids.imdb ? <span>{ids.imdb}</span> : null}
          {ids.anilist?.length ? <span>anilist:{ids.anilist.join(',')}</span> : null}
          {ids.fankai ? <span>fankai:{ids.fankai}</span> : null}
        </p>
        <p className="mt-1 text-xs text-bone-dim">
          {m.handEdits(card.handEdits)} · {m.added(fmt.relative(card.createdAt, locale) ?? '')}
          {card.isManual ? ` · ${t.work.manualEntry}` : ''}
        </p>
        {note ? <p className="mt-1 text-xs text-brass">{note}</p> : null}
        {onSwap ? (
          <Button size="sm" variant="quiet" className="-ml-3 mt-1" onClick={onSwap}>
            <Glyph name="shuffle" className="size-3.5" />
            {m.swap}
          </Button>
        ) : null}
      </div>
    </article>
  )
}

/** One field whose value differs: the two values, and which is kept. */
function Choice({
  field,
  registry,
  side,
  onSide,
}: {
  field: MergeField
  registry?: FieldRegistry
  side: MergeSide
  onSide: (side: MergeSide) => void
}) {
  const { t } = useI18n()
  const m = t.admin.merge
  const words = { yes: t.common.yes, no: t.common.no }
  const def = registry?.item.find((one) => one.name === field.field)
  const name = def ? fieldLabel(def, t) : ((t.labels.fields as Record<string, string>)[field.field] ?? field.field)
  const group = `merge-${field.field}`

  const option = (which: MergeSide, value: unknown, locked: boolean, disabled: boolean) => (
    <label
      className={cn(
        'flex min-h-11 min-w-0 cursor-pointer items-start gap-2.5 rounded-[6px] px-2 py-2 text-sm',
        'has-[:checked]:bg-ink-high has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-50',
      )}
    >
      <input
        type="radio"
        name={group}
        className="mt-0.5 size-4 shrink-0 accent-vermillion"
        checked={side === which}
        disabled={disabled}
        onChange={() => onSide(which)}
      />
      <span className="min-w-0">
        <span className="line-clamp-3 break-words text-bone">
          {shown(value, words) ?? <i className="text-bone-faint">{t.admin.editor.journal.nothing}</i>}
        </span>
        {locked ? (
          <span className="mt-0.5 flex items-center gap-1 text-xs text-brass">
            <Glyph name="lock" className="size-3" />
            {m.locked}
          </span>
        ) : null}
        {disabled ? <span className="mt-0.5 block text-xs text-bone-faint">{m.notChoosable}</span> : null}
      </span>
    </label>
  )

  return (
    <li className="px-3 py-2">
      <fieldset>
        <legend className="text-xs font-medium text-bone-dim">{name}</legend>
        <div className="mt-1 grid gap-1 sm:grid-cols-2">
          {option('survivor', field.survivor, field.survivorLocked, false)}
          {option('merged', field.merged, field.mergedLocked, !field.choosable)}
        </div>
      </fieldset>
    </li>
  )
}

/** The other record, found by its title among the works of the same kind. */
function Picker({ work, onPick }: { work: MergeOpening; onPick: (id: string) => void }) {
  const { t } = useI18n()
  const m = t.admin.merge
  const [term, setTerm] = useState('')
  const settled = useSettled(term.trim())

  const found = useQuery({
    queryKey: ['items', 'merge-pick', work.kind, settled],
    queryFn: () =>
      api.get<ItemPage>(
        `/items${query({ term: settled, kind: work.kind, includeDisabled: true, limit: 8 })}`,
      ),
    enabled: settled.length >= 2,
  })
  const others = (found.data?.items ?? []).filter((item) => item.id !== work.id)

  return (
    <div className="space-y-3">
      <p className="text-sm leading-relaxed text-bone-dim">{m.pickLead(work.title)}</p>
      <div>
        <label htmlFor="merge-pick" className="label mb-1.5 block">
          {m.pick}
        </label>
        <Input
          id="merge-pick"
          type="search"
          autoFocus
          autoComplete="off"
          value={term}
          onChange={(event) => setTerm(event.target.value)}
        />
      </div>
      {found.isFetching ? <Spinner className="size-4 text-bone-faint" /> : null}
      {settled.length >= 2 && found.isSuccess && others.length === 0 ? (
        <p className="text-sm text-bone-faint">{m.noMatch}</p>
      ) : null}
      {others.length ? (
        <ul className="divide-y divide-rule rounded-card border border-rule">
          {others.map((item) => (
            <li key={item.id}>
              <button
                type="button"
                onClick={() => onPick(item.id)}
                className="flex min-h-11 w-full cursor-pointer items-center gap-3 px-3 py-2 text-left text-sm text-bone transition-colors duration-150 hover:bg-ink-high"
              >
                <span className="min-w-0 flex-1 truncate">
                  {item.title}
                  {item.year ? <span className="ml-1.5 font-mono text-xs text-bone-faint tabular-nums">{item.year}</span> : null}
                </span>
                <span className="shrink-0 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                  {item.externalIds.tvdb ? `tvdb:${item.externalIds.tvdb} ` : ''}
                  {item.externalIds.tmdb ? `tmdb:${item.externalIds.tmdb}` : ''}
                </span>
                <Glyph name="chevronRight" className="size-3.5 shrink-0 text-bone-faint" />
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}
