/**
 * Whether the posters of works for adults are shown as they are, chosen by
 * whoever is signed in — on their own page (`/account`) or in the bar's
 * reading menu — and kept with their account, so the choice follows them
 * from one browser to the next. A visitor's is kept in this browser
 * (`prefs.ts`), and so is the choice of an account that never made one.
 *
 * Every card reads the store in `prefs.ts`, not the account: the account's
 * choice is put there once, from the top of the page
 * (`useAdultPostersFromAccount`), so a card costs no more than it did.
 */

import { hashKey, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useReducer } from 'react'

import { useMe } from './hooks'
import { usePreference } from './preferences'
import { adoptAdultPosters, showAdultPosters, useAdultPostersShown } from './prefs'

/** The account's preference, under a name the server takes (`clean_preference_key`). */
export const ADULT_POSTERS_PREFERENCE = 'reader.adultPosters'

/** Who is signed in, as the cache holds the answer: under the key every page asks it by (`useMe`). */
const ME = hashKey(['me'])

/** What the account keeps, read as a choice: shown or veiled. Anything else is none. */
export function readChoice(raw: unknown): boolean | undefined {
  return typeof raw === 'boolean' ? raw : undefined
}

/**
 * Put the choice of the account signed in where every card reads it — and
 * take it away when nobody is, or the account never chose, so that this
 * browser's counts. Mounted once, above everything that shows a poster.
 */
export function useAdultPostersFromAccount() {
  const queryClient = useQueryClient()
  const account = useMe().data?.user?.id
  const [kept] = usePreference<boolean | undefined>(ADULT_POSTERS_PREFERENCE, undefined, readChoice)
  const chosen = account ? kept : undefined
  const [, reread] = useReducer((count: number) => count + 1, 0)

  useEffect(() => {
    adoptAdultPosters(chosen)
  }, [chosen])

  // Signing out, or changing one's password, empties the whole cache
  // (`SignOut`, `ChangePassword`) without a word to whoever reads it: the
  // account's choice goes at once, before the page is drawn again, and the
  // reads above are made afresh, so that whoever signs in next is the one
  // whose choice is put here — not the account that left.
  useEffect(
    () =>
      queryClient.getQueryCache().subscribe((event) => {
        if (event.type !== 'removed' || event.query.queryHash !== ME) return
        adoptAdultPosters(undefined)
        reread()
      }),
    [queryClient],
  )
}

/**
 * Whether the posters of works for adults are shown here, and the way to
 * choose: for whoever is signed in, kept with their account — taking at
 * once, and sent a moment after — and for a visitor, in this browser.
 */
export function useAdultPosters(): [boolean, (shown: boolean) => void] {
  const shown = useAdultPostersShown()
  const account = useMe().data?.user?.id
  const [, keep] = usePreference<boolean | undefined>(ADULT_POSTERS_PREFERENCE, undefined, readChoice)

  const choose = useCallback(
    (next: boolean) => {
      if (!account) {
        showAdultPosters(next)
        return
      }
      adoptAdultPosters(next)
      keep(next)
    },
    [account, keep],
  )

  return [shown, choose]
}
