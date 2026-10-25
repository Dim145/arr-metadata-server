/**
 * Settings, drawn from the server's own registry.
 *
 * Nothing here knows what `refresh.batchSize` is. The server says which
 * settings exist, what each one may hold and where it may be said, and this
 * turns that into controls — so a setting added to the server appears on this
 * screen without this file being touched. The dictionary carries a title and a
 * sentence for the keys we already know about; one we do not stands under its
 * own name, in mono, in the group its prefix puts it in.
 *
 * The same rows serve the server scope and a single client's, because they are
 * the same decision at two distances: the only difference is that a client can
 * decline to answer and take whatever the server said.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Link } from 'react-router'

import { ApiError, api } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n, type Dict } from '../lib/i18n'
import type {
  EffectiveSetting,
  ListImport,
  SettingDef,
  SettingScope,
  Settings as Config,
} from '../lib/types'
import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  Field,
  Glyph,
  Input,
  Panel,
  PanelHead,
  Select,
  Skeleton,
  Spinner,
  Toggle,
} from './ui'

/** What a scope is addressed by. The server has no id; the route wants `-`. */
export type Scope = { scope: SettingScope; id: string }

export const SERVER: Scope = { scope: 'server', id: '-' }

/**
 * Which panel a key belongs under, by its prefix.
 *
 * Deriving the group rather than listing the keys is the point: a second
 * refresh setting lands beside the first one on the day the server grows it,
 * and only a genuinely new area of the server falls through to "everything
 * else" — which is a prompt to name it, not a failure.
 */
export const GROUPS = [
  { id: 'answering', prefixes: ['tmdb'] },
  { id: 'providers', prefixes: ['skyhook', 'radarr', 'sonarr', 'tvdb'] },
  { id: 'sources', prefixes: ['tvmaze', 'anilist', 'mal', 'imdb', 'fankai'] },
  { id: 'refresh', prefixes: ['refresh'] },
  // Kept on the Opening & APIs page, where each is explained with what it
  // governs; never drawn among the rest.
  { id: 'access', prefixes: ['keys', 'site', 'registration', 'api', 'oidc', 'auth'] },
  // Kept on the Media page, beside what they govern.
  { id: 'media', prefixes: ['media'] },
  // Kept on the Cache page, one switch per space.
  { id: 'cache', prefixes: ['cache'] },
  { id: 'adult', prefixes: ['adult'] },
  // The schedule, what is kept, and the archives made before an upgrade;
  // the archives themselves are on the Backups page.
  { id: 'backup', prefixes: ['backup'] },
  // Where the errors are reported: a Sentry's or a GlitchTip's DSN.
  { id: 'errors', prefixes: ['sentry'] },
  // Kept in a panel of their own on the Settings page: the events are
  // ticked rather than typed, and the address can be tested.
  { id: 'webhooks', prefixes: ['webhooks'] },
  // Kept on the Sources page, where the columns are put in order.
  { id: 'order', prefixes: ['sources'] },
  // Kept on the Sonarr & Radarr page, where an address is tested before it
  // is kept and a key goes one way.
  { id: 'arr', prefixes: ['arr'] },
] as const

export type GroupId = (typeof GROUPS)[number]['id'] | 'other'

export function groupOf(key: string): GroupId {
  const prefix = key.split('.')[0] ?? ''
  return GROUPS.find((group) => group.prefixes.some((p) => p === prefix))?.id ?? 'other'
}

/**
 * The dictionary indexed by a key the dictionary may not have.
 *
 * Widened on purpose: the literal type is what makes French compulsory for
 * every key we do know, and this is the one place that has to survive asking
 * for one we do not.
 */
function named(t: Dict) {
  const names: Record<string, string | undefined> = t.settings.names
  const hints: Record<string, string | undefined> = t.settings.hints
  const choices: Record<string, string | undefined> = t.settings.choices

  return { names, hints, choices }
}

export function useRegistry() {
  return useQuery({
    queryKey: ['settings', 'registry'],
    queryFn: () => api.get<SettingDef[]>('/settings/registry'),
    // The registry only changes when the binary does.
    staleTime: 60 * 60_000,
  })
}

function useValues({ scope, id }: Scope) {
  return useQuery({
    queryKey: ['settings', 'scope', scope, id],
    queryFn: () => api.get<EffectiveSetting[]>(`/settings/${scope}/${id}`),
  })
}

/** What the server scope holds, as the rows read it — one query, shared with the page that searches them. */
export function useServerValues() {
  return useValues(SERVER)
}

/**
 * Lower-cased and stripped of its accents, so that a search finds what it
 * means — and the ligatures, which no accent can be taken from: "oeuvres" is
 * the "œuvres" it was typed for.
 */
function folded(text: string): string {
  return text
    .normalize('NFD')
    .replace(/\p{M}/gu, '')
    .toLocaleLowerCase()
    .replace(/œ/g, 'oe')
    .replace(/æ/g, 'ae')
}

/**
 * Whether a setting answers to what was typed: each word of it is found in the
 * key, the title, the sentence that explains it or the name of the group it
 * stands under. Nothing typed, everything answers.
 */
export function settingMatches(def: SettingDef, t: Dict, term: string): boolean {
  const words = folded(term).split(/\s+/).filter(Boolean)
  if (!words.length) return true

  const { names, hints } = named(t)
  const haystack = folded(
    [def.key, names[def.key], hints[def.key], t.settings.groups[groupOf(def.key)]].filter(Boolean).join(' '),
  )
  return words.every((word) => haystack.includes(word))
}

/**
 * Whether a server setting is other than it began.
 *
 * Measured against the value the server says it began with — what the
 * environment or the code gave it when the server started. One that begins
 * unset is changed once it is set. A secret has nothing to be measured by:
 * what it holds is never read back.
 */
export function isChanged(def: SettingDef, state: EffectiveSetting | undefined): boolean {
  if (!state || def.kind.type === 'secret') return false
  return state.default !== undefined ? state.value !== state.default : state.overridden
}

/**
 * A value as a person reads it: a switch as on or off, a choice by its words,
 * nothing as "not set".
 */
function shownValue(def: SettingDef, value: string | undefined, t: Dict): string {
  if (value === undefined || value === '') return t.common.notSet
  if (def.kind.type === 'bool') {
    return (value === 'true' ? t.admin.config.enabled : t.admin.config.disabled).toLocaleLowerCase()
  }
  if (def.kind.type === 'choice') return named(t).choices[`${def.key}.${value}`] ?? value
  return value
}

/* ── The server's own ─────────────────────────────────────────────────────── */

/**
 * Everything settable server-wide, one panel per group — or, under a search,
 * only the settings that answer to it, and only the groups that still have one.
 */
export function ServerSettings({
  delay = 0,
  filter = '',
  changedOnly = false,
}: {
  delay?: number
  /** What was typed to find a setting by. */
  filter?: string
  /** Only those other than they began. */
  changedOnly?: boolean
}) {
  const { t } = useI18n()

  const registry = useRegistry()
  const values = useValues(SERVER)

  if (registry.isPending || values.isPending) {
    return <Skeleton className="h-64 w-full" />
  }

  if (registry.isError || values.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {t.settings.loadFailed}
      </p>
    )
  }

  const here = registry.data.filter((def) => def.scopes.includes('server'))
  const groups: GroupId[] = ['answering', 'providers', 'sources', 'refresh', 'adult', 'backup', 'errors', 'other']

  return (
    <div className="space-y-6">
      {groups.map((group, index) => {
        const defs = here.filter(
          (def) =>
            groupOf(def.key) === group &&
            settingMatches(def, t, filter) &&
            (!changedOnly || isChanged(def, values.data.find((value) => value.key === def.key))),
        )
        if (!defs.length) return null

        const title = t.settings.groups[group]

        return (
          <Panel
            key={group}
            id={`settings-${group}`}
            label={title}
            className="rise"
            style={{ animationDelay: `${delay + index * 40}ms` }}
          >
            <PanelHead title={title} />
            <ul>
              {defs.map((def) => (
                <li key={def.key} className="border-b border-rule px-5 py-4 last:border-0">
                  <SettingRow
                    def={def}
                    at={SERVER}
                    state={values.data.find((value) => value.key === def.key)}
                  />
                </li>
              ))}
            </ul>
            {group === 'sources' && !filter && !changedOnly ? <SourcesAtWork values={values.data} /> : null}
          </Panel>
        )
      })}
    </div>
  )
}

/**
 * What the further sources are working from, under the switches that start it.
 *
 * Switching a source on is where an operator wonders whether it took: the
 * lists it needs arrive within a minute, and this says when they did. Asked
 * again every few seconds only while a source is on and its list has not
 * landed yet.
 */
function SourcesAtWork({ values }: { values: EffectiveSetting[] }) {
  const { t } = useI18n()

  const on = (key: string) => values.find((value) => value.key === key)?.value === 'true'
  const wantsAnime = on('anilist.enabled') || on('mal.enabled')
  const wantsImdb = on('imdb.enabled')

  const summary = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Config>('/settings'),
    staleTime: 5 * 60_000,
    refetchInterval: (query) => {
      const further = query.state.data?.furtherSources
      const waiting =
        (wantsAnime && !further?.animeList) || (wantsImdb && !further?.imdbRatings)
      return waiting ? 5_000 : false
    },
  })

  const further = summary.data?.furtherSources
  if (!further) return null

  return (
    <div className="border-t border-rule">
      <dl className="divide-y divide-rule">
        <Field label={t.admin.config.animeList}>
          <span className="inline-flex flex-wrap items-center justify-end gap-x-3 gap-y-1">
            <Imported list={further.animeList} rows={t.admin.config.animeEntries} />
            {wantsAnime ? <ImportNow name="anime-lists" /> : null}
          </span>
        </Field>
        <Field label={t.admin.config.imdbRatings}>
          <span className="inline-flex flex-wrap items-center justify-end gap-x-3 gap-y-1">
            <Imported list={further.imdbRatings} rows={t.admin.config.imdbWorks} />
            {wantsImdb ? <ImportNow name="imdb-ratings" /> : null}
          </span>
        </Field>
        <Field label={t.admin.config.malVia}>
          {further.malVia === 'official' ? (
            <Chip tone="provider">
              <Glyph name="check" className="size-3" />
              {t.admin.config.malOfficial}
            </Chip>
          ) : (
            <Chip>{t.admin.config.malJikan}</Chip>
          )}
        </Field>
      </dl>
      <p className="border-t border-rule px-5 py-4 text-xs leading-relaxed text-bone-faint">
        {t.admin.config.furtherHint} {t.admin.config.furtherJobs}{' '}
        <Link
          to="/admin/jobs"
          className="text-bone-dim underline decoration-rule-bright underline-offset-2 transition-colors duration-150 hover:text-bone"
        >
          {t.admin.jobs}
        </Link>
        .
      </p>
    </div>
  )
}

/**
 * Download a list now rather than when the schedule next says — after
 * switching a source on, or to check a list is not stale.
 */
function ImportNow({ name }: { name: 'anime-lists' | 'imdb-ratings' }) {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const run = useMutation({
    // No summary when the server answered before the download finished: it
    // carries on, and records itself as a job.
    mutationFn: () => api.post<{ summary?: string }>(`/datasets/${name}/import`),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ['settings'], exact: true })
      void queryClient.invalidateQueries({ queryKey: ['jobs'] })
    },
  })

  // A refusal says why in words worth showing — another import running, the
  // source switched off. A failed download does not: its reason can name
  // hosts, so it is in the job run, and this says where to read it.
  const refusal =
    run.error instanceof ApiError && run.error.status === 409 ? run.error.message : undefined

  return (
    <span className="inline-flex flex-col items-end gap-1">
      <Button size="sm" variant="quiet" disabled={run.isPending} onClick={() => run.mutate()}>
        {run.isPending ? <Spinner className="size-3.5" /> : <Glyph name="download" className="size-3.5" />}
        {run.isPending ? t.admin.config.importing : t.admin.config.importNow}
      </Button>
      {/* Always there, so what appears in it is read out: a live region
          inserted together with its text often is not. */}
      <span role="status" className={cn('text-xs', run.data?.summary ? 'text-moss' : 'text-bone-dim')}>
        {run.isSuccess ? (run.data.summary ?? t.admin.config.importRunning) : ''}
      </span>
      {run.isError ? (
        <span role="alert" className="text-xs text-vermillion">
          {refusal ?? t.admin.config.importFailed}
        </span>
      ) : null}
    </span>
  )
}

/** A downloaded list: how much of it is kept, and how long ago it landed. */
function Imported({ list, rows }: { list: ListImport | null; rows: (n: string) => string }) {
  const { t, locale } = useI18n()

  if (!list) {
    return <span className="text-bone-faint">{t.admin.config.notImported}</span>
  }

  return (
    <span className="font-mono text-[0.8125rem] tabular-nums" title={fmt.dateTime(list.importedAt, locale)}>
      {rows(fmt.count(list.rows, locale))} · {fmt.relative(list.importedAt, locale)}
    </span>
  )
}

/* ── One client's ─────────────────────────────────────────────────────────── */

/**
 * What one client answers differently.
 *
 * A dialog rather than an expanding row: the three keys that can differ per
 * client are a form, and a form unfolding inside a table row pushes everything
 * below it down the page while you read it.
 */
export function ScopeSettingsDialog({
  at,
  who,
  onClose,
}: {
  at: Scope | null
  who: string
  onClose: () => void
}) {
  const { t } = useI18n()

  return (
    <Dialog
      open={Boolean(at)}
      title={t.settings.openFor(who)}
      onClose={onClose}
      footer={<Button onClick={onClose}>{t.nav.close}</Button>}
    >
      {at ? <ScopeSettings at={at} /> : null}
    </Dialog>
  )
}

function ScopeSettings({ at }: { at: Scope }) {
  const { t } = useI18n()

  const registry = useRegistry()
  const values = useValues(at)

  if (registry.isPending || values.isPending) {
    return (
      <div className="space-y-3">
        {Array.from({ length: 3 }, (_, index) => (
          <Skeleton key={index} className="h-12 w-full" />
        ))}
      </div>
    )
  }

  if (registry.isError || values.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {t.settings.loadFailed}
      </p>
    )
  }

  const here = registry.data.filter((def) => def.scopes.includes(at.scope))

  if (!here.length) {
    return <EmptyState title={t.settings.empty} />
  }

  return (
    <>
      <p className="max-w-prose text-xs leading-relaxed text-bone-faint">{t.settings.scopeLead}</p>
      <ul className="mt-2">
        {here.map((def) => (
          <li key={def.key} className="border-b border-rule py-4 last:border-0 last:pb-0">
            <SettingRow
              def={def}
              at={at}
              state={values.data.find((value) => value.key === def.key)}
            />
          </li>
        ))}
      </ul>
    </>
  )
}

/* ── One setting ──────────────────────────────────────────────────────────── */

/** What a kind holds when nothing anywhere has said otherwise. */
function fallback(def: SettingDef): string {
  switch (def.kind.type) {
    case 'bool':
      return 'false'
    case 'int':
      return String(def.kind.min)
    case 'choice':
      return def.kind.options[0] ?? ''
    case 'text':
    case 'secret':
      return ''
  }
}

function SettingRow({
  def,
  at,
  state,
}: {
  def: SettingDef
  at: Scope
  state: EffectiveSetting | undefined
}) {
  const { t } = useI18n()
  const queryClient = useQueryClient()
  const { names, hints } = named(t)

  const value = state?.value ?? fallback(def)
  const overridden = state?.overridden ?? false

  // The server scope is where a value ends up; it has nothing above it to
  // inherit from, so it never offers to.
  const inheritable = at.scope !== 'server'
  const editable = !inheritable || overridden

  const save = useMutation({
    mutationFn: (next: string | null) =>
      api.put<EffectiveSetting[]>(`/settings/${at.scope}/${at.id}`, { key: def.key, value: next }),
    onSuccess: () => {
      // Refetched rather than written into the cache from the response. Each
      // row owns its own mutation, and the response carries the *whole*
      // collection as it stood when the server computed it — so flipping two
      // switches quickly and having the first response land last put the
      // second switch visibly back where it was, while the server had it
      // right. Asking again cannot be out of order in a way that sticks.
      void queryClient.invalidateQueries({ queryKey: ['settings', 'scope', at.scope, at.id] })
      // The read-only summary and the sidebar's version line read the same
      // configuration from a different route.
      void queryClient.invalidateQueries({ queryKey: ['settings'], exact: true })
      // The footer credits whichever sources are on.
      void queryClient.invalidateQueries({ queryKey: ['sources'] })
    },
  })

  const id = `setting-${at.scope}-${def.key.replace(/\W/g, '-')}`
  const name = names[def.key]
  const hint = hints[def.key]
  // Said of the server's own only: a key's or an address's is told by whether
  // it sets the setting here or inherits it.
  const changed = !inheritable && isChanged(def, state)

  // The server answers `{key} {why}`; the key is already the title of this row.
  const message = save.error instanceof ApiError ? save.error.message : save.error?.message
  const error = message?.startsWith(`${def.key} `) ? message.slice(def.key.length + 1) : message

  return (
    <div>
      {/* The control follows the sentence that explains it and the provenance
          follows both: in a dialog nothing fits side by side, and a reader who
          meets "inherited" before the value it describes has to go back. */}
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
        <div className="min-w-40 flex-1">
          <label htmlFor={id} id={`${id}-label`} className="block text-sm font-medium text-bone">
            {name ?? def.key}
          </label>
          {name ? (
            <p className="mt-0.5 font-mono text-[0.6875rem] text-bone-faint">{def.key}</p>
          ) : null}
          {hint ? (
            <p className="mt-1.5 max-w-prose text-xs leading-relaxed text-bone-faint">{hint}</p>
          ) : null}
        </div>

        <div className="flex shrink-0 items-center gap-2">
          {save.isPending ? <Spinner className="size-3.5 text-bone-faint" /> : null}
          {save.isSuccess ? (
            <span className="flex items-center gap-1 text-xs text-moss">
              <Glyph name="check" className="size-3.5" />
              {t.settings.saved}
            </span>
          ) : null}

          <Control
            id={id}
            def={def}
            value={value}
            label={name ?? def.key}
            disabled={!editable || save.isPending}
            onCommit={(next) => {
              if (next !== value) save.mutate(next)
            }}
            onEdit={() => {
              if (save.isSuccess || save.isError) save.reset()
            }}
          />
        </div>
      </div>

      {changed ? (
        <div className="mt-2.5 flex flex-wrap items-center gap-x-3 gap-y-1.5">
          {/* Brass: a person decided this. */}
          <Chip tone="manual">
            <Glyph name="pencil" className="size-3" />
            {t.settings.modified}
          </Chip>
          <span className="text-xs text-bone-faint">{t.settings.defaultIs(shownValue(def, state?.default, t))}</span>
          {/* Back to what it began as: that value written, or — for one that
              begins unset — the setting cleared. The row it belongs to is
              what names it to a screen reader, so no two share a name. */}
          <Button
            size="sm"
            variant="quiet"
            disabled={save.isPending}
            aria-describedby={`${id}-label`}
            onClick={() => save.mutate(state?.default ?? null)}
          >
            <Glyph name="arrowLeft" className="size-3.5" />
            {t.settings.revert}
          </Button>
        </div>
      ) : null}

      {inheritable ? (
        <div className="mt-2.5 flex flex-wrap items-center gap-x-3 gap-y-1.5">
          <Chip tone={overridden ? 'manual' : 'neutral'}>
            {overridden ? (
              <Glyph name="pencil" className="size-3" />
            ) : state ? (
              <Glyph name="link" className="size-3" />
            ) : null}
            {overridden ? t.settings.setHere : state ? t.settings.inherited : t.common.notSet}
          </Chip>
          <Button
            size="sm"
            variant="quiet"
            disabled={save.isPending}
            onClick={() => save.mutate(overridden ? null : value)}
          >
            {overridden ? t.settings.stopOverriding : t.settings.override}
          </Button>
          {!overridden && state ? (
            <span className="text-xs text-bone-faint">{t.settings.inheritedHint}</span>
          ) : null}
        </div>
      ) : null}

      {error ? (
        <p role="alert" className="mt-2 flex items-start gap-1.5 text-xs text-vermillion">
          <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
          {error}
        </p>
      ) : null}
    </div>
  )
}

/**
 * The control a kind deserves.
 *
 * A flag and a choice commit as they change, because the change is the whole
 * gesture. Anything typed waits for the field to be left: saving per keystroke
 * would write `1`, `19`, `190` on the way to `1900` and refuse two of them.
 */
function Control({
  id,
  def,
  value,
  label,
  disabled,
  onCommit,
  onEdit,
}: {
  id: string
  def: SettingDef
  value: string
  label: string
  disabled?: boolean
  onCommit: (next: string) => void
  onEdit: () => void
}) {
  const { t } = useI18n()
  const { choices } = named(t)

  const [draft, setDraft] = useState(value)

  // The stored value moves under the field when a write is refused and the
  // server hands back what it kept, or when another scope's dialog is opened.
  useEffect(() => setDraft(value), [value])

  if (def.kind.type === 'bool') {
    return (
      <Toggle
        id={id}
        label={label}
        checked={value === 'true'}
        disabled={disabled}
        onChange={(next) => {
          onEdit()
          onCommit(String(next))
        }}
      />
    )
  }

  if (def.kind.type === 'choice') {
    return (
      <Select
        id={id}
        className="w-52"
        value={value}
        disabled={disabled}
        onChange={(event) => {
          onEdit()
          onCommit(event.target.value)
        }}
      >
        {def.kind.options.map((option) => (
          <option key={option} value={option}>
            {choices[`${def.key}.${option}`] ?? option}
          </option>
        ))}
      </Select>
    )
  }

  const kind = def.kind
  const suggestions = SUGGESTIONS[def.key]

  return (
    <>
    <Input
      id={id}
      className={kind.type === 'int' ? 'w-28 text-right' : 'w-52'}
      value={draft}
      list={suggestions ? `${id}-suggestions` : undefined}
      disabled={disabled}
      inputMode={kind.type === 'int' ? 'numeric' : undefined}
      min={kind.type === 'int' ? kind.min : undefined}
      max={kind.type === 'int' ? kind.max : undefined}
      onChange={(event) => {
        onEdit()
        setDraft(event.target.value)
      }}
      onBlur={() => onCommit(draft.trim())}
      // Enter is how a form is submitted, and this field is not in one.
      onKeyDown={(event) => {
        if (event.key === 'Enter') event.currentTarget.blur()
      }}
    />
    {suggestions ? (
      <datalist id={`${id}-suggestions`}>
        {suggestions.map((value) => (
          <option key={value} value={value} />
        ))}
      </datalist>
    ) : null}
    </>
  )
}

/**
 * Values worth offering for a free-text setting, as the browser's own list.
 *
 * The language is a TMDB tag, and a field that took anything took `fr_FR` and
 * `french` just as happily — the server refuses neither, and TMDB answers both
 * in English. Suggestions, not a menu: TMDB serves more tags than are listed.
 */
const SUGGESTIONS: Record<string, readonly string[]> = {
  'tmdb.language': [
    'en-US', 'en-GB', 'fr-FR', 'fr-CA', 'de-DE', 'es-ES', 'es-MX', 'it-IT', 'pt-BR', 'pt-PT',
    'nl-NL', 'sv-SE', 'da-DK', 'nb-NO', 'fi-FI', 'pl-PL', 'cs-CZ', 'hu-HU', 'ru-RU', 'uk-UA',
    'tr-TR', 'el-GR', 'he-IL', 'ar-SA', 'hi-IN', 'ja-JP', 'ko-KR', 'zh-CN', 'zh-TW', 'th-TH',
  ],
}
