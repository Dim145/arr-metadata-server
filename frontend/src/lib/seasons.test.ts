import { describe, expect, it } from 'vitest'

import { around, formatDay, isSeason, mondayOf, seasonOf, seasonPath, stepSeason, weekday, weeksOf } from './seasons'

describe('the week of a day', () => {
  it('starts on the Monday, the day itself when it is one', () => {
    expect(mondayOf('2026-09-21')).toBe('2026-09-21')
    expect(mondayOf('2026-09-27')).toBe('2026-09-21')
  })

  it('crosses into the month and the year before', () => {
    expect(mondayOf('2026-01-01')).toBe('2025-12-29')
  })

  it('is not moved by the clocks going back', () => {
    // 1 November 2026: the reader's clocks fall back that Sunday morning.
    expect(mondayOf('2026-11-01')).toBe('2026-10-26')
    expect(mondayOf('2026-11-02')).toBe('2026-11-02')
  })

  it('counts days from Monday, nought, to Sunday, six', () => {
    expect(weekday('2026-09-21')).toBe(0)
    expect(weekday('2026-09-27')).toBe(6)
  })
})

describe('the weeks of a quarter', () => {
  const weeks = weeksOf('2026-01-01', '2026-03-31')

  it('are named by their Mondays, the first one in the year before', () => {
    expect(weeks).toHaveLength(14)
    expect(weeks[0]).toEqual({ monday: '2025-12-29', first: '2026-01-01', last: '2026-01-04' })
  })

  it('are cut to the days the quarter has of them', () => {
    expect(weeks.at(-1)).toEqual({ monday: '2026-03-30', first: '2026-03-30', last: '2026-03-31' })
  })
})

describe('seasons', () => {
  it('are the calendar’s quarters, winter from January', () => {
    expect(seasonOf(new Date(2026, 0, 1))).toEqual({ year: 2026, season: 'winter' })
    expect(seasonOf(new Date(2026, 3, 1))).toEqual({ year: 2026, season: 'spring' })
    expect(seasonOf(new Date(2026, 11, 31))).toEqual({ year: 2026, season: 'autumn' })
  })

  it('step across the years both ways', () => {
    expect(stepSeason({ year: 2026, season: 'winter' }, -1)).toEqual({ year: 2025, season: 'autumn' })
    expect(stepSeason({ year: 2026, season: 'autumn' }, 1)).toEqual({ year: 2027, season: 'winter' })
    expect(stepSeason({ year: 2026, season: 'summer' }, 4)).toEqual({ year: 2027, season: 'summer' })
    expect(stepSeason({ year: 2026, season: 'spring' }, -5)).toEqual({ year: 2025, season: 'winter' })
  })

  it('are known by their names alone', () => {
    expect(isSeason('summer')).toBe(true)
    expect(isSeason('fall')).toBe(false)
    expect(isSeason(undefined)).toBe(false)
    expect(seasonPath({ year: 2026, season: 'winter' })).toBe('/seasons/2026/winter')
  })
})

describe('a chart’s days', () => {
  it('are formatted as the days they are, wherever the reader is', () => {
    expect(formatDay('2026-09-21', 'en-GB', { day: 'numeric', month: 'long' })).toBe('21 September')
  })

  it('are said from the reader’s today while that is worth saying', () => {
    const evening = new Date(2026, 8, 21, 22, 0)
    expect(around('2026-09-22', 'en', evening)).toBe('tomorrow')
    expect(around('2026-10-12', 'en', evening)).toBe('in 3 weeks')
    expect(around('2026-12-25', 'en', evening)).toBeUndefined()
  })
})
