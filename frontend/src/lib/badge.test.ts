import { afterEach, describe, expect, it, vi } from 'vitest'

import { keepBadge } from './badge'

/** An installed app, or a tab: its icon's badge, its page, and its moves to the back and the front. */
function app({ standalone = true } = {}) {
  let change = () => {}
  const icon = { setAppBadge: vi.fn(() => Promise.resolve()), clearAppBadge: vi.fn(() => Promise.resolve()) }
  const page = {
    visibilityState: 'visible',
    addEventListener: (_type: string, listener: () => void) => {
      change = listener
    },
  }
  vi.stubGlobal('navigator', icon)
  vi.stubGlobal('document', page)
  vi.stubGlobal('window', { matchMedia: () => ({ matches: standalone }), location: { pathname: '/' } })
  return {
    icon,
    behind: () => {
      page.visibilityState = 'hidden'
      change()
    },
    front: () => {
      page.visibilityState = 'visible'
      change()
    },
  }
}

/** The schedule's answer: these entries, and nothing else. */
function schedule(entries: unknown[]) {
  const asked = vi.fn((_url: string) => Promise.resolve(new Response(JSON.stringify({ entries, works: [], truncated: false }))))
  vi.stubGlobal('fetch', asked)
  return asked
}

/** Everything the answer set going has finished: a turn of the event loop. */
const settled = () => new Promise((resolve) => setTimeout(resolve, 0))

const episode = (airDate: string, airDateUtc?: string) => ({
  kind: 'episode',
  workId: 'w1',
  episode: { airDate, airDateUtc },
})

describe('the count on the installed app’s icon', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('is today’s episodes while the app is behind another, and nothing while it is in front', async () => {
    // Evening of the 19th in Los Angeles, where the suite runs: already the
    // 20th in UTC.
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date(2026, 9, 19, 20, 0))
    const asked = schedule([
      // At 19:00 on the 19th, here.
      episode('2026-10-20', '2026-10-20T02:00:00Z'),
      // The 19th, with no time: the network's date, the same everywhere.
      episode('2026-10-19'),
      // Tomorrow, and a film: neither is today's episode.
      episode('2026-10-20'),
      { kind: 'release', workId: 'w2', date: '2026-10-19', releaseTypes: ['digital'] },
    ])
    const { icon, behind, front } = app()

    keepBadge()
    expect(icon.clearAppBadge).toHaveBeenCalledTimes(1)
    await vi.waitFor(() => expect(asked).toHaveBeenCalledTimes(1))
    // The reader's day, and the same date at midnight UTC, where an episode
    // with no time is stored: whichever comes first, to whichever comes last.
    const url = new URL(asked.mock.calls[0]![0], 'http://catalogue.test')
    expect(url.pathname).toBe('/api/v1/calendar')
    expect(url.searchParams.get('from')).toBe('2026-10-19T00:00:00.000Z')
    expect(url.searchParams.get('to')).toBe('2026-10-20T07:00:00.000Z')

    // The answer read, the app is put behind another.
    await settled()
    behind()
    expect(icon.setAppBadge).toHaveBeenCalledWith(2)
    front()
    expect(icon.clearAppBadge).toHaveBeenCalledTimes(2)
  })

  it('says nothing of a day that has gone by since it was counted', async () => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date(2026, 9, 19, 23, 50))
    const asked = schedule([episode('2026-10-19')])
    const { icon, behind } = app()

    keepBadge()
    await vi.waitFor(() => expect(asked).toHaveBeenCalledTimes(1))
    await settled()
    vi.setSystemTime(new Date(2026, 9, 20, 0, 10))
    behind()
    expect(icon.setAppBadge).not.toHaveBeenCalled()
  })

  it('stays off the icon of a page opened in a tab', () => {
    const asked = schedule([episode('2026-10-19')])
    const { icon } = app({ standalone: false })

    keepBadge()
    expect(icon.clearAppBadge).not.toHaveBeenCalled()
    expect(asked).not.toHaveBeenCalled()
  })
})
