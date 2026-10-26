/**
 * The programmer's note: what somebody who keeps the catalogue wrote about
 * what a page shows — the work the front page features, a selection, a genre,
 * a network, a year — set apart in brass, the colour of what people did by
 * hand, from what the sources said.
 *
 * Read in the reader's language, else in English, else in the one written
 * last; the server says which, and the text carries that language for a
 * screen reader. Whoever may edit the catalogue writes it where it is read:
 * the note opens as a field in place, with what it will look like beneath it,
 * and is saved in the language of the interface they are using. The front
 * page offers none to write where none is (`offer`): its work changes every
 * day, and a note on it is begun in the administration's page of notes.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Fragment, useId, useState } from 'react'
import { Link } from 'react-router'

import { ApiError, api, query, segment } from '../lib/api'
import { cn } from '../lib/cn'
import { useMe } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { languageName } from '../lib/labels'
import { MAX_NOTE, inline, isInside, paragraphs, problem, type Inline } from '../lib/note'
import type { EditorialNote, EditorialScope } from '../lib/types'
import { Button, Glyph, Textarea } from './ui'

/** A note's pieces as elements: emphasis, strong emphasis, links within the site. */
function Pieces({ list }: { list: Inline[] }) {
  return list.map((piece, index) => {
    switch (piece.kind) {
      case 'text':
        return <Fragment key={index}>{piece.text}</Fragment>
      case 'em':
        return (
          <em key={index}>
            <Pieces list={piece.children} />
          </em>
        )
      case 'strong':
        return (
          <strong key={index} className="font-semibold">
            <Pieces list={piece.children} />
          </strong>
        )
      case 'link':
        // Checked by the server as it was saved; anything else is its words.
        return isInside(piece.to) ? (
          <Link
            key={index}
            to={piece.to}
            className="underline decoration-brass/60 decoration-1 underline-offset-4 transition-colors duration-150 hover:text-brass hover:decoration-brass"
          >
            <Pieces list={piece.children} />
          </Link>
        ) : (
          <Fragment key={index}>
            <Pieces list={piece.children} />
          </Fragment>
        )
    }
  })
}

/** A note's text, a paragraph each, drawn from its pieces and never as HTML. */
export function NoteBody({ body, className, lang }: { body: string; className?: string; lang?: string }) {
  return (
    <div lang={lang} className={cn('space-y-3', className)}>
      {paragraphs(body).map((paragraph, index) => (
        <p key={index}>
          <Pieces list={inline(paragraph)} />
        </p>
      ))}
    </div>
  )
}

/**
 * The note on one thing, for its page. `note` is what the page was already
 * told — the genre's, network's and year's pages carry theirs — and saves a
 * request; without it, the note is asked for.
 */
export function ProgrammersNote({
  scope,
  id,
  note: given,
  offer = true,
  className,
}: {
  scope: EditorialScope
  id: string
  note?: EditorialNote | null
  /** Whether whoever may edit is offered a note to write where none is written. */
  offer?: boolean
  className?: string
}) {
  const { t, lang, locale } = useI18n()
  const me = useMe()
  const editor = Boolean(me.data?.canWrite)
  const [editing, setEditing] = useState(false)

  const note = useQuery({
    queryKey: ['note', scope, id, lang],
    queryFn: async () => {
      try {
        // Nothing written about it is `null`, no note and no failure.
        return await api.get<EditorialNote | null>(`/editorial/${scope}/${segment(id)}${query({ language: lang })}`)
      } catch (error) {
        // A work or a selection the reader may not see: no note either.
        if (error instanceof ApiError && error.status === 404) return null
        throw error
      }
    },
    initialData: given,
    staleTime: 60_000,
  })
  const shown = note.data ?? undefined

  if (editing) {
    return <NoteEditor scope={scope} id={id} current={shown} onDone={() => setEditing(false)} className={className} />
  }

  if (!shown) {
    // A visitor is shown nothing; whoever may edit is offered a note to
    // write, where the page offers one.
    return editor && offer ? (
      <div className={className}>
        <button
          type="button"
          onClick={() => setEditing(true)}
          className="inline-flex min-h-9 cursor-pointer items-center gap-1.5 rounded-full border border-dashed border-brass-deep px-3 text-body-sm text-brass transition-colors duration-150 hover:bg-brass/10"
        >
          <Glyph name="pencil" className="size-3.5" />
          {t.note.write}
        </button>
      </div>
    ) : null
  }

  const elsewhere = shown.language !== lang
  return (
    <aside aria-label={t.note.label} className={cn('border-l-2 border-brass pl-4 sm:pl-5', className)}>
      <p className="label text-brass">{t.note.label}</p>
      <NoteBody
        body={shown.body}
        lang={elsewhere ? shown.language : undefined}
        className="mt-2 max-w-prose font-display text-lg leading-relaxed text-bone sm:text-xl"
      />
      {editor ? (
        <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
          <Button size="sm" variant="quiet" onClick={() => setEditing(true)}>
            <Glyph name="pencil" className="size-3.5" />
            {t.note.edit}
          </Button>
          {elsewhere ? <span className="text-xs text-bone-faint">{t.note.fallback(languageName(shown.language, locale) ?? shown.language)}</span> : null}
        </div>
      ) : null}
    </aside>
  )
}

/**
 * The note as a field, where it is read: written in the language of the
 * interface, with what it will look like beneath it and what the server
 * would refuse said before it is sent. Escape puts it back as it was.
 */
function NoteEditor({
  scope,
  id,
  current,
  onDone,
  className,
}: {
  scope: EditorialScope
  id: string
  current?: EditorialNote
  onDone: () => void
  className?: string
}) {
  const { t, lang, locale } = useI18n()
  const client = useQueryClient()
  // The note in this language, if one is written: the one shown may be
  // another language's, standing in for it.
  const own = current && current.language === lang ? current : undefined
  const [body, setBody] = useState(own?.body ?? '')
  const field = useId()
  const hint = useId()
  const issue = problem(body)
  const path = `/editorial/${scope}/${segment(id)}/${segment(lang)}`

  const done = () => {
    void client.invalidateQueries({ queryKey: ['note', scope, id] })
    void client.invalidateQueries({ queryKey: ['facet'] })
    void client.invalidateQueries({ queryKey: ['editorial'] })
    onDone()
  }
  const save = useMutation({
    mutationFn: (text: string) => api.put<EditorialNote | undefined>(path, { body: text }),
    onSuccess: done,
  })
  const remove = useMutation({
    mutationFn: () => api.delete<undefined>(path),
    onSuccess: done,
  })
  const failure = save.error ?? remove.error
  const issues = { html: t.note.html, link: t.note.link, long: t.note.long(MAX_NOTE) }
  const language = languageName(lang, locale) ?? lang

  return (
    <form
      className={cn('border-l-2 border-brass pl-4 sm:pl-5', className)}
      onSubmit={(event) => {
        event.preventDefault()
        if (!issue) save.mutate(body)
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.stopPropagation()
          onDone()
        }
      }}
    >
      <label htmlFor={field} className="label text-brass">
        {t.note.editIn(language)}
      </label>
      <Textarea
        id={field}
        value={body}
        onChange={(event) => setBody(event.target.value)}
        rows={4}
        // The field grows with what is written, where the browser can.
        className="mt-2 min-h-28 field-sizing-content"
        aria-describedby={hint}
        aria-invalid={issue ? true : undefined}
        autoFocus
      />
      <p id={hint} className="mt-1.5 text-xs leading-relaxed text-bone-faint">
        {t.note.hint}
        <span className="ml-2 font-mono tabular-nums">{t.note.count(Array.from(body).length, MAX_NOTE)}</span>
      </p>
      {issue ? (
        <p role="alert" className="mt-1.5 text-xs text-status-error">
          {issues[issue]}
        </p>
      ) : null}
      {failure ? (
        <p role="alert" className="mt-1.5 text-xs text-status-error">
          {failure.message || t.note.failed}
        </p>
      ) : null}

      {body.trim() ? (
        <div className="mt-3 rounded-card border border-rule px-4 py-3">
          <p className="label">{t.note.preview}</p>
          <NoteBody body={body} className="mt-2 max-w-prose font-display text-lg leading-relaxed text-bone" />
        </div>
      ) : null}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button type="submit" variant="primary" size="sm" disabled={save.isPending || Boolean(issue) || (!body.trim() && !own)}>
          <Glyph name="check" className="size-3.5" />
          {save.isPending ? t.note.saving : t.note.save}
        </Button>
        <Button type="button" size="sm" variant="quiet" onClick={onDone}>
          {t.note.cancel}
        </Button>
        {own ? (
          <Button type="button" size="sm" variant="danger" onClick={() => remove.mutate()} disabled={remove.isPending}>
            <Glyph name="trash" className="size-3.5" />
            {t.note.remove}
          </Button>
        ) : null}
      </div>
    </form>
  )
}
