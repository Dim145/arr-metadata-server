import { afterEach, describe, expect, it, vi } from 'vitest'

import { bytes, clock, count, dayDistance, localDay, longDate, relative, runtime, score, shortDate, weekdayDate, year } from './format'

describe('the suite', () => {
  it('runs west of Greenwich, where a day misread as midnight UTC shows', () => {
    // Pacific time: eight hours behind in winter.
    expect(new Date('2026-01-15T12:00:00Z').getTimezoneOffset()).toBe(480)
  })
})

describe('a calendar date', () => {
  it('stays on its own day, where midnight UTC is the evening before', () => {
    expect(longDate('2008-01-20', 'en-GB')).toBe('20 January 2008')
    expect(shortDate('2008-01-20', 'en-GB')).toBe('20/01/2008')
    expect(weekdayDate('2009-09-21', 'en-GB')).toMatch(/^Monday,? 21 September 2009$/)
  })

  it('is filed under its own year, New Year’s Day included', () => {
    expect(year('2009-01-01')).toBe('2009')
  })

  it('has no time of day to show', () => {
    expect(clock('2008-01-20', 'en-GB')).toBeUndefined()
  })

  it('is nothing at all when it is missing or not a date', () => {
    expect(longDate(undefined, 'en-GB')).toBeUndefined()
    expect(longDate('', 'en-GB')).toBeUndefined()
    expect(longDate('soon', 'en-GB')).toBeUndefined()
    expect(year('TBA')).toBeUndefined()
  })
})

describe('a moment', () => {
  it('is shown at the reader’s own hour', () => {
    // 03:30 UTC on the 22nd is 20:30 on the 21st in Los Angeles.
    expect(clock('2026-09-22T03:30:00Z', 'en-GB')).toBe('20:30')
  })
})

describe('a runtime', () => {
  it('stays in minutes under the hour', () => {
    expect(runtime(48, 'en')).toBe('48 min')
    expect(runtime(48, 'fr')).toBe('48 min')
  })

  it('reads as hours past it, each language its own way', () => {
    expect(runtime(136, 'en')).toBe('2h 16m')
    expect(runtime(125, 'fr')).toBe('2 h 05')
    expect(runtime(120, 'en')).toBe('2h')
    expect(runtime(120, 'fr')).toBe('2 h')
  })

  it('is nothing when unknown or nought', () => {
    expect(runtime(undefined, 'en')).toBeUndefined()
    expect(runtime(0, 'en')).toBeUndefined()
  })
})

describe('figures', () => {
  it('counts bytes by 1024, to one decimal past the first unit', () => {
    expect(bytes(512, 'en')).toBe('512 B')
    expect(bytes(1536, 'en')).toBe('1.5 kB')
    expect(bytes(64_068_812, 'en')).toBe('61.1 MB')
  })

  it('groups a count as the reader’s language does, and dashes a missing one', () => {
    expect(count(28_707, 'en')).toBe('28,707')
    expect(count(28_707, 'fr')).toBe('28 707')
    expect(count(undefined, 'en')).toBe('—')
  })

  it('gives a score one decimal, and no score for nought', () => {
    expect(score(8, 'fr')).toBe('8,0')
    expect(score(7.46, 'en')).toBe('7.5')
    expect(score(0, 'en')).toBeUndefined()
    expect(score(undefined, 'en')).toBeUndefined()
  })
})

describe('days from today', () => {
  // A Thursday at breakfast.
  const now = new Date(2026, 9, 15, 8, 0)

  it('is the reader’s own date of a moment', () => {
    expect(localDay(new Date(2026, 8, 21, 23, 59))).toBe('2026-09-21')
  })

  it('counts calendar days, not elapsed hours', () => {
    expect(dayDistance('2026-10-15', 'en', now)).toBe('today')
    // Thirty-eight hours away is Friday night: tomorrow, not "in 2 days".
    expect(dayDistance('2026-10-16', 'en', now)).toBe('tomorrow')
    expect(dayDistance('2026-10-18', 'en', now)).toBe('in 3 days')
    expect(dayDistance('2026-10-13', 'en', now)).toBe('2 days ago')
  })
})

describe('time since', () => {
  afterEach(() => vi.useRealTimers())

  it('says the largest unit that fits', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-09-21T12:00:00Z'))
    expect(relative('2026-09-18T12:00:00Z', 'en')).toBe('3 days ago')
    expect(relative('2026-09-21T11:58:00Z', 'en')).toBe('2 minutes ago')
    expect(relative('2026-09-21T14:00:00Z', 'en')).toBe('in 2 hours')
    expect(relative(undefined, 'en')).toBeUndefined()
  })
})
