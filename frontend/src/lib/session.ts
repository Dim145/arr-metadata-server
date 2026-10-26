/**
 * A session over, in this browser: signed out from either side of the site
 * (`SignOut`); ended with every other one by a new password
 * (`ChangePassword`), or by an administrator closing their own sessions or
 * untying their own account from the identity provider (`UserDetail`); or
 * ended somewhere else, and heard of here (`sessionCaches`).
 *
 * Everything the cache holds was answered for somebody who is now gone. It
 * used to be emptied, and nobody was told: whoever was reading an answer went
 * on reading the one it had. Who is looking is read once, above every page,
 * by a part that is never drawn again by itself — so the bar went on offering
 * "Admin" and the way out to a visitor until the page was loaded again, and
 * the sign-in that followed, which asks again only what the cache still
 * holds, never reached it either.
 */

import { MutationCache, QueryCache, type QueryClient, type QueryKey } from '@tanstack/react-query'

import { ApiError } from './api'
import { forget } from './recent'
import type { Me } from './types'

/**
 * The account each cache was last told is signed in, by its id: what an
 * answer saying nobody is set against. Kept apart from the answer itself,
 * which the cache already holds by the time it is heard.
 */
const signedIn = new WeakMap<QueryClient, string>()

/**
 * Forget the account that left: the shelf of the works it opened, what it
 * sent, and every answer it was given. An answer somebody is reading goes
 * back to none, they are told, and it is asked again — by whoever is here
 * now; one nobody is reading leaves the cache altogether. An answer still on
 * its way is dropped with the rest rather than kept when it lands.
 */
export function endSession(queryClient: QueryClient) {
  signedIn.delete(queryClient)
  forget()
  queryClient.getMutationCache().clear()
  void queryClient.resetQueries()
  queryClient.removeQueries({ predicate: (query) => query.getObserversCount() === 0 })
}

/**
 * The caches of the interface's client, which hear of a session that ended
 * somewhere else: it ran out, or it was closed from another browser, by
 * another administrator, or by a new password set elsewhere. Nothing tells
 * this page so. The server only refuses what it is asked from then on, and
 * the bar went on offering "Admin" and the way out until a reload.
 *
 * A refusal for want of a credential, while somebody is signed in here, is a
 * reason to ask again who is, not the answer: a password typed wrong is
 * refused the same way, signing in or changing it, and the session goes on.
 * Who is looking is asked once for any number of refusals at a time, and an
 * answer that says nobody, where somebody was, ends the session here — once:
 * after it, nobody is signed in, and the refusals that follow ask nothing. So
 * do a visitor's, which on a private site are every answer.
 *
 * The same answer ends it when it comes some other way: asked again because
 * it was old by the time a page wanted it, after the session had run out.
 *
 * Made before the client they belong to, which they reach when they hear
 * something.
 */
export function sessionCaches(client: () => QueryClient) {
  return {
    queryCache: new QueryCache({
      onSuccess: (data, query) => {
        if (isMe(query.queryKey)) heard(client(), (data as Me | undefined)?.user?.id)
      },
      onError: (error, query) => {
        if (!isMe(query.queryKey)) refused(client(), error)
        else if (error instanceof ApiError && error.isUnauthorized) heard(client(), undefined)
      },
    }),
    mutationCache: new MutationCache({ onError: (error) => refused(client(), error) }),
  }
}

/** The question every page shares: who is looking (`useMe`). */
function isMe(key: QueryKey) {
  return key.length === 1 && key[0] === 'me'
}

/** Who is looking, as the server just answered: an account, or nobody. */
function heard(queryClient: QueryClient, account: string | undefined) {
  if (account) signedIn.set(queryClient, account)
  else if (signedIn.has(queryClient)) endSession(queryClient)
}

/**
 * A read or a write refused: if it was for want of a credential, and
 * somebody is signed in, who is looking is asked again — joining the
 * question already on its way rather than asking it once per refusal.
 */
function refused(queryClient: QueryClient, error: unknown) {
  if (!(error instanceof ApiError && error.isUnauthorized) || !signedIn.has(queryClient)) return
  void queryClient.invalidateQueries({ queryKey: ['me'], exact: true }, { cancelRefetch: false })
}
