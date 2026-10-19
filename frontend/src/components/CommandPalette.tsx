/**
 * The command palette: ⌘K or Ctrl+K anywhere, then a title, a name, an
 * episode, or a place to go. One field and one list — what was opened lately
 * while the field is empty, the places, then what the catalogue finds: its
 * works, the people credited on them, its episodes, each group headed by its
 * kind — moved through with the arrows and opened with Enter, as a combobox
 * is. A term that starts with `@` looks for people alone.
 */

import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router'

import { cn } from '../lib/cn'
import { useSettled } from '../lib/debounce'
import { drawWork } from '../lib/draw'
import { useHasLists, useSearch } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { useRecent } from '../lib/recent'
import { Glyph } from './ui'

/** What the shortcut is called on this machine. */
export function paletteShortcut(): string {
  if (typeof navigator === 'undefined') return 'Ctrl K'
  const platform = (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ?? navigator.userAgent
  return /mac|iphone|ipad/i.test(platform) ? '⌘K' : 'Ctrl K'
}

interface Place {
  key: string
  label: string
  to: string
  /** In the administration, which the note beside it says. */
  admin?: boolean
  /** Of the administration, the part an editor is not given: the sidebar leaves it out for them. */
  adminOnly?: boolean
  /** Something to do rather than somewhere to go. */
  run?: () => void | Promise<void>
}

interface Option {
  id: string
  label: string
  /** The other name a work was found by, beside the one it is shown under. */
  aside?: string
  note?: string
  to: string
  kind: 'recent' | 'place' | 'work' | 'person' | 'episode'
  run?: () => void | Promise<void>
}

/** What marks a term as a look for people alone. */
const PEOPLE = '@'

/** An episode's place in its series, as release names and guides write it: S02E05. */
function episodeCode(season: number, episode: number): string {
  return `S${String(season).padStart(2, '0')}E${String(episode).padStart(2, '0')}`
}

/**
 * `canWrite` is whether the administration is theirs to open, `isAdmin`
 * whether all of it is: an editor is offered the catalogue's pages and not
 * the keys to the house, as the sidebar does, rather than a page the server
 * would only answer with a refusal.
 */
export function CommandPalette({ canWrite, isAdmin }: { canWrite: boolean; isAdmin: boolean }) {
  const { t, lang } = useI18n()
  const hasLists = useHasLists()
  const recent = useRecent()
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  const [term, setTerm] = useState('')
  const [active, setActive] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const dialogRef = useRef<HTMLDialogElement>(null)
  const listId = useId()
  const settled = useSettled(term)

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey || event.shiftKey || event.key.toLowerCase() !== 'k') return
      // Not from inside a field being typed in, where the keys are the
      // field's own — the palette's own input excepted, so it can be closed.
      const target = event.target as HTMLElement | null
      if (target?.closest('input, textarea, select, [contenteditable="true"]') && target !== inputRef.current) return
      event.preventDefault()
      setOpen((was) => !was)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  // Focus goes into the field on opening and back where it came from on
  // closing; the page behind holds still meanwhile — and out of reach: the
  // dialog is the browser's own, modal, so the rest of the page is inert and
  // Tab cannot leave it for the links behind, as it did when this was a
  // `div` that only claimed to be a modal.
  const before = useRef<HTMLElement | null>(null)
  useEffect(() => {
    if (!open) return
    before.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    setTerm('')
    setActive(0)
    const node = dialogRef.current
    if (node && !node.open) node.showModal()
    const overflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const frame = requestAnimationFrame(() => inputRef.current?.focus())
    return () => {
      cancelAnimationFrame(frame)
      document.body.style.overflow = overflow
      before.current?.focus()
      before.current = null
    }
  }, [open])

  const places: Place[] = useMemo(
    () => [
      {
        key: 'random',
        label: t.palette.random,
        to: '',
        // Drawn from the whole catalogue; the browse page's own button draws
        // under its filters.
        run: async () => {
          const work = await drawWork({ language: lang })
          if (work) navigate(`/work/${work.id}`)
        },
      },
      { key: 'home', label: t.brand.name, to: '/' },
      { key: 'browse', label: t.nav.browse, to: '/browse' },
      { key: 'series', label: t.nav.series, to: '/browse?kind=series' },
      { key: 'films', label: t.nav.films, to: '/browse?kind=movie' },
      { key: 'calendar', label: t.nav.calendar, to: '/calendar' },
      { key: 'seasons', label: t.nav.seasons, to: '/seasons' },
      ...(hasLists ? [{ key: 'lists', label: t.nav.lists, to: '/lists' }] : []),
      ...(canWrite
        ? ([
            { key: 'admin', label: t.admin.dashboard, to: '/admin', admin: true },
            { key: 'catalogue', label: t.admin.catalogue, to: '/admin/catalogue', admin: true },
            { key: 'discover', label: t.admin.discover, to: '/admin/discover', admin: true },
            { key: 'clients', label: t.admin.clients, to: '/admin/clients', admin: true, adminOnly: true },
            { key: 'users', label: t.admin.users, to: '/admin/users', admin: true, adminOnly: true },
            { key: 'access', label: t.admin.accessPage, to: '/admin/access', admin: true, adminOnly: true },
            { key: 'account', label: t.admin.account, to: '/admin/account', admin: true },
            { key: 'admin-lists', label: t.admin.lists, to: '/admin/lists', admin: true },
            { key: 'jobs', label: t.admin.jobs, to: '/admin/jobs', admin: true, adminOnly: true },
            { key: 'audit', label: t.admin.audit, to: '/admin/audit', admin: true, adminOnly: true },
            { key: 'notifications', label: t.admin.notificationsNav, to: '/admin/notifications', admin: true, adminOnly: true },
            { key: 'settings', label: t.admin.settings, to: '/admin/settings', admin: true, adminOnly: true },
          ] satisfies Place[]).filter((place) => isAdmin || !place.adminOnly)
        : []),
    ],
    [t, lang, canWrite, isAdmin, hasLists, navigate],
  )

  // `@` first: people alone, and no place is one of them.
  const people = term.trimStart().startsWith(PEOPLE)
  const needle = (people ? term.trimStart().slice(PEOPLE.length) : term).trim().toLowerCase()
  const matchingPlaces = people ? [] : needle ? places.filter((place) => place.label.toLowerCase().includes(needle)) : places

  const asked = settled.trimStart().startsWith(PEOPLE) ? settled.trimStart().slice(PEOPLE.length) : settled
  const search = useSearch(asked, {
    kinds: people ? ['person'] : ['work', 'person', 'episode'],
    limit: people ? 8 : 5,
    enabled: open,
  })
  // What the search holds is the last term's while the next is asked: shown
  // only while the field still asks for a search.
  const hits = asked.trim().length >= 2 && needle.length >= 2 ? search.data : undefined

  const kindOf = (kind: 'series' | 'movie') => (kind === 'series' ? t.nav.series : t.nav.films)

  const options: Option[] = [
    // What was opened lately, while nothing is typed: the likeliest thing to
    // want back. A word typed is a search, and the list is the search's; an
    // `@` asks for people, which none of these is.
    ...(needle || people
      ? []
      : recent.map((work) => ({
          id: `recent:${work.id}`,
          label: work.title,
          note: [work.year, kindOf(work.kind)].filter(Boolean).join(' · '),
          to: `/work/${work.id}`,
          kind: 'recent' as const,
        }))),
    ...matchingPlaces.map((place) => ({
      id: `place:${place.key}`,
      label: place.label,
      note: place.admin ? t.admin.title : undefined,
      to: place.to,
      kind: 'place' as const,
      run: place.run,
    })),
    ...(hits?.works ?? []).map((work) => ({
      id: `work:${work.id}`,
      label: work.title,
      aside: work.matched,
      note: [work.year, kindOf(work.kind)].filter(Boolean).join(' · '),
      to: `/work/${work.id}`,
      kind: 'work' as const,
    })),
    ...(hits?.people ?? []).map((person) => ({
      id: `person:${person.tmdbId ?? person.name}`,
      label: person.name,
      note: t.palette.credited(person.works),
      // Somebody known by no TMDB id has no page of their own: the works
      // that credit them are the list's, filtered by their name.
      to: person.tmdbId ? `/person/${person.tmdbId}` : `/browse?person=${encodeURIComponent(person.name)}`,
      kind: 'person' as const,
    })),
    ...(hits?.episodes ?? []).map((episode) => ({
      id: `episode:${episode.workId}:${episode.seasonNumber}:${episode.episodeNumber}`,
      label: `${episodeCode(episode.seasonNumber, episode.episodeNumber)} — ${episode.title}`,
      note: episode.workTitle,
      to: `/work/${episode.workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`,
      kind: 'episode' as const,
    })),
  ]
  const count = options.length
  useEffect(() => {
    setActive((current) => Math.min(current, Math.max(count - 1, 0)))
  }, [count])

  const go = (option: Option) => {
    setOpen(false)
    if (option.run) void option.run()
    else navigate(option.to)
  }

  const headings = {
    recent: t.palette.recent,
    place: t.palette.goTo,
    work: t.palette.works,
    person: t.palette.people,
    episode: t.palette.episodes,
  }

  if (!open) return null

  return (
    <dialog
      ref={dialogRef}
      aria-label={t.palette.open}
      // Escape, which the field handles itself, and whatever else the browser
      // closes a dialog by.
      onClose={() => setOpen(false)}
      onMouseDown={(event) => {
        // The dark around the panel: the dialog itself is what a press there lands on.
        if (event.target === event.currentTarget) setOpen(false)
      }}
      onKeyDown={(event) => {
        // The field is the one place to stand — the options are walked with the
        // arrows, not tabbed to — so Tab stays on it rather than going out of
        // the page to the browser's own controls and back. Escape is the way out.
        if (event.key === 'Tab') event.preventDefault()
      }}
      className="plate m-auto mt-[12vh] w-[calc(100vw-2rem)] max-w-xl overflow-hidden rounded-panel border border-rule bg-ink-raised p-0 text-bone backdrop:bg-ink/70 backdrop:backdrop-blur-sm"
    >
      <div>
        <div className="flex items-center gap-3 border-b border-rule px-4">
          <Glyph name="search" className="size-4 shrink-0 text-bone-faint" />
          <input
            ref={inputRef}
            role="combobox"
            aria-expanded="true"
            aria-controls={listId}
            aria-activedescendant={options[active] ? `${listId}-${active}` : undefined}
            aria-autocomplete="list"
            aria-label={t.palette.open}
            placeholder={t.palette.placeholder}
            value={term}
            onChange={(event) => setTerm(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown') {
                event.preventDefault()
                setActive((index) => Math.min(index + 1, Math.max(count - 1, 0)))
              } else if (event.key === 'ArrowUp') {
                event.preventDefault()
                setActive((index) => Math.max(index - 1, 0))
              } else if (event.key === 'Enter') {
                event.preventDefault()
                const option = options[active]
                if (option) go(option)
              } else if (event.key === 'Escape') {
                event.preventDefault()
                setOpen(false)
              }
            }}
            className="min-h-12 w-full bg-transparent text-base text-bone placeholder:text-bone-faint focus:outline-none"
          />
          <kbd className="hidden shrink-0 rounded-card border border-rule px-1.5 py-0.5 font-mono text-[0.625rem] text-bone-faint sm:inline">
            esc
          </kbd>
        </div>
        <ul id={listId} role="listbox" aria-label={t.palette.open} className="max-h-[50vh] overflow-y-auto py-2">
          {options.length === 0 ? (
            <li className="px-4 py-3 text-sm text-bone-faint">
              {/* While the term settles, what it finds is on its way: not "nothing" yet. */}
              {search.isFetching || (settled !== term && needle.length >= 2)
                ? t.common.loading
                : people && needle.length < 2
                  ? t.palette.namePrompt
                  : t.palette.none}
            </li>
          ) : (
            options.map((option, index) => {
              const heads = index === 0 || options[index - 1]?.kind !== option.kind
              return (
                // eslint-disable-next-line jsx-a11y/click-events-have-key-events -- the keys are the field's: the arrows move along the options, Enter opens one
                <li
                  key={option.id}
                  id={`${listId}-${index}`}
                  role="option"
                  aria-selected={index === active}
                  onMouseEnter={() => setActive(index)}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => go(option)}
                  className={cn(
                    'flex cursor-pointer items-center gap-3 px-4 py-2 text-sm',
                    index === active ? 'bg-ink-high text-bone' : 'text-bone-dim',
                  )}
                >
                  <span className="label w-20 shrink-0">{heads ? headings[option.kind] : ''}</span>
                  {option.run ? <Glyph name="shuffle" className="size-4 shrink-0 text-bone-faint" /> : null}
                  <span className="min-w-0 flex-1 truncate">
                    {option.label}
                    {/* The name it was found by, where the reader would not
                        know it for the one it is shown under. */}
                    {option.aside ? <span className="text-bone-faint"> · {t.palette.matched(option.aside)}</span> : null}
                  </span>
                  {option.note ? <span className="max-w-[40%] shrink-0 truncate font-mono text-xs text-bone-faint">{option.note}</span> : null}
                </li>
              )
            })
          )}
        </ul>
        <p className="border-t border-rule px-4 py-2 text-xs text-bone-faint">{t.palette.hint}</p>
      </div>
    </dialog>
  )
}
