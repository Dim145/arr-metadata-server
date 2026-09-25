/**
 * Dark or light: the reader's choice, kept in the browser, or the system's
 * where they have not chosen. `public/theme.js` settles it before the first
 * paint; this is the same decision, made again when the reader changes it.
 */

import { useSyncExternalStore } from 'react'

export type Theme = 'dark' | 'light'

export const THEME_KEY = 'ams.theme'

const PAGE_COLOUR: Record<Theme, string> = { dark: '#0b0b0c', light: '#f6f3ee' }

const listeners = new Set<() => void>()

function current(): Theme {
  return document.documentElement.dataset.theme === 'light' ? 'light' : 'dark'
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** Wear this theme; remembered unless it is only the system's word. */
export function applyTheme(theme: Theme, remember = true) {
  document.documentElement.dataset.theme = theme
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', PAGE_COLOUR[theme])
  if (remember) {
    try {
      localStorage.setItem(THEME_KEY, theme)
    } catch {
      // The choice still applies to this visit.
    }
  }
  for (const listener of listeners) listener()
}

/** What the reader has chosen, if anything. */
function chosen(): Theme | undefined {
  try {
    const stored = localStorage.getItem(THEME_KEY)
    if (stored === 'light' || stored === 'dark') return stored
  } catch {
    // Nothing chosen that can be read.
  }
  return undefined
}

export function useTheme(): [Theme, (theme: Theme) => void] {
  const theme = useSyncExternalStore(subscribe, current, () => 'dark' as Theme)
  return [theme, applyTheme]
}

// Follow the system while the reader has not chosen.
if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
  window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', (event) => {
    if (chosen() === undefined) applyTheme(event.matches ? 'light' : 'dark', false)
  })
}
