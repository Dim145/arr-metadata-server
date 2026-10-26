/**
 * A session over, in this browser: signed out from either side of the site
 * (`SignOut`), or ended with every other one by a new password
 * (`ChangePassword`).
 *
 * Everything the cache holds was answered for somebody who is now gone. It
 * used to be emptied, and nobody was told: whoever was reading an answer went
 * on reading the one it had. Who is looking is read once, above every page,
 * by a part that is never drawn again by itself — so the bar went on offering
 * "Admin" and the way out to a visitor until the page was loaded again, and
 * the sign-in that followed, which asks again only what the cache still
 * holds, never reached it either.
 */

import type { QueryClient } from '@tanstack/react-query'

import { forget } from './recent'

/**
 * Forget the account that left: the shelf of the works it opened, what it
 * sent, and every answer it was given. An answer somebody is reading goes
 * back to none, they are told, and it is asked again — by whoever is here
 * now; one nobody is reading leaves the cache altogether. An answer still on
 * its way is dropped with the rest rather than kept when it lands.
 */
export function endSession(queryClient: QueryClient) {
  forget()
  queryClient.getMutationCache().clear()
  void queryClient.resetQueries()
  queryClient.removeQueries({ predicate: (query) => query.getObserversCount() === 0 })
}
