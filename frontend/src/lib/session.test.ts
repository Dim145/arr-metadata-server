import { MutationObserver, QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { endSession } from './session'

/** Who is looking, as much of it as these need: whether they may write. */
interface Me {
  canWrite: boolean
}

/** A browser's storage, kept in a map. */
function storage(initial: Record<string, string> = {}) {
  const kept = new Map(Object.entries(initial))
  return {
    kept,
    getItem: (key: string) => kept.get(key) ?? null,
    setItem: (key: string, value: string) => void kept.set(key, value),
    removeItem: (key: string) => void kept.delete(key),
  }
}

/** A cache as the interface's, without its retries: a refusal is the answer here. */
function cache() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } })
}

describe('the end of a session', () => {
  let local: ReturnType<typeof storage>

  beforeEach(() => {
    local = storage({ 'ams.recent': JSON.stringify([{ id: 'a', title: 'Heat', kind: 'movie', at: 1 }]) })
    vi.stubGlobal('localStorage', local)
  })

  afterEach(() => vi.unstubAllGlobals())

  it('tells whoever reads who is looking, at once, and asks again', async () => {
    const queryClient = cache()
    let signedIn = true
    const me = new QueryObserver<Me>(queryClient, {
      queryKey: ['me'],
      queryFn: () => Promise.resolve({ canWrite: signedIn }),
    })
    const stop = me.subscribe(() => undefined)
    await vi.waitFor(() => expect(me.getCurrentResult().data).toEqual({ canWrite: true }))

    signedIn = false
    endSession(queryClient)
    // Emptying the cache left this reader the answer it had: the bar kept
    // "Admin" until the page was loaded again.
    expect(me.getCurrentResult().data).toBeUndefined()
    await vi.waitFor(() => expect(me.getCurrentResult().data).toEqual({ canWrite: false }))
    stop()
  })

  it('keeps nothing the account was answered or sent', async () => {
    const queryClient = cache()
    let signedIn = true
    // An answer nobody reads any more, and one a page still does.
    queryClient.setQueryData(['account', 'keys'], { keys: ['a key'] })
    const sessions = new QueryObserver<string[]>(queryClient, {
      queryKey: ['account', 'sessions'],
      queryFn: () => (signedIn ? Promise.resolve(['this one']) : Promise.reject(new Error('Unauthorized'))),
    })
    const stop = sessions.subscribe(() => undefined)
    await vi.waitFor(() => expect(sessions.getCurrentResult().data).toEqual(['this one']))
    // What was sent, and what came back.
    await new MutationObserver(queryClient, {
      mutationFn: (name: string) => Promise.resolve({ name, secret: 'shown once' }),
    }).mutate('a key')
    expect(queryClient.getMutationCache().getAll()).toHaveLength(1)

    signedIn = false
    endSession(queryClient)
    expect(queryClient.getQueryCache().find({ queryKey: ['account', 'keys'] })).toBeUndefined()
    expect(queryClient.getQueryData(['account', 'sessions'])).toBeUndefined()
    expect(queryClient.getMutationCache().getAll()).toEqual([])
    // Asked again, as whoever is here now, and refused: still nothing of theirs.
    await vi.waitFor(() => expect(sessions.getCurrentResult().status).toBe('error'))
    expect(sessions.getCurrentResult().data).toBeUndefined()
    stop()
  })

  it('drops an answer that was still on its way for the account that left', async () => {
    const queryClient = cache()
    let answer: (me: Me) => void = () => undefined
    const late = new Promise<Me>((resolve) => {
      answer = resolve
    })
    let asked = 0
    const me = new QueryObserver<Me>(queryClient, {
      queryKey: ['me'],
      queryFn: () => {
        asked += 1
        return asked === 1 ? late : Promise.resolve({ canWrite: false })
      },
    })
    const stop = me.subscribe(() => undefined)
    expect(asked).toBe(1)

    endSession(queryClient)
    answer({ canWrite: true })
    await vi.waitFor(() => expect(me.getCurrentResult().data).toEqual({ canWrite: false }))
    await new Promise((settled) => setTimeout(settled, 20))
    expect(me.getCurrentResult().data).toEqual({ canWrite: false })
    stop()
  })

  it('forgets the shelf of the works they opened', () => {
    endSession(cache())
    expect(local.kept.has('ams.recent')).toBe(false)
  })
})
