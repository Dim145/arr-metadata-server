/**
 * The works opened lately, kept in this browser alone.
 *
 * A shortcut, not a record: a shelf's worth, in localStorage, sent nowhere.
 * What was watched is a tracker's business; this is only what was looked at,
 * on this device, so the next visit can pick up where the last one left off.
 */

import { useSyncExternalStore } from 'react'

import { headlineRating, poster } from './media'
import type { MediaItem, MediaKind, Rating } from './types'

export const RECENT_KEY = 'ams.recent'
/** How many are kept: a shelf's worth. */
const KEPT = 12

/** What a card needs of a work, and when it was opened. */
export interface Recent {
  id: string
  title: string
  kind: MediaKind
  year?: number
  poster?: string
  rating?: Rating
  at: number
}

const EMPTY: Recent[] = []
let held: Recent[] | undefined
const listeners = new Set<() => void>()

function isRecent(value: unknown): value is Recent {
  if (!value || typeof value !== 'object') return false
  const entry = value as Record<string, unknown>
  return typeof entry.id === 'string' && typeof entry.title === 'string' && (entry.kind === 'series' || entry.kind === 'movie')
}

function read(): Recent[] {
  if (held) return held
  try {
    const raw = localStorage.getItem(RECENT_KEY)
    const parsed: unknown = raw ? JSON.parse(raw) : []
    held = Array.isArray(parsed) && parsed.length ? parsed.filter(isRecent) : EMPTY
  } catch {
    held = EMPTY
  }
  return held
}

function write(next: Recent[]) {
  held = next.length ? next : EMPTY
  try {
    if (next.length) localStorage.setItem(RECENT_KEY, JSON.stringify(next))
    else localStorage.removeItem(RECENT_KEY)
  } catch {
    // Kept for this visit only.
  }
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  // Another tab's doing reaches this one too.
  const onStorage = (event: StorageEvent) => {
    if (event.key === null || event.key === RECENT_KEY) {
      held = undefined
      listener()
    }
  }
  window.addEventListener('storage', onStorage)
  return () => {
    listeners.delete(listener)
    window.removeEventListener('storage', onStorage)
  }
}

/** Note that a work was opened: to the front of the shelf, once. */
export function remember(item: MediaItem) {
  const entry: Recent = {
    id: item.id,
    title: item.title,
    kind: item.kind,
    year: item.year,
    poster: poster(item),
    rating: headlineRating(item.ratings),
    at: Date.now(),
  }
  write([entry, ...read().filter((recent) => recent.id !== item.id)].slice(0, KEPT))
}

/** The shelf, cleared. */
export function forget() {
  write([])
}

/** What was opened lately, newest first; empty where nothing can be read. */
export function useRecent(): Recent[] {
  return useSyncExternalStore(subscribe, read, () => EMPTY)
}
