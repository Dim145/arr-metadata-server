/**
 * The command palette: ⌘K or Ctrl+K anywhere, then a title, a name, an
 * episode, or a place to go. One field and one list — what was opened lately
 * while the field is empty, the places, then what the catalogue finds: its
 * works, the people credited on them, its episodes, each group headed by its
 * kind — moved through with the arrows and opened with Enter, as a combobox
 * is. A term that starts with `@` looks for people alone.
 *
 * In the administration it is also where things are done. The page open
 * says what can be done there (`useCommands`): to the work in the editor,
 * to the rows chosen in the catalogue or the one the focus is on — refresh
 * it, lock its overview, enable or disable it, add it to a list. A command
 * that asks for a choice — which list, which task — opens a page of its own
 * in the same field; Backspace in an empty field comes back. Each command
 * shows its keys, which work anywhere in the administration without the
 * palette (`useCommandKeys`): learnt by reading them here.
 */

import {
  createContext,
  use,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { useNavigate } from 'react-router'

import { cn } from '../lib/cn'
import { useSettled } from '../lib/debounce'
import { drawWork } from '../lib/draw'
import { useHasLists, useSearch } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { leaving } from '../lib/leave'
import { useRecent } from '../lib/recent'
import { Glyph, Spinner, type GlyphName } from './ui'

/** What the shortcut is called on this machine. */
export function paletteShortcut(): string {
  return onMac() ? '⌘K' : 'Ctrl K'
}

function onMac(): boolean {
  if (typeof navigator === 'undefined') return false
  const platform = (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ?? navigator.userAgent
  return /mac|iphone|ipad/i.test(platform)
}

/* ── Commands ─────────────────────────────────────────────────────────────── */

/**
 * Something to do, offered by the page that knows what it would be done to.
 * `keys` are written as `mod+s` — ⌘ on a Mac, Ctrl elsewhere — `shift+mod+enter`,
 * `r`, or `g c` for one key and then the other; they work anywhere in the
 * administration, and the palette shows them beside the command.
 */
export interface Command {
  id: string
  label: string
  /** What it is done to, or where it leads, at the end of its line. */
  note?: string
  glyph?: GlyphName
  keys?: string
  /**
   * Done when it is chosen or its keys are pressed. A promise keeps the
   * palette open, the command waiting, until it settles: closed when it
   * worked, the reason said when it did not.
   */
  run?: () => void | Promise<unknown>
  /** A choice to make first, as a page of its own: which list, which task. */
  choose?: { title: string; load: () => Promise<Command[]> | Command[] }
  disabled?: boolean
  /** Why it cannot be done now, beside it. */
  hint?: string
}

/** The commands a page offers, under a heading that says what they are done to. */
export interface CommandGroup {
  id: string
  heading: string
  commands: Command[]
}

interface Registry {
  set: (group: CommandGroup) => void
  drop: (id: string) => void
  groups: () => CommandGroup[]
}

const CommandsContext = createContext<Registry | null>(null)

/**
 * Where the pages of the administration put their commands. Held aside
 * rather than as state: a page writes its group again each time it draws —
 * with the work as it is now — and the palette and the keys read the
 * groups when they are used, so nothing is drawn again for it.
 */
export function CommandsProvider({ children }: { children: ReactNode }) {
  const groups = useRef(new Map<string, CommandGroup>())
  const registry = useMemo<Registry>(
    () => ({
      set: (group) => groups.current.set(group.id, group),
      drop: (id) => groups.current.delete(id),
      groups: () => [...groups.current.values()],
    }),
    [],
  )
  return <CommandsContext value={registry}>{children}</CommandsContext>
}

/** Offer a group of commands while the page is open; `null` offers none. */
export function useCommands(group: CommandGroup | null) {
  const registry = use(CommandsContext)
  const id = group?.id

  // Written again at each drawing: the commands close over the page as it is.
  useEffect(() => {
    if (registry && group) registry.set(group)
  })
  useEffect(() => {
    if (!registry || !id) return
    return () => registry.drop(id)
  }, [registry, id])
}

/** One step of a command's keys: the key, and what is held with it. */
interface Stroke {
  key: string
  mod: boolean
  shift: boolean
  alt: boolean
}

/** A command's keys as strokes: one, or two one after the other. */
export function strokesOf(keys: string): Stroke[] {
  return keys
    .trim()
    .split(/\s+/)
    .map((part) => {
      const pieces = part.toLowerCase().split('+')
      const key = pieces[pieces.length - 1] ?? ''
      return {
        key,
        mod: pieces.includes('mod'),
        shift: pieces.includes('shift'),
        alt: pieces.includes('alt'),
      }
    })
}

/** How a key is written on this machine's keyboard. */
function keyName(key: string, mac: boolean): string {
  switch (key) {
    case 'mod':
      return mac ? '⌘' : 'Ctrl'
    case 'shift':
      return '⇧'
    case 'alt':
      return mac ? '⌥' : 'Alt'
    case 'enter':
      return '↵'
    case 'escape':
      return 'Esc'
    case 'arrowup':
      return '↑'
    case 'arrowdown':
      return '↓'
    default:
      return key.length === 1 ? key.toUpperCase() : key
  }
}

/** A command's keys as they are shown: each stroke's keys, strokes apart. */
export function keyLabels(keys: string, mac = onMac()): string[][] {
  return keys
    .trim()
    .split(/\s+/)
    .map((part) =>
      part
        .toLowerCase()
        .split('+')
        .map((key) => (part.includes('+') || key.length > 1 ? keyName(key, mac) : key)),
    )
}

/** Whether a key belongs to what is being typed: a field, a list of choices, anything editable. */
function typing(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])') !== null
}

/** How long a first key waits for the one after it. */
const CHORD = 1500

/**
 * The keys of every command offered, listened for on the whole page. A key
 * held with ⌘ or Ctrl works even in a field — ⌘S saves what is being typed;
 * a key alone does not, nor anything while a dialog has the keys to itself.
 */
export function useCommandKeys(extra: () => Command[]) {
  const registry = use(CommandsContext)
  const latest = useRef(extra)
  latest.current = extra

  useEffect(() => {
    if (!registry) return
    let first: Stroke | null = null
    let waiting: number | undefined
    const forget = () => {
      first = null
      window.clearTimeout(waiting)
    }
    const same = (stroke: Stroke, event: KeyboardEvent) =>
      stroke.key === event.key.toLowerCase() &&
      stroke.mod === (event.metaKey || event.ctrlKey) &&
      stroke.shift === event.shiftKey &&
      stroke.alt === event.altKey

    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || event.repeat) return
      if (document.querySelector('dialog:modal')) return
      const commands = [...registry.groups().flatMap((group) => group.commands), ...latest.current()].filter(
        (command) => command.keys && !command.disabled && (command.run || command.choose),
      )
      const held = event.metaKey || event.ctrlKey
      if (typing(event.target) && !held) return

      for (const command of commands) {
        const strokes = strokesOf(command.keys!)
        const matched =
          strokes.length === 1
            ? !first && same(strokes[0]!, event)
            : strokes.length === 2 && first !== null && sameStroke(first, strokes[0]!) && same(strokes[1]!, event)
        if (!matched) continue
        event.preventDefault()
        forget()
        if (command.run) void command.run()
        return
      }

      // The first of two strokes, when some command starts with it.
      const pressed: Stroke = { key: event.key.toLowerCase(), mod: held, shift: event.shiftKey, alt: event.altKey }
      const starts = commands.some((command) => {
        const strokes = strokesOf(command.keys!)
        return strokes.length === 2 && sameStroke(strokes[0]!, pressed)
      })
      if (starts && !first) {
        first = pressed
        waiting = window.setTimeout(forget, CHORD)
      } else {
        forget()
      }
    }

    window.addEventListener('keydown', onKey)
    return () => {
      forget()
      window.removeEventListener('keydown', onKey)
    }
  }, [registry])
}

function sameStroke(a: Stroke, b: Stroke): boolean {
  return a.key === b.key && a.mod === b.mod && a.shift === b.shift && a.alt === b.alt
}

/** A command's keys, drawn as keys. */
export function Keys({ keys, className }: { keys: string; className?: string }) {
  const { t } = useI18n()
  const strokes = keyLabels(keys)
  return (
    <span className={cn('inline-flex shrink-0 items-center gap-1 text-xs text-bone-faint', className)}>
      {strokes.map((stroke, index) => (
        <span key={index} className="inline-flex items-center gap-1">
          {index > 0 ? <span>{t.keys.then}</span> : null}
          {stroke.map((key) => (
            <kbd
              key={key}
              className="inline-flex min-w-6 items-center justify-center rounded-sm border border-rule-bright bg-ink px-1.5 py-0.5 font-mono text-xs text-bone-dim"
            >
              {key}
            </kbd>
          ))}
        </span>
      ))}
    </span>
  )
}

/* ── The palette ──────────────────────────────────────────────────────────── */

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
  /** The second key after `g`, in the administration. */
  chord?: string
}

interface Option {
  id: string
  label: string
  /** The other name a work was found by, beside the one it is shown under. */
  aside?: string
  note?: string
  to?: string
  kind: 'recent' | 'place' | 'work' | 'person' | 'episode' | 'command'
  /** A command's group, which heads its kind. */
  heading?: string
  command?: Command
  run?: () => void | Promise<void>
  keys?: string
  glyph?: GlyphName
  disabled?: boolean
}

/** A page of choices a command opened. */
interface Page {
  title: string
  commands: Command[]
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
 * would only answer with a refusal. `places` adds what only the
 * administration offers: its own keys to its places, a task to run.
 */
export function CommandPalette({
  canWrite,
  isAdmin,
  extra,
}: {
  canWrite: boolean
  isAdmin: boolean
  /** The administration's own commands, offered on every page of it. */
  extra?: Command[]
}) {
  const { t, lang } = useI18n()
  const hasLists = useHasLists()
  const recent = useRecent()
  const navigate = useNavigate()
  const registry = use(CommandsContext)
  const [open, setOpen] = useState(false)
  const [term, setTerm] = useState('')
  const [active, setActive] = useState(0)
  const [pages, setPages] = useState<Page[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const dialogRef = useRef<HTMLDialogElement>(null)
  const listId = useId()
  const settled = useSettled(term)
  const page = pages[pages.length - 1]

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
    setPages([])
    setBusy(null)
    setFailure(null)
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

  const inAdmin = registry !== null
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
          if (work) leaving(() => navigate(`/work/${work.id}`))
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
            { key: 'admin', label: t.admin.dashboard, to: '/admin', admin: true, chord: 'd' },
            { key: 'catalogue', label: t.admin.catalogue, to: '/admin/catalogue', admin: true, chord: 'c' },
            { key: 'admin-people', label: t.admin.peopleNav, to: '/admin/people', admin: true, chord: 'p' },
            { key: 'editorial', label: t.admin.editorialNav, to: '/admin/editorial', admin: true },
            { key: 'discover', label: t.admin.discover, to: '/admin/discover', admin: true, chord: 'i' },
            { key: 'clients', label: t.admin.clients, to: '/admin/clients', admin: true, adminOnly: true, chord: 'k' },
            { key: 'users', label: t.admin.users, to: '/admin/users', admin: true, adminOnly: true, chord: 'u' },
            { key: 'access', label: t.admin.accessPage, to: '/admin/access', admin: true, adminOnly: true },
            { key: 'account', label: t.admin.account, to: '/admin/account', admin: true },
            { key: 'admin-lists', label: t.admin.lists, to: '/admin/lists', admin: true, chord: 'l' },
            { key: 'jobs', label: t.admin.jobs, to: '/admin/jobs', admin: true, adminOnly: true, chord: 't' },
            { key: 'sources', label: t.admin.sourcesNav, to: '/admin/sources', admin: true },
            { key: 'media', label: t.admin.mediaNav, to: '/admin/media', admin: true, adminOnly: true, chord: 'm' },
            { key: 'backups', label: t.admin.backupsNav, to: '/admin/backups', admin: true, adminOnly: true, chord: 'b' },
            { key: 'audit', label: t.admin.audit, to: '/admin/audit', admin: true, adminOnly: true, chord: 'a' },
            { key: 'logs', label: t.admin.logsNav, to: '/admin/logs', admin: true, adminOnly: true },
            { key: 'notifications', label: t.admin.notificationsNav, to: '/admin/notifications', admin: true, adminOnly: true, chord: 'n' },
            { key: 'settings', label: t.admin.settings, to: '/admin/settings', admin: true, adminOnly: true, chord: 's' },
          ] satisfies Place[]).filter((place) => isAdmin || !place.adminOnly)
        : []),
    ],
    [t, lang, canWrite, isAdmin, hasLists, navigate],
  )

  // `@` first: people alone, and no place is one of them.
  const people = !page && term.trimStart().startsWith(PEOPLE)
  const needle = (people ? term.trimStart().slice(PEOPLE.length) : term).trim().toLowerCase()
  const matches = (label: string) => !needle || label.toLowerCase().includes(needle)
  const matchingPlaces = people || page ? [] : places.filter((place) => matches(place.label))

  const asked = settled.trimStart().startsWith(PEOPLE) ? settled.trimStart().slice(PEOPLE.length) : settled
  const search = useSearch(asked, {
    kinds: people ? ['person'] : ['work', 'person', 'episode'],
    limit: people ? 8 : 5,
    enabled: open && !page,
  })
  // What the search holds is the last term's while the next is asked: shown
  // only while the field still asks for a search.
  const hits = !page && asked.trim().length >= 2 && needle.length >= 2 ? search.data : undefined

  const kindOf = (kind: 'series' | 'movie') => (kind === 'series' ? t.nav.series : t.nav.films)

  // What the page open offers, then the administration's own.
  const groups: CommandGroup[] = open && registry ? registry.groups() : []
  const commandOption = (command: Command, heading: string): Option => ({
    id: `command:${heading}:${command.id}`,
    label: command.label,
    note: command.disabled ? command.hint : command.note,
    kind: 'command',
    heading,
    command,
    keys: command.keys,
    glyph: command.glyph,
    disabled: command.disabled,
  })

  // A page's choices need no heading: the way back above them names it.
  const options: Option[] = page
    ? page.commands.filter((command) => matches(command.label)).map((command) => commandOption(command, ''))
    : [
        ...(people
          ? []
          : [
              ...groups.flatMap((group) =>
                group.commands.filter((command) => matches(command.label)).map((command) => commandOption(command, group.heading)),
              ),
              ...(extra ?? []).filter((command) => matches(command.label)).map((command) => commandOption(command, t.palette.heading.tasks)),
            ]),
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
          keys: inAdmin && place.chord ? `g ${place.chord}` : undefined,
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
          id: `person:${person.id}`,
          label: person.name,
          note: t.palette.credited(person.works),
          // Everybody found has a record, and a page by it.
          to: `/people/${encodeURIComponent(person.id)}`,
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

  const close = () => setOpen(false)

  const go = async (option: Option) => {
    if (option.disabled || busy) return
    setFailure(null)
    const command = option.command
    if (command?.choose) {
      setBusy(option.id)
      try {
        const commands = await command.choose.load()
        setPages((was) => [...was, { title: command.choose!.title, commands }])
        setTerm('')
        setActive(0)
      } catch (error) {
        setFailure(error instanceof Error ? error.message : String(error))
      } finally {
        setBusy(null)
      }
      return
    }
    const run = command?.run ?? option.run
    if (!run) {
      close()
      const to = option.to
      // Through whatever holds unsaved changes, which asks first.
      if (to) leaving(() => navigate(to))
      return
    }
    const outcome = run()
    if (!(outcome instanceof Promise)) {
      close()
      return
    }
    setBusy(option.id)
    try {
      await outcome
      close()
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  const headings: Record<Exclude<Option['kind'], 'command'>, string> = {
    recent: t.palette.recent,
    place: t.palette.goTo,
    work: t.palette.works,
    person: t.palette.people,
    episode: t.palette.episodes,
  }
  const headingOf = (option: Option) => (option.kind === 'command' ? (option.heading ?? '') : headings[option.kind])

  if (!open) return null

  return (
    <dialog
      ref={dialogRef}
      aria-label={t.palette.open}
      // Escape, which the field handles itself, and whatever else the browser
      // closes a dialog by.
      onClose={close}
      onMouseDown={(event) => {
        // The dark around the panel: the dialog itself is what a press there lands on.
        if (event.target === event.currentTarget) close()
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
          {page ? (
            <button
              type="button"
              onClick={() => {
                setPages((was) => was.slice(0, -1))
                setTerm('')
                inputRef.current?.focus()
              }}
              className="inline-flex min-h-8 shrink-0 cursor-pointer items-center gap-1 rounded-full border border-rule-bright px-2.5 text-xs text-bone-dim hover:text-bone"
            >
              <Glyph name="chevronLeft" className="size-3" />
              {page.title}
              <span className="sr-only">: {t.palette.back}</span>
            </button>
          ) : (
            <Glyph name="search" className="size-4 shrink-0 text-bone-faint" />
          )}
          <input
            ref={inputRef}
            role="combobox"
            aria-expanded="true"
            aria-controls={listId}
            aria-activedescendant={options[active] ? `${listId}-${active}` : undefined}
            aria-autocomplete="list"
            aria-label={page ? t.palette.choosing(page.title) : t.palette.open}
            placeholder={page ? t.palette.choosing(page.title) : t.palette.placeholder}
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
                if (option) void go(option)
              } else if (event.key === 'Escape') {
                event.preventDefault()
                close()
              } else if (event.key === 'Backspace' && !term && page) {
                event.preventDefault()
                setPages((was) => was.slice(0, -1))
                setActive(0)
              }
            }}
            className="min-h-12 w-full bg-transparent text-base text-bone placeholder:text-bone-faint focus:outline-none"
          />
          <kbd className="hidden shrink-0 rounded-card border border-rule px-1.5 py-0.5 font-mono text-[0.625rem] text-bone-faint sm:inline">
            esc
          </kbd>
        </div>
        {failure ? (
          <p role="alert" className="flex items-start gap-2 border-b border-rule px-4 py-2 text-sm text-status-error">
            <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
            <span>
              {t.palette.failed} {failure}
            </span>
          </p>
        ) : null}
        <ul id={listId} role="listbox" aria-label={page?.title ?? t.palette.open} className="max-h-[50vh] overflow-y-auto py-2">
          {options.length === 0 ? (
            <li className="px-4 py-3 text-sm text-bone-faint">
              {/* While the term settles, what it finds is on its way: not "nothing" yet. */}
              {!page && (search.isFetching || (settled !== term && needle.length >= 2))
                ? t.common.loading
                : people && needle.length < 2
                  ? t.palette.namePrompt
                  : t.palette.none}
            </li>
          ) : (
            options.map((option, index) => {
              const heading = headingOf(option)
              const heads = index === 0 || headingOf(options[index - 1]!) !== heading
              return (
                // eslint-disable-next-line jsx-a11y/click-events-have-key-events -- the keys are the field's: the arrows move along the options, Enter opens one
                <li
                  key={option.id}
                  id={`${listId}-${index}`}
                  role="option"
                  aria-selected={index === active}
                  aria-disabled={option.disabled || undefined}
                  onMouseEnter={() => setActive(index)}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => void go(option)}
                  className={cn(
                    'flex cursor-pointer items-center gap-3 px-4 py-2 text-sm',
                    index === active ? 'bg-ink-high text-bone' : 'text-bone-dim',
                    option.disabled && 'cursor-not-allowed opacity-60',
                  )}
                >
                  <span className="label w-20 shrink-0 truncate" title={heads ? heading : undefined}>
                    {heads ? heading : ''}
                  </span>
                  {busy === option.id ? (
                    <Spinner className="size-4 shrink-0" />
                  ) : option.glyph ? (
                    <Glyph name={option.glyph} className="size-4 shrink-0 text-bone-faint" />
                  ) : option.run ? (
                    <Glyph name="shuffle" className="size-4 shrink-0 text-bone-faint" />
                  ) : null}
                  <span className="min-w-0 flex-1 truncate">
                    {option.label}
                    {/* The name it was found by, where the reader would not
                        know it for the one it is shown under. */}
                    {option.aside ? <span className="text-bone-faint"> · {t.palette.matched(option.aside)}</span> : null}
                  </span>
                  {option.note ? <span className="max-w-[40%] shrink-0 truncate font-mono text-xs text-bone-faint">{option.note}</span> : null}
                  {option.keys ? <Keys keys={option.keys} className="hidden sm:inline-flex" /> : null}
                  {option.command?.choose ? <Glyph name="chevronRight" className="size-3.5 shrink-0 text-bone-faint" /> : null}
                </li>
              )
            })
          )}
        </ul>
        <p className="border-t border-rule px-4 py-2 text-xs text-bone-faint">{inAdmin ? t.palette.adminHint : t.palette.hint}</p>
      </div>
    </dialog>
  )
}
