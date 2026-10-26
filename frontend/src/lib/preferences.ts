/**
 * What a person keeps of their own way of working — in the administration,
 * the views of the catalogue they saved, which columns each table shows them
 * and how tightly it is set, whether the editor shows its preview; in the
 * catalogue, whether the posters of works for adults are shown (see
 * `adultPosters.ts`) — kept with their account, so it follows them from one
 * browser to the next (`/account/preferences`).
 *
 * The whole set is read once and held for every page: each
 * preference is read from that one answer, its last value kept in this
 * browser as well, so a page is drawn as it was left before the answer is
 * in, and written back a moment after it changes — the last of several
 * quick changes alone. A change is marked as not yet received until the
 * account says it has it: sent as the page goes away if it was still
 * waiting, and sent again by the next page if it never arrived, rather
 * than given up for the account's older answer. Whoever reaches the
 * administration without an account of their own (a trusted address, the
 * server with authentication off) keeps theirs in this browser alone.
 */

import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useState } from 'react'

import { useMe } from './hooks'
import type { Preferences } from './types'

/** Where the answer is held: one for the whole administration. */
export const PREFERENCES_KEY = ['account', 'preferences'] as const

/** How long a change waits for the next before it is sent. */
const SETTLE_MS = 400

/** The browser's copy of one preference, under whose it is. */
function localName(owner: string, key: string): string {
  return `ams.pref.${owner}.${key}`
}

/** Where a change the account has not received yet is marked. */
function unsentName(owner: string, key: string): string {
  return `ams.pref-unsent.${owner}.${key}`
}

function readLocal(owner: string, key: string): unknown {
  try {
    const raw = window.localStorage.getItem(localName(owner, key))
    return raw === null ? undefined : (JSON.parse(raw) as unknown)
  } catch {
    // No storage, or a value from another version that no longer reads.
    return undefined
  }
}

function writeLocal(owner: string, key: string, value: unknown) {
  try {
    if (value === undefined) window.localStorage.removeItem(localName(owner, key))
    else window.localStorage.setItem(localName(owner, key), JSON.stringify(value))
  } catch {
    // A private window, or storage refused: the account still has it.
  }
}

function unsent(owner: string, key: string): boolean {
  try {
    return window.localStorage.getItem(unsentName(owner, key)) !== null
  } catch {
    return false
  }
}

function markUnsent(owner: string, key: string, waiting: boolean) {
  try {
    if (waiting) window.localStorage.setItem(unsentName(owner, key), '1')
    else window.localStorage.removeItem(unsentName(owner, key))
  } catch {
    // Without storage nothing outlives the page to be sent again.
  }
}

/**
 * Send one preference to the account: kept, or forgotten when it is
 * `undefined`. `keepalive` lets it leave with a page that is going away.
 */
function send(owner: string, key: string, value: unknown, keepalive: boolean): Promise<void> {
  const path = `/api/v1/account/preferences/${encodeURIComponent(key)}`
  const init: RequestInit =
    value === undefined
      ? { method: 'DELETE', credentials: 'same-origin', keepalive }
      : {
          method: 'PUT',
          credentials: 'same-origin',
          keepalive,
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ value }),
        }
  return fetch(path, init)
    .then((response) => {
      if (response.ok) markUnsent(owner, key, false)
    })
    .catch(() => {
      // Still marked: the next page sends it again.
    })
}

/** The writes waiting to be sent, one per preference: the last value of each. */
const waiting = new Map<string, { timer: number; flush: (keepalive: boolean) => void }>()

/** Send `value` a moment from now, in place of any change of `key` still waiting. */
function schedule(owner: string, key: string, value: unknown) {
  markUnsent(owner, key, true)
  const was = waiting.get(key)
  if (was) window.clearTimeout(was.timer)
  const flush = (keepalive: boolean) => {
    waiting.delete(key)
    void send(owner, key, value, keepalive)
  }
  waiting.set(key, { timer: window.setTimeout(() => flush(false), SETTLE_MS), flush })
}

// What is still waiting leaves with the page rather than with it.
if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', () => {
    for (const { timer, flush } of [...waiting.values()]) {
      window.clearTimeout(timer)
      flush(true)
    }
  })
}

/** Put `value` in the held answer, so every reader of it moves together. */
function hold(queryClient: QueryClient, key: string, value: unknown) {
  queryClient.setQueryData<Preferences>(PREFERENCES_KEY, (was) => {
    const values = { ...(was?.values ?? {}) }
    if (value === undefined) delete values[key]
    else values[key] = value
    return { values }
  })
}

/**
 * One preference: its value — `fallback` while none is kept, or what is kept
 * no longer reads as `read` expects — and the way to change it. Changed to
 * `undefined`, it is forgotten, and the interface goes back to its own way.
 */
export function usePreference<T>(
  key: string,
  fallback: T,
  read?: (raw: unknown) => T | undefined,
): [T, (next: T | undefined) => void] {
  const me = useMe()
  const queryClient = useQueryClient()
  const account = me.data?.user?.id
  const owner = account ?? 'browser'

  const kept = useQuery({
    queryKey: PREFERENCES_KEY,
    queryFn: async () => {
      const response = await fetch('/api/v1/account/preferences', { credentials: 'same-origin' })
      if (!response.ok) throw new Error(response.statusText)
      return (await response.json()) as Preferences
    },
    enabled: Boolean(account),
    staleTime: Infinity,
    retry: false,
  })
  const [local, setLocal] = useState<unknown>(() => readLocal(owner, key))

  // The account's answer is the truth, and this browser's copy is brought
  // to it — but for a change of this browser's the account never received,
  // which is kept, and sent again.
  const answered = account !== undefined && kept.data !== undefined
  const fromAccount = kept.data?.values[key]
  const mine = answered && unsent(owner, key)
  useEffect(() => {
    if (!answered) return
    if (unsent(owner, key)) {
      const value = readLocal(owner, key)
      hold(queryClient, key, value)
      if (!waiting.has(key)) schedule(owner, key, value)
      return
    }
    writeLocal(owner, key, fromAccount)
  }, [answered, owner, key, fromAccount, queryClient])

  // A change not received yet is read from this browser's copy, which every
  // reader of the preference shares.
  const raw = answered ? (mine ? readLocal(owner, key) : fromAccount) : local
  const value = raw === undefined ? undefined : read ? read(raw) : (raw as T)

  const set = useCallback(
    (next: T | undefined) => {
      setLocal(next)
      writeLocal(owner, key, next)
      if (!account) return
      hold(queryClient, key, next)
      schedule(owner, key, next)
    },
    [account, owner, key, queryClient],
  )

  return [value ?? fallback, set]
}
