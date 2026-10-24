/**
 * Who made the work and what else it is called: the credits, the other
 * titles Sonarr and Radarr match release names against, and the languages
 * the sources describe it in.
 *
 * Every row says whose it is in a word as well as a colour: a brass edge and
 * "yours" for one a person added, which is also the only kind that can be
 * taken away here — a provider's would simply return at the next refresh.
 * A provider's is hidden instead, and a credit corrected — the character,
 * the place — each a lock the next refresh keeps: a hidden row stays in the
 * list, faded, with "hidden" in brass, to be shown again.
 *
 * Each credit names the person it is filed under: their page from the name,
 * their record from the mark beside it. A credit by name alone — an actor
 * typed in here, a guest TheTVDB lists — is filed under somebody from its
 * row, found by a part of their name.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import { Artwork } from '../../../components/media'
import {
  Button,
  Chip,
  FormField,
  Glyph,
  Input,
  Panel,
  PanelHead,
  Segmented,
  Select,
  Spinner,
  useTooltip,
} from '../../../components/ui'
import { api, segment } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import { useI18n } from '../../../lib/i18n'
import { jobLabel, providerName, titleOrigin } from '../../../lib/labels'
import type { Credit, MediaItem, PersonCard, ProvenanceReport } from '../../../lib/types'

import { PersonPicker, afterPeopleWrite } from '../People'
import { creditScope, locksOn, useEditorLocks } from './locks'
import { AddForm, Count, Empty, Yours, useRemove } from './shared'

type CastFilter = 'all' | 'cast' | 'crew' | 'yours'

/** The sources api.radarr.video files a title under, where others file its kind. */
const RADARR_SOURCES = new Set(['tmdb', 'mappings', 'user', 'indexer'])

export function PeopleTab({ work, report, onChanged }: { work: MediaItem; report?: ProvenanceReport; onChanged: () => void }) {
  const { ask, dialog } = useRemove(work, onChanged)

  return (
    <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1.25fr)_minmax(0,1fr)] lg:items-start">
      <Credits work={work} onChanged={onChanged} onRemove={ask} />
      <div className="space-y-6">
        <AlternativeTitles work={work} onChanged={onChanged} onRemove={ask} />
        <Translations work={work} report={report} />
      </div>
      {dialog}
    </div>
  )
}

type PanelProps = {
  work: MediaItem
  onChanged: () => void
  onRemove: (target: { path: string; label: string }) => void
}

/* ── Credits ──────────────────────────────────────────────────────────────── */

function Credits({ work, onChanged, onRemove }: PanelProps) {
  const { t, lang } = useI18n()
  const c = t.admin.editor.children
  const e = t.admin.editor
  const p = t.admin.peoplePage
  const [filter, setFilter] = useState<CastFilter>('all')
  const [name, setName] = useState('')
  const [character, setCharacter] = useState('')
  const [role, setRole] = useState('actor')
  const [image, setImage] = useState('')

  const add = useMutation({
    mutationFn: (body: unknown) => api.post<{ id: string }>(`/items/${work.id}/credits`, body),
    onSuccess: () => {
      setName('')
      setCharacter('')
      setImage('')
      onChanged()
    },
  })

  // Hidden, corrected: a lock on the credit, set by the row it is read in.
  const locks = useEditorLocks()
  const [correcting, setCorrecting] = useState<string | null>(null)
  // A credit by name alone being filed under somebody, and whom the last one went to.
  const [filing, setFiling] = useState<string | null>(null)
  const [filed, setFiled] = useState<string | null>(null)
  const people = work.people?.byCredit ?? {}
  const edit = useMutation({
    mutationFn: ({ id, body }: { id: string; body: Record<string, unknown> }) => api.patch(`/items/${work.id}/credits/${id}`, body),
    onSuccess: () => {
      setCorrecting(null)
      onChanged()
    },
  })

  // What a person hid stays in the list, faded, to be shown again.
  const hidden = new Set((work.hidden?.credits ?? []).map((credit) => credit.id))
  const credits = [...(work.credits ?? []), ...(work.hidden?.credits ?? [])].sort((x, y) => x.sortOrder - y.sortOrder)
  const shown = credits.filter((credit) =>
    filter === 'all' ? true : filter === 'cast' ? credit.creditType === 'actor' : filter === 'crew' ? credit.creditType !== 'actor' : credit.isManual,
  )
  const roles = [
    ['actor', c.actor],
    ['director', c.director],
    ['writer', c.writer],
    ['producer', c.producer],
    ['guest', c.guest],
  ] as const

  const castFilter = (
    <Segmented<CastFilter>
      label={e.filter.label}
      value={filter}
      onChange={setFilter}
      options={[
        { value: 'all', label: e.filter.all },
        { value: 'cast', label: e.filter.cast },
        { value: 'crew', label: e.filter.crew },
        { value: 'yours', label: e.filter.yours },
      ]}
    />
  )

  return (
    <Panel id="credits" label={c.credits} className="scroll-mt-28 lg:scroll-mt-16">
      {/* The filter beside the head where there is room, and under it on a
          phone, where the two shared one line badly. */}
      <PanelHead
        title={
          <span className="flex items-center gap-2">
            {c.credits}
            <Count>{credits.length}</Count>
          </span>
        }
        action={credits.length ? <div className="hidden sm:block">{castFilter}</div> : undefined}
      />
      {credits.length ? <div className="border-b border-rule px-5 py-2 sm:hidden">{castFilter}</div> : null}

      {shown.length === 0 ? (
        <Empty>{credits.length ? e.filter.none : e.peopleTab.noCredits}</Empty>
      ) : (
        <ul className="divide-y divide-rule">
          {shown.map((credit) => {
            const face = credit.image ? (
              <Artwork url={credit.image} role="headshot" alt="" className="size-10 shrink-0 rounded-full border border-rule object-cover" />
            ) : (
              <span className="grid size-10 shrink-0 place-items-center rounded-full border border-rule bg-ink-high">
                <Glyph name="user" className="size-4 text-bone-faint" />
              </span>
            )
            const isHidden = hidden.has(credit.id)
            const corrected = !credit.isManual && locksOn(locks, creditScope(credit)).some((lock) => lock.field !== 'hidden')
            const busy = edit.isPending && edit.variables?.id === credit.id
            // The person it is filed under: their page by TMDB's id, as it
            // always was, or by their record where TMDB knows them not.
            const personId = people[credit.id]?.personId
            const page = credit.tmdbPersonId
              ? `/person/${credit.tmdbPersonId}`
              : personId
                ? `/people/${encodeURIComponent(personId)}`
                : undefined
            return (
              <li
                key={credit.id}
                data-credit={credit.personName}
                className={cn(
                  'px-5 py-2 transition-colors duration-150 hover:bg-ink-high',
                  (credit.isManual || isHidden || corrected) && 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]',
                )}
              >
                <div className="flex items-center gap-3">
                  {/* The name is the link; a second one on the portrait would
                      be a stop with nothing to read. Faded when hidden: the
                      marks and the way back stay as they are. */}
                  <span className={cn('contents', isHidden && '[&>*]:opacity-60')}>
                    {face}
                    <span className="min-w-0 flex-1">
                      <span className="flex min-w-0 items-center gap-1.5">
                        <span className={cn('truncate text-sm font-medium text-bone', isHidden && 'line-through decoration-bone-faint')}>
                          {page ? (
                            <Link to={page} className="transition-colors duration-150 hover:text-vermillion">
                              {credit.personName}
                            </Link>
                          ) : (
                            credit.personName
                          )}
                        </span>
                        {personId ? <RecordLink id={personId} name={credit.personName} /> : null}
                      </span>
                      <span className="block truncate text-xs text-bone-faint">
                        {credit.creditType === 'actor'
                          ? credit.characterName
                            ? c.as(credit.characterName)
                            : ''
                          : jobLabel(credit.characterName ?? credit.creditType, lang)}
                      </span>
                    </span>
                    <span className="hidden shrink-0 font-mono text-[0.6875rem] tracking-[0.08em] text-slate uppercase sm:inline">
                      {roles.find(([value]) => value === credit.creditType)?.[1] ?? credit.creditType}
                    </span>
                  </span>
                  {isHidden ? (
                    <Chip tone="manual">
                      <Glyph name="lock" className="size-3" />
                      {c.hidden}
                    </Chip>
                  ) : corrected ? (
                    <Chip tone="manual">
                      <Glyph name="lock" className="size-3" />
                      {c.corrected}
                    </Chip>
                  ) : null}
                  {isHidden ? null : (
                    <button
                      type="button"
                      onClick={() => setCorrecting(correcting === credit.id ? null : credit.id)}
                      aria-expanded={correcting === credit.id}
                      aria-label={c.correctNamed(credit.personName)}
                      title={c.correct}
                      className="grid size-11 shrink-0 cursor-pointer place-items-center rounded-card text-bone-faint transition-colors duration-150 hover:bg-ink-high hover:text-bone"
                    >
                      <Glyph name="pencil" className="size-4" />
                    </button>
                  )}
                  {/* By name alone, the credit is filed under somebody by
                      hand: TMDB's id, where there is one, files it already. */}
                  {isHidden || credit.tmdbPersonId ? null : (
                    <button
                      type="button"
                      id={`file-${credit.id}`}
                      onClick={() => setFiling(filing === credit.id ? null : credit.id)}
                      aria-expanded={filing === credit.id}
                      aria-label={p.fileUnder(credit.personName)}
                      title={p.fileUnder(credit.personName)}
                      className="grid size-11 shrink-0 cursor-pointer place-items-center rounded-card text-bone-faint transition-colors duration-150 hover:bg-ink-high hover:text-bone"
                    >
                      <Glyph name="link" className="size-4" />
                    </button>
                  )}
                  {credit.isManual ? (
                    <Yours onRemove={() => onRemove({ path: `credits/${credit.id}`, label: credit.personName })} />
                  ) : (
                    <Button
                      size="sm"
                      variant={isHidden ? undefined : 'quiet'}
                      disabled={busy}
                      aria-label={isHidden ? c.showNamed(credit.personName) : c.hideNamed(credit.personName)}
                      onClick={() => edit.mutate({ id: credit.id, body: { hidden: !isHidden } })}
                    >
                      {busy ? <Spinner className="size-4" /> : <Glyph name={isHidden ? 'unlock' : 'lock'} className="size-4" />}
                      {isHidden ? c.showAgain : c.hide}
                    </Button>
                  )}
                </div>
                {correcting === credit.id ? (
                  <Correction
                    credit={credit}
                    corrected={corrected}
                    pending={busy}
                    error={edit.isError && edit.variables?.id === credit.id ? edit.error : null}
                    onSave={(body) => edit.mutate({ id: credit.id, body })}
                    onCancel={() => setCorrecting(null)}
                  />
                ) : null}
                {filing === credit.id ? (
                  <FileUnder
                    credit={credit}
                    current={personId}
                    onFiled={(name) => {
                      setFiling(null)
                      setFiled(name)
                      onChanged()
                      // The box it was found in is gone: the focus goes back to the row's button.
                      requestAnimationFrame(() => document.getElementById(`file-${credit.id}`)?.focus())
                    }}
                    onCancel={() => {
                      setFiling(null)
                      requestAnimationFrame(() => document.getElementById(`file-${credit.id}`)?.focus())
                    }}
                  />
                ) : null}
              </li>
            )
          })}
        </ul>
      )}
      {edit.isError && correcting === null ? (
        <p role="alert" className="border-t border-rule px-5 py-2 text-xs text-vermillion">
          {edit.error.message || t.common.actionFailed}
        </p>
      ) : null}
      {filed ? (
        <p role="status" className="border-t border-rule px-5 py-2 text-xs text-bone-dim">
          {p.filed(filed)}
        </p>
      ) : null}
      {credits.some((credit) => !credit.isManual) ? (
        <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{c.hiddenHint}</p>
      ) : null}

      <AddForm
        label={c.addCredit}
        pending={add.isPending}
        error={add.error}
        onSubmit={() =>
          add.mutate({
            personName: name,
            characterName: character || undefined,
            creditType: role,
            image: image || undefined,
          })
        }
      >
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField label={c.personName} htmlFor="credit-name">
            <Input id="credit-name" required value={name} onChange={(event) => setName(event.target.value)} />
          </FormField>
          <FormField label={c.character} htmlFor="credit-character">
            <Input id="credit-character" value={character} onChange={(event) => setCharacter(event.target.value)} />
          </FormField>
          <FormField label={c.role} htmlFor="credit-role">
            <Select id="credit-role" value={role} onChange={(event) => setRole(event.target.value)}>
              {roles.map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </Select>
          </FormField>
          <FormField label={e.peopleTab.photo} htmlFor="credit-image">
            <Input id="credit-image" type="url" placeholder="https://…" value={image} onChange={(event) => setImage(event.target.value)} />
          </FormField>
        </div>
      </AddForm>
    </Panel>
  )
}

/**
 * A credit corrected in place: the character played, its place among the
 * credits of its kind. Only what changed is sent — each is a lock of its
 * own — and a correction is lifted back to what the source gives.
 */
function Correction({
  credit,
  corrected,
  pending,
  error,
  onSave,
  onCancel,
}: {
  credit: Credit
  corrected: boolean
  pending: boolean
  error: Error | null
  onSave: (body: Record<string, unknown>) => void
  onCancel: () => void
}) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const [character, setCharacter] = useState(credit.characterName ?? '')
  const [place, setPlace] = useState(String(credit.sortOrder))
  const id = `correct-${credit.id}`

  const submit = () => {
    const body: Record<string, unknown> = {}
    if (character.trim() !== (credit.characterName ?? '')) body.characterName = character.trim()
    const order = Number.parseInt(place, 10)
    if (!Number.isNaN(order) && order !== credit.sortOrder) body.sortOrder = order
    if (Object.keys(body).length === 0) {
      onCancel()
      return
    }
    onSave(body)
  }

  return (
    <form
      className="mt-2 flex flex-col gap-3 rounded-card border border-rule bg-ink/40 p-3"
      onSubmit={(event) => {
        event.preventDefault()
        submit()
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onCancel()
      }}
    >
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_8rem]">
        <FormField label={c.character} htmlFor={`${id}-character`}>
          <Input id={`${id}-character`} autoFocus value={character} onChange={(event) => setCharacter(event.target.value)} />
        </FormField>
        <FormField label={c.order} htmlFor={`${id}-order`} hint={c.orderHint}>
          <Input id={`${id}-order`} type="number" inputMode="numeric" step={1} value={place} onChange={(event) => setPlace(event.target.value)} />
        </FormField>
      </div>
      {error ? (
        <p role="alert" className="text-xs text-vermillion">
          {error.message || t.common.actionFailed}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" variant="primary" size="sm" disabled={pending}>
          {pending ? <Spinner className="size-4" /> : <Glyph name="lock" className="size-4" />}
          {credit.isManual ? t.common.save : t.admin.editor.saveAndLock}
        </Button>
        <Button type="button" size="sm" onClick={onCancel}>
          {t.common.cancel}
        </Button>
        {corrected ? (
          <Button type="button" size="sm" variant="quiet" disabled={pending} onClick={() => onSave({ characterName: null, sortOrder: null })}>
            <Glyph name="unlock" className="size-4" />
            {c.backToSource}
          </Button>
        ) : null}
        {credit.isManual ? null : <span className="text-xs text-bone-faint">{c.correctHint}</span>}
      </div>
    </form>
  )
}

/**
 * The record a credit is filed under, among the people: a glyph beside the
 * name, named in full for whoever does not see it, and shown its name by the
 * site's own tooltip.
 */
function RecordLink({ id, name }: { id: string; name: string }) {
  const { t } = useI18n()
  const label = t.admin.peoplePage.inPeople(name)
  const { trigger, tip } = useTooltip(label)

  return (
    <Link
      to={`/admin/people?id=${encodeURIComponent(id)}`}
      aria-label={label}
      {...trigger}
      className="hit grid size-6 shrink-0 place-items-center rounded-card text-bone-faint transition-colors duration-150 hover:bg-ink-high hover:text-bone"
    >
      <Glyph name="user" className="size-3.5" />
      {tip}
    </Link>
  )
}

/**
 * A credit by name alone filed under somebody: found by a part of their
 * name, theirs or one they go by, and filed the moment they are chosen. A
 * refresh leaves it where it was put.
 */
function FileUnder({
  credit,
  current,
  onFiled,
  onCancel,
}: {
  credit: Credit
  /** The record it is filed under now, which is no choice. */
  current?: string
  onFiled: (name: string) => void
  onCancel: () => void
}) {
  const { t } = useI18n()
  const client = useQueryClient()
  const file = useMutation({
    mutationFn: async (person: PersonCard) => {
      await api.post(`/people/${segment(person.id)}/credits`, { credits: [credit.id] })
      return person.name
    },
    onSuccess: (name) => {
      afterPeopleWrite(client)
      onFiled(name)
    },
  })

  return (
    <div className="mt-2 space-y-3 rounded-card border border-rule bg-ink/40 p-3">
      <PersonPicker id={`file-${credit.id}-find`} exclude={current} onPick={(person) => file.mutate(person)} autoFocus />
      {file.isError ? (
        <p role="alert" className="text-xs text-vermillion">
          {file.error.message || t.common.actionFailed}
        </p>
      ) : null}
      <div className="flex items-center gap-2">
        <Button type="button" size="sm" onClick={onCancel}>
          {t.common.cancel}
        </Button>
        {file.isPending ? <Spinner className="size-4 text-bone-faint" /> : null}
      </div>
    </div>
  )
}

/* ── Alternative titles ───────────────────────────────────────────────────── */

function AlternativeTitles({ work, onChanged, onRemove }: PanelProps) {
  const { t, locale } = useI18n()
  const c = t.admin.editor.children
  const p = t.admin.editor.peopleTab
  const [title, setTitle] = useState('')
  const [type, setType] = useState('')
  const [language, setLanguage] = useState('')
  // A dozen first: a work TMDB names in forty languages is a column of
  // forty rows, most of them never looked at.
  const [all, setAll] = useState(false)

  const add = useMutation({
    mutationFn: (body: unknown) => api.post<{ id: string }>(`/items/${work.id}/alternative-titles`, body),
    onSuccess: () => {
      setTitle('')
      onChanged()
    },
  })

  // A source's title is hidden rather than removed: a lock on it, by its
  // text and language, set from the row it is read in.
  const hide = useMutation({
    mutationFn: ({ id, hidden }: { id: string; hidden: boolean }) => api.patch(`/items/${work.id}/alternative-titles/${id}`, { hidden }),
    onSuccess: onChanged,
  })
  const hidden = new Set((work.hidden?.alternativeTitles ?? []).map((title) => title.id))
  const titles = [...(work.alternativeTitles ?? []), ...(work.hidden?.alternativeTitles ?? [])]
  // The hand-added ones first, so one just added is seen whatever the count;
  // the hidden ones last.
  const ordered = [...titles].sort((x, y) => Number(y.isManual) - Number(x.isManual) || Number(hidden.has(x.id)) - Number(hidden.has(y.id)))
  const shown = all ? ordered : ordered.slice(0, 12)

  return (
    <Panel id="titles" label={c.titles} className="scroll-mt-28 lg:scroll-mt-16">
      <PanelHead
        title={
          <span className="flex items-center gap-2">
            {c.titles}
            <Count>{titles.length}</Count>
          </span>
        }
      />
      <p className="px-5 pt-3 text-xs leading-relaxed text-bone-faint">{c.titlesHint}</p>

      {titles.length === 0 ? (
        <Empty>{p.noTitles}</Empty>
      ) : (
        <ul className="mt-2 divide-y divide-rule border-t border-rule">
          {shown.map((alt) => {
            const kind = alt.titleType?.toLowerCase()
            const fromRadarr = kind !== undefined && RADARR_SOURCES.has(kind)
            const named = kind === undefined || fromRadarr ? undefined : ((t.work.titleTypes as Record<string, string>)[kind] ?? alt.titleType)
            const origin = titleOrigin(alt.language, locale, fromRadarr ? 'language' : 'place')
            const note = [origin, named].filter(Boolean).join(' · ')
            return (
              <li
                key={alt.id}
                className={cn(
                  'flex items-center gap-3 px-5 py-2 transition-colors duration-150 hover:bg-ink-high',
                  (alt.isManual || hidden.has(alt.id)) && 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]',
                )}
                data-title={alt.title}
              >
                <span className={cn('min-w-0 flex-1', hidden.has(alt.id) && 'opacity-60')}>
                  <span className={cn('block text-sm break-words text-bone', hidden.has(alt.id) && 'line-through decoration-bone-faint')}>{alt.title}</span>
                  {note || alt.language ? (
                    <span className="block text-xs text-bone-faint" title={alt.language}>
                      {note || alt.language?.toUpperCase()}
                    </span>
                  ) : null}
                </span>
                {hidden.has(alt.id) ? (
                  <Chip tone="manual">
                    <Glyph name="lock" className="size-3" />
                    {c.hidden}
                  </Chip>
                ) : null}
                {alt.isManual ? (
                  <Yours onRemove={() => onRemove({ path: `alternative-titles/${alt.id}`, label: alt.title })} />
                ) : (
                  <Button
                    size="sm"
                    variant={hidden.has(alt.id) ? undefined : 'quiet'}
                    disabled={hide.isPending && hide.variables?.id === alt.id}
                    aria-label={hidden.has(alt.id) ? c.showNamed(alt.title) : c.hideNamed(alt.title)}
                    onClick={() => hide.mutate({ id: alt.id, hidden: !hidden.has(alt.id) })}
                  >
                    {hide.isPending && hide.variables?.id === alt.id ? (
                      <Spinner className="size-4" />
                    ) : (
                      <Glyph name={hidden.has(alt.id) ? 'unlock' : 'lock'} className="size-4" />
                    )}
                    {hidden.has(alt.id) ? c.showAgain : c.hide}
                  </Button>
                )}
              </li>
            )
          })}
        </ul>
      )}
      {hide.isError ? (
        <p role="alert" className="border-t border-rule px-5 py-2 text-xs text-vermillion">
          {hide.error.message || t.common.actionFailed}
        </p>
      ) : null}
      {titles.length > shown.length ? (
        <div className="border-t border-rule px-5 py-3">
          <Button size="sm" variant="quiet" onClick={() => setAll(true)}>
            {t.work.allTitles(titles.length)}
          </Button>
        </div>
      ) : null}

      <AddForm
        label={c.addTitle}
        pending={add.isPending}
        error={add.error}
        onSubmit={() => add.mutate({ title, titleType: type.trim() || undefined, language: language.trim() || undefined })}
      >
        <FormField label={c.titles} htmlFor="alt-title">
          <Input id="alt-title" required value={title} onChange={(event) => setTitle(event.target.value)} />
        </FormField>
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField label={p.titleType} htmlFor="alt-type" hint={p.titleTypeHint}>
            <Input id="alt-type" value={type} onChange={(event) => setType(event.target.value)} />
          </FormField>
          <FormField label={p.titleLanguage} htmlFor="alt-language" hint={p.titleLanguageHint}>
            <Input id="alt-language" value={language} maxLength={5} onChange={(event) => setLanguage(event.target.value)} />
          </FormField>
        </div>
      </AddForm>
    </Panel>
  )
}

/* ── Translations ─────────────────────────────────────────────────────────── */

/**
 * The languages the sources describe the work in, each with who gave it.
 * Read here, not edited: a translation is a provider's whole answer, and
 * "Show in", on the record, reads the work in one.
 */
function Translations({ work, report }: { work: MediaItem; report?: ProvenanceReport }) {
  const { t } = useI18n()
  const p = t.admin.editor.peopleTab
  const translations = [...(work.translations ?? [])].sort((x, y) => x.language.localeCompare(y.language))
  const from = report?.provenance?.translations ?? {}

  return (
    <Panel id="translations" label={p.translations} className="scroll-mt-28 lg:scroll-mt-16">
      <PanelHead
        title={
          <span className="flex items-center gap-2">
            {p.translations}
            <Count>{translations.length}</Count>
          </span>
        }
        action={translations.length ? <span className="label hidden sm:inline">{p.translationCount(translations.length)}</span> : undefined}
      />
      {translations.length === 0 ? (
        <Empty>{p.noTranslations}</Empty>
      ) : (
        <ul className="flex flex-wrap gap-1.5 p-4">
          {translations.map((translation) => (
            <li
              key={translation.language}
              title={translation.overview?.slice(0, 200)}
              className={cn(
                'inline-flex max-w-full items-center gap-2 rounded-full border px-3 py-1.5 text-[0.8125rem]',
                translation.isManual ? 'border-brass-deep text-brass' : 'border-rule text-bone-dim',
              )}
            >
              <span className="font-mono text-[0.6875rem] font-medium text-bone uppercase">{translation.language}</span>
              <span className="min-w-0 truncate">{translation.title || p.untitled}</span>
              {from[translation.language] ? (
                <span className="font-mono text-[0.625rem] tracking-[0.08em] text-slate uppercase">{providerName(from[translation.language]!)}</span>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{p.translationsHint}</p>
    </Panel>
  )
}
