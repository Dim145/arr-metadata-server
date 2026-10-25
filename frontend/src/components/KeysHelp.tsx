/**
 * The keys the catalogue answers to, and the list of them `?` opens.
 *
 * A reader who comes back every evening learns a few keys rather than the
 * way through the bar: `/` to search, `g` and a letter to go somewhere, `r`
 * for a work at random, `t` for tonight's schedule. They are the ones the
 * catalogues and mail readers people already know use — `/`, `g` then a
 * place, `?` for the list — so nothing here has to be learnt twice.
 *
 * None of them fires while something is being typed, in a field or in an
 * open dialog, which has the keys to itself; nor with a modifier held, so the
 * browser's own and the palette's ⌘K stay theirs.
 */

import { useEffect, useRef } from 'react'
import { useLocation, useNavigate } from 'react-router'

import { drawWork } from '../lib/draw'
import { localDay } from '../lib/format'
import { useI18n, type Dict } from '../lib/i18n'
import { paletteShortcut } from './CommandPalette'
import { Button, Dialog } from './ui'

/** How long a `g` waits for the letter that says where to. */
const CHORD = 1500

/** The places a `g` leads to, by the letter that follows it. */
const PLACES: Record<string, string> = { b: '/browse', c: '/calendar', s: '/seasons' }

/**
 * Whether a key belongs to what is being typed: a field, a list of choices,
 * anything editable. Typing "great" in a search must not go to the schedule.
 */
function typing(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])') !== null
}

/**
 * The shortcuts, listened for on the whole page. `search` puts the cursor in
 * the search, wherever the width keeps it; `help` opens the list.
 */
export function useShortcuts({ search, help }: { search: () => void; help: () => void }) {
  const navigate = useNavigate()
  const { lang } = useI18n()
  const location = useLocation()
  // What the listener reads when a key comes: the latest of each, without
  // adding and removing the listener at every render.
  const latest = useRef({ search, help, lang, location, navigate })
  latest.current = { search, help, lang, location, navigate }
  const drawing = useRef(false)

  useEffect(() => {
    let waiting: number | undefined
    let chord = false
    const done = () => {
      chord = false
      window.clearTimeout(waiting)
    }

    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || event.repeat) return
      if (event.metaKey || event.ctrlKey || event.altKey) return
      if (typing(event.target)) return
      // A modal dialog keeps the keys: the palette, the sheet, a picture, this list.
      if (document.querySelector('dialog:modal')) return

      const { search, help, lang, location, navigate } = latest.current
      const key = event.key

      if (chord) {
        done()
        const place = PLACES[key]
        if (place) {
          event.preventDefault()
          navigate(place)
        }
        return
      }

      switch (key) {
        case '/':
          event.preventDefault()
          search()
          break
        case '?':
          event.preventDefault()
          help()
          break
        case 'g':
          chord = true
          waiting = window.setTimeout(done, CHORD)
          break
        case 'r': {
          event.preventDefault()
          if (drawing.current) return
          drawing.current = true
          // Drawn from the whole catalogue, as the palette's own draw is.
          drawWork({ language: lang })
            .then((work) => {
              if (work) navigate(`/work/${work.id}`)
            })
            .catch(() => {
              // Nothing drawn: the key did nothing, which is what it says.
            })
            .finally(() => {
              drawing.current = false
            })
          break
        }
        case 't': {
          event.preventDefault()
          // Today, in the schedule as it is being read: the month stays the
          // month, laid out as it was, and the week opens at today's line.
          const params = new URLSearchParams(location.search)
          const month = location.pathname === '/calendar' && params.get('view') === 'month'
          navigate(
            month
              ? `/calendar?view=month${params.get('as') === 'list' ? '&as=list' : ''}`
              : `/calendar#day-${localDay(new Date())}`,
          )
          break
        }
      }
    }

    window.addEventListener('keydown', onKey)
    return () => {
      done()
      window.removeEventListener('keydown', onKey)
    }
  }, [])
}

/** One line of the list: the keys, then what they do. */
interface Shortcut {
  /** Pressed together, or one after the other with a `then` between. */
  keys: string[]
  sequence?: boolean
  label: (t: Dict) => string
}

const SHORTCUTS: Shortcut[] = [
  { keys: ['/'], label: (t) => t.keys.search },
  { keys: [], label: (t) => t.keys.palette },
  { keys: ['g', 'b'], sequence: true, label: (t) => t.keys.browse },
  { keys: ['g', 'c'], sequence: true, label: (t) => t.keys.calendar },
  { keys: ['g', 's'], sequence: true, label: (t) => t.keys.seasons },
  { keys: ['r'], label: (t) => t.keys.random },
  { keys: ['t'], label: (t) => t.keys.today },
  { keys: ['?'], label: (t) => t.keys.help },
]

const KEY = 'inline-flex min-w-7 items-center justify-center rounded-sm border border-rule-bright bg-ink px-1.5 py-0.5 font-mono text-xs text-bone'

/**
 * The list of the keys, in a dialog: the browser's own, so the focus is held
 * inside and given back, and Escape closes it.
 */
export function KeysHelp({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useI18n()

  return (
    <Dialog
      open={open}
      title={t.keys.title}
      onClose={onClose}
      footer={
        <Button size="sm" onClick={onClose}>
          {t.nav.close}
        </Button>
      }
    >
      <dl className="divide-y divide-rule">
        {SHORTCUTS.map((shortcut) => {
          // The palette's keys are the machine's: ⌘K on a Mac, Ctrl K elsewhere.
          const keys = shortcut.keys.length ? shortcut.keys : [paletteShortcut()]
          return (
            <div key={shortcut.label(t)} className="flex items-center justify-between gap-6 py-2.5">
              <dt className="text-sm text-bone">{shortcut.label(t)}</dt>
              <dd className="flex shrink-0 items-center gap-1.5 text-xs text-bone-faint">
                {keys.map((key, index) => (
                  <span key={key} className="contents">
                    {index > 0 && shortcut.sequence ? <span>{t.keys.then}</span> : null}
                    <kbd className={KEY}>{key}</kbd>
                  </span>
                ))}
              </dd>
            </div>
          )
        })}
      </dl>
      <p className="mt-4 text-xs leading-relaxed text-bone-faint">{t.keys.hint}</p>
    </Dialog>
  )
}
