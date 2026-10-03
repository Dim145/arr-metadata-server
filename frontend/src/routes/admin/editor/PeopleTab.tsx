/**
 * Who made the work and what else it is called: the credits, the other
 * titles Sonarr and Radarr match release names against, and the languages
 * the sources describe it in.
 *
 * Every row says whose it is in a word as well as a colour: a brass edge and
 * "yours" for one a person added, which is also the only kind that can be
 * taken away here — a provider's would simply return at the next refresh.
 */

import { useMutation } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import { Artwork } from '../../../components/media'
import { Button, FormField, Glyph, Input, Panel, PanelHead, Segmented, Select } from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import { useI18n } from '../../../lib/i18n'
import { jobLabel, providerName, titleOrigin } from '../../../lib/labels'
import type { MediaItem, ProvenanceReport } from '../../../lib/types'

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

  const credits = [...(work.credits ?? [])].sort((x, y) => x.sortOrder - y.sortOrder)
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
            return (
              <li
                key={credit.id}
                className={cn(
                  'flex items-center gap-3 px-5 py-2 transition-colors duration-150 hover:bg-ink-high',
                  credit.isManual && 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]',
                )}
              >
                {/* The name is the link; a second one on the portrait would
                    be a stop with nothing to read. */}
                {face}
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-bone">
                    {credit.tmdbPersonId ? (
                      <Link to={`/person/${credit.tmdbPersonId}`} className="transition-colors duration-150 hover:text-vermillion">
                        {credit.personName}
                      </Link>
                    ) : (
                      credit.personName
                    )}
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
                {credit.isManual ? <Yours onRemove={() => onRemove({ path: `credits/${credit.id}`, label: credit.personName })} /> : null}
              </li>
            )
          })}
        </ul>
      )}

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

  const titles = work.alternativeTitles ?? []
  // The hand-added ones first, so one just added is seen whatever the count.
  const ordered = [...titles].sort((x, y) => Number(y.isManual) - Number(x.isManual))
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
                  alt.isManual && 'border-l-2 border-brass bg-brass/[0.05] pl-[calc(1.25rem-2px)]',
                )}
              >
                <span className="min-w-0 flex-1">
                  <span className="block text-sm break-words text-bone">{alt.title}</span>
                  {note || alt.language ? (
                    <span className="block text-xs text-bone-faint" title={alt.language}>
                      {note || alt.language?.toUpperCase()}
                    </span>
                  ) : null}
                </span>
                {alt.isManual ? <Yours onRemove={() => onRemove({ path: `alternative-titles/${alt.id}`, label: alt.title })} /> : null}
              </li>
            )
          })}
        </ul>
      )}
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
