/**
 * The programmer's notes, all of them: what has been written about which
 * work, selection, genre, network, year or decade, in which language, by whom
 * and when — each written again, or taken away, where it is listed, and a new
 * one written for anything the catalogue has a page for.
 *
 * The same notes are written in place on the pages themselves; this is where
 * they are found again, and where a note in a language the interface is not
 * in is written.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useId, useState } from 'react'
import { Link } from 'react-router'

import { NoteBody } from '../../components/ProgrammersNote'
import { Button, Chip, Dialog, EmptyState, Glyph, Input, Label, Panel, PanelHead, Select, Skeleton, Textarea } from '../../components/ui'
import { api, query, segment } from '../../lib/api'
import { relative } from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { describeIdentity, languageName } from '../../lib/labels'
import { MAX_NOTE, plain, problem } from '../../lib/note'
import type { EditorialNote, EditorialNotes, EditorialScope } from '../../lib/types'

const SCOPES: EditorialScope[] = ['work', 'list', 'genre', 'network', 'year', 'decade']

/** The languages a note is offered in: the interface's own. */
const LANGUAGES = ['en', 'fr']

/** Where the page a note stands on is. */
export function pageOf(note: Pick<EditorialNote, 'scope' | 'key'>): string {
  switch (note.scope) {
    case 'work':
      return `/work/${note.key}`
    case 'list':
      return `/lists/${note.key}`
    default:
      return `/${note.scope}/${note.key}`
  }
}

export function Editorial() {
  const { t, locale } = useI18n()
  const p = t.admin.editorial
  const [scope, setScope] = useState<EditorialScope | ''>('')
  const [language, setLanguage] = useState('')
  const [editing, setEditing] = useState<EditorialNote | 'new' | null>(null)
  const [removing, setRemoving] = useState<EditorialNote | null>(null)
  const client = useQueryClient()

  const notes = useQuery({
    queryKey: ['editorial', scope, language],
    queryFn: () => api.get<EditorialNotes>(`/editorial${query({ scope, language })}`),
  })

  const refresh = () => {
    void client.invalidateQueries({ queryKey: ['editorial'] })
    void client.invalidateQueries({ queryKey: ['note'] })
    void client.invalidateQueries({ queryKey: ['facet'] })
  }
  const remove = useMutation({
    mutationFn: (note: EditorialNote) =>
      api.delete(`/editorial/${note.scope}/${segment(note.key)}/${segment(note.language)}`),
    onSuccess: () => {
      setRemoving(null)
      refresh()
    },
  })

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.editorialNav}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      </header>

      <div className="mb-6 flex flex-wrap items-end gap-3">
        <div>
          <label htmlFor="editorial-scope" className="label mb-1.5 block">
            {p.scope}
          </label>
          <Select
            id="editorial-scope"
            value={scope}
            onChange={(event) => setScope(event.target.value as EditorialScope | '')}
            className="min-w-44"
          >
            <option value="">{p.everyScope}</option>
            {SCOPES.map((value) => (
              <option key={value} value={value}>
                {p.scopes[value]}
              </option>
            ))}
          </Select>
        </div>
        <div>
          <label htmlFor="editorial-language" className="label mb-1.5 block">
            {p.language}
          </label>
          <Select
            id="editorial-language"
            value={language}
            onChange={(event) => setLanguage(event.target.value)}
            className="min-w-40"
          >
            <option value="">{p.everyLanguage}</option>
            {LANGUAGES.map((code) => (
              <option key={code} value={code}>
                {languageName(code, locale) ?? code}
              </option>
            ))}
          </Select>
        </div>
        <Button variant="primary" className="ml-auto" onClick={() => setEditing('new')}>
          <Glyph name="plus" className="size-4" />
          {p.new}
        </Button>
      </div>

      {editing === 'new' ? (
        <NoteForm
          onDone={() => {
            setEditing(null)
            refresh()
          }}
          onCancel={() => setEditing(null)}
        />
      ) : null}

      <Panel label={t.admin.editorialNav}>
        <PanelHead title={p.written} />
        {notes.isPending ? (
          <div className="space-y-3 p-5" aria-hidden>
            <Skeleton className="h-12 w-full" />
            <Skeleton className="h-12 w-full" />
          </div>
        ) : notes.isError ? (
          <p role="alert" className="flex items-center gap-2 p-5 text-sm text-status-error">
            <Glyph name="alert" className="size-4" />
            {notes.error.message}
          </p>
        ) : !notes.data.notes.length ? (
          <EmptyState kind={scope || language ? 'filtered' : 'empty'} title={p.none} hint={p.noneHint} />
        ) : (
          <ul className="divide-y divide-rule">
            {notes.data.notes.map((note) => {
              const key = `${note.scope}:${note.key}:${note.language}`
              const who = note.updatedBy ? describeIdentity(note.updatedBy, t).name : undefined
              return (
                <li key={key} className="px-5 py-4">
                  {editing !== 'new' && editing && `${editing.scope}:${editing.key}:${editing.language}` === key ? (
                    <NoteForm
                      note={note}
                      onDone={() => {
                        setEditing(null)
                        refresh()
                      }}
                      onCancel={() => setEditing(null)}
                    />
                  ) : (
                    <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <Chip tone="manual">{p.scopes[note.scope]}</Chip>
                          <Link to={pageOf(note)} className="font-mono text-sm text-bone hover:underline">
                            {note.key}
                          </Link>
                          <span className="font-mono text-caption tracking-[0.12em] text-bone-faint uppercase">{note.language}</span>
                        </div>
                        <p lang={note.language} className="mt-2 line-clamp-2 text-sm leading-relaxed text-bone-dim">
                          {plain(note.body)}
                        </p>
                        <p className="mt-1 text-xs text-bone-faint">
                          {who ? p.byWhom(who, relative(note.updatedAt, locale) ?? note.updatedAt) : relative(note.updatedAt, locale)}
                        </p>
                      </div>
                      <div className="flex gap-2">
                        <Button size="sm" onClick={() => setEditing(note)} aria-label={p.editOne(note.key, note.language)}>
                          <Glyph name="pencil" className="size-3.5" />
                          {p.edit}
                        </Button>
                        <Button size="sm" variant="danger" onClick={() => setRemoving(note)} aria-label={p.removeOne(note.key, note.language)}>
                          <Glyph name="trash" className="size-3.5" />
                        </Button>
                      </div>
                    </div>
                  )}
                </li>
              )
            })}
          </ul>
        )}
      </Panel>

      <Dialog
        open={Boolean(removing)}
        title={p.removeTitle}
        onClose={() => setRemoving(null)}
        footer={
          <>
            <Button variant="quiet" onClick={() => setRemoving(null)}>
              {t.note.cancel}
            </Button>
            <Button variant="danger" disabled={remove.isPending} onClick={() => removing && remove.mutate(removing)}>
              <Glyph name="trash" className="size-4" />
              {p.removeConfirm}
            </Button>
          </>
        }
      >
        <p>{removing ? p.removeBody(p.scopes[removing.scope], removing.key, languageName(removing.language, locale) ?? removing.language) : null}</p>
        {remove.error ? (
          <p role="alert" className="mt-2 text-sm text-status-error">
            {remove.error.message}
          </p>
        ) : null}
      </Dialog>
    </div>
  )
}

/**
 * A note as a form: what it is about and in which language — fixed for one
 * already written — and its text, with what it will look like beneath it.
 */
function NoteForm({ note, onDone, onCancel }: { note?: EditorialNote; onDone: () => void; onCancel: () => void }) {
  const { t, lang, locale } = useI18n()
  const p = t.admin.editorial
  const [scope, setScope] = useState<EditorialScope>(note?.scope ?? 'genre')
  const [key, setKey] = useState(note?.key ?? '')
  const [language, setLanguage] = useState(note?.language ?? lang)
  const [body, setBody] = useState(note?.body ?? '')
  const ids = { scope: useId(), key: useId(), language: useId(), body: useId(), hint: useId() }
  const issue = problem(body)
  const issues = { html: t.note.html, link: t.note.link, long: t.note.long(MAX_NOTE) }

  const save = useMutation({
    mutationFn: () =>
      api.put<EditorialNote | undefined>(`/editorial/${scope}/${segment(key.trim())}/${segment(language)}`, { body }),
    onSuccess: onDone,
  })

  return (
    <form
      className="mb-6 rounded-panel border border-brass-deep bg-brass/[0.04] p-5"
      onSubmit={(event) => {
        event.preventDefault()
        if (!issue && key.trim() && body.trim()) save.mutate()
      }}
    >
      <div className="grid gap-4 sm:grid-cols-[12rem_minmax(0,1fr)_10rem]">
        <div>
          <label htmlFor={ids.scope} className="label mb-1.5 block">
            {p.scope}
          </label>
          <Select id={ids.scope} value={scope} disabled={Boolean(note)} onChange={(event) => setScope(event.target.value as EditorialScope)}>
            {SCOPES.map((value) => (
              <option key={value} value={value}>
                {p.scopes[value]}
              </option>
            ))}
          </Select>
        </div>
        <div>
          <label htmlFor={ids.key} className="label mb-1.5 block">
            {p.key}
          </label>
          <Input id={ids.key} value={key} disabled={Boolean(note)} onChange={(event) => setKey(event.target.value)} placeholder={p.keyHints[scope]} required />
        </div>
        <div>
          <label htmlFor={ids.language} className="label mb-1.5 block">
            {p.language}
          </label>
          <Select id={ids.language} value={language} disabled={Boolean(note)} onChange={(event) => setLanguage(event.target.value)}>
            {LANGUAGES.map((code) => (
              <option key={code} value={code}>
                {languageName(code, locale) ?? code}
              </option>
            ))}
          </Select>
        </div>
      </div>

      <label htmlFor={ids.body} className="label mt-4 mb-1.5 block">
        {t.note.body}
      </label>
      <Textarea
        id={ids.body}
        lang={language}
        value={body}
        onChange={(event) => setBody(event.target.value)}
        rows={5}
        className="min-h-32 field-sizing-content"
        aria-describedby={ids.hint}
        aria-invalid={issue ? true : undefined}
      />
      <p id={ids.hint} className="mt-1.5 text-xs leading-relaxed text-bone-faint">
        {t.note.hint}
        <span className="ml-2 font-mono tabular-nums">{t.note.count(Array.from(body).length, MAX_NOTE)}</span>
      </p>
      {issue ? (
        <p role="alert" className="mt-1.5 text-xs text-status-error">
          {issues[issue]}
        </p>
      ) : null}
      {save.error ? (
        <p role="alert" className="mt-1.5 text-xs text-status-error">
          {save.error.message}
        </p>
      ) : null}

      {body.trim() ? (
        <div className="mt-4 border-l-2 border-brass pl-4">
          <p className="label text-brass">{t.note.preview}</p>
          <NoteBody body={body} lang={language} className="mt-2 max-w-prose font-display text-lg leading-relaxed text-bone" />
        </div>
      ) : null}

      <div className="mt-4 flex flex-wrap gap-2">
        <Button type="submit" variant="primary" disabled={save.isPending || Boolean(issue) || !key.trim() || !body.trim()}>
          <Glyph name="check" className="size-4" />
          {save.isPending ? t.note.saving : t.note.save}
        </Button>
        <Button type="button" variant="quiet" onClick={onCancel}>
          {t.note.cancel}
        </Button>
      </div>
    </form>
  )
}
