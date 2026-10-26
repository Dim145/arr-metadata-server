import { MutationObserver, QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ApiError } from './api'
import { endSession, sessionCaches } from './session'

/** Who is looking, as much of it as these need: whether they may write, and their account. */
interface Me {
  canWrite: boolean
  user?: { id: string }
}

/** A browser's storage, kept in a map. */
function storage(initial: Record<string, string> = {}) {
  const kept = new Map(Object.entries(initial))
  return {
    kept,
    getItem: (key: string) => kept.get(key) ?? null,
    setItem: (key: string, value: string) => void kept.set(key, value),
    removeItem: vi.fn((key: string) => void kept.delete(key)),
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

describe('a session ended somewhere else', () => {
  let local: ReturnType<typeof storage>

  beforeEach(() => {
    local = storage({ 'ams.recent': JSON.stringify([{ id: 'a', title: 'Heat', kind: 'movie', at: 1 }]) })
    vi.stubGlobal('localStorage', local)
  })

  afterEach(() => vi.unstubAllGlobals())

  /** A cache as the interface's: told of every refusal, and without its retries. */
  function listening() {
    const queryClient: QueryClient = new QueryClient({
      ...sessionCaches(() => queryClient),
      defaultOptions: { queries: { retry: false } },
    })
    return queryClient
  }

  /** The server's refusal for want of a credential. */
  const refusal = () => new ApiError(401, 'unauthorized', 'Unauthorized')

  const somebody: Me = { canWrite: true, user: { id: 'u1' } }
  const nobody: Me = { canWrite: false }

  /** Who is looking, read as the bar reads it, and how many times it was asked. */
  async function looking(queryClient: QueryClient, answer: () => Promise<Me>) {
    let asked = 0
    const me = new QueryObserver<Me>(queryClient, {
      queryKey: ['me'],
      queryFn: () => {
        asked += 1
        return answer()
      },
      staleTime: 5 * 60_000,
    })
    const stop = me.subscribe(() => undefined)
    await vi.waitFor(() => expect(me.getCurrentResult().data).toEqual(somebody))
    return { me, stop, asked: () => asked }
  }

  /** A page that reads something the server now refuses. */
  function refusedRead(queryClient: QueryClient, name: string) {
    return new QueryObserver(queryClient, {
      queryKey: ['users', name],
      queryFn: () => Promise.reject(refusal()),
    }).subscribe(() => undefined)
  }

  /** Long enough for anything still to come to have come. */
  const settled = () => new Promise((done) => setTimeout(done, 20))

  it('asks again who is looking when a read is refused, and ends the session when nobody is', async () => {
    const queryClient = listening()
    let session = true
    const { me, stop, asked } = await looking(queryClient, () => Promise.resolve(session ? somebody : nobody))
    queryClient.setQueryData(['account', 'keys'], { keys: ['a key'] })

    // Closed from another browser: nothing here hears of it but what is refused.
    session = false
    const page = refusedRead(queryClient, 'list')
    await vi.waitFor(() => expect(me.getCurrentResult().data).toEqual(nobody))
    expect(local.kept.has('ams.recent')).toBe(false)
    expect(queryClient.getQueryCache().find({ queryKey: ['account', 'keys'] })).toBeUndefined()
    // Asked again, then once more by whoever is here now, and no further.
    await settled()
    expect(asked()).toBe(3)
    page()
    stop()
  })

  it('takes a refusal to say who is looking, on a private site, as the answer, and asks nothing after it', async () => {
    const queryClient = listening()
    let session = true
    const { me, stop, asked } = await looking(queryClient, () =>
      session ? Promise.resolve(somebody) : Promise.reject(refusal()),
    )

    // A write, this time: refused like a read.
    session = false
    await expect(
      new MutationObserver(queryClient, { mutationFn: () => Promise.reject(refusal()) }).mutate(),
    ).rejects.toThrow('Unauthorized')
    await vi.waitFor(() => expect(local.kept.has('ams.recent')).toBe(false))
    // The account's name went with the session: a refused answer used to
    // leave the last one in place, and the bar with it.
    await vi.waitFor(() => expect(me.getCurrentResult().status).toBe('error'))
    expect(me.getCurrentResult().data).toBeUndefined()
    await settled()
    expect(asked()).toBe(3)
    expect(local.removeItem).toHaveBeenCalledTimes(1)
    stop()
  })

  it('ends nothing for a password typed wrong: the session goes on', async () => {
    const queryClient = listening()
    const { me, stop, asked } = await looking(queryClient, () => Promise.resolve(somebody))
    queryClient.setQueryData(['account', 'keys'], { keys: ['a key'] })

    // Changing it, with the wrong current one: refused the same way.
    await expect(
      new MutationObserver(queryClient, { mutationFn: () => Promise.reject(refusal()) }).mutate(),
    ).rejects.toThrow('Unauthorized')
    await vi.waitFor(() => expect(asked()).toBe(2))
    await settled()
    expect(me.getCurrentResult().data).toEqual(somebody)
    expect(queryClient.getQueryData(['account', 'keys'])).toEqual({ keys: ['a key'] })
    expect(local.kept.has('ams.recent')).toBe(true)
    stop()
  })

  it('asks once for refusals that come together, and ends the session once', async () => {
    const queryClient = listening()
    let session = true
    const { stop, asked } = await looking(queryClient, () => Promise.resolve(session ? somebody : nobody))

    // A page of the administration opening on a session gone: every read refused at once.
    session = false
    const pages = ['list', 'counts', 'invitations'].map((name) => refusedRead(queryClient, name))
    await vi.waitFor(() => expect(local.kept.has('ams.recent')).toBe(false))
    await settled()
    expect(asked()).toBe(3)
    expect(local.removeItem).toHaveBeenCalledTimes(1)
    for (const page of pages) page()
    stop()
  })

  it('asks nothing for a visitor, whose refusals are only the site’s', async () => {
    const queryClient = listening()
    let asked = 0
    const me = new QueryObserver<Me>(queryClient, {
      queryKey: ['me'],
      queryFn: () => {
        asked += 1
        return Promise.resolve(nobody)
      },
    })
    const stop = me.subscribe(() => undefined)
    await vi.waitFor(() => expect(me.getCurrentResult().data).toEqual(nobody))

    const page = refusedRead(queryClient, 'list')
    await settled()
    expect(asked).toBe(1)
    expect(local.kept.has('ams.recent')).toBe(true)
    page()
    stop()
  })

  it('ends nothing when the server cannot say', async () => {
    const queryClient = listening()
    let reachable = true
    const { me, stop, asked } = await looking(queryClient, () =>
      reachable ? Promise.resolve(somebody) : Promise.reject(new TypeError('Failed to fetch')),
    )

    reachable = false
    const page = refusedRead(queryClient, 'list')
    await vi.waitFor(() => expect(asked()).toBe(2))
    await settled()
    expect(me.getCurrentResult().data).toEqual(somebody)
    expect(local.kept.has('ams.recent')).toBe(true)
    page()
    stop()
  })

  it('ends it too when the answer, gone old, is asked again after the session ran out', async () => {
    const queryClient = listening()
    let session = true
    const { me, stop } = await looking(queryClient, () => Promise.resolve(session ? somebody : nobody))

    // A page drawn later asks again, as `useMe` does once the answer is old.
    session = false
    await me.refetch()
    await vi.waitFor(() => expect(local.kept.has('ams.recent')).toBe(false))
    expect(me.getCurrentResult().data).toEqual(nobody)
    stop()
  })

  it('is ended once by the way out: the answer that follows it asks nothing more', async () => {
    const queryClient = listening()
    let session = true
    const { me, stop, asked } = await looking(queryClient, () => Promise.resolve(session ? somebody : nobody))

    session = false
    endSession(queryClient)
    await vi.waitFor(() => expect(me.getCurrentResult().data).toEqual(nobody))
    await settled()
    expect(asked()).toBe(2)
    expect(local.removeItem).toHaveBeenCalledTimes(1)
    stop()
  })
})
