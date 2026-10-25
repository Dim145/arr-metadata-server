import { describe, expect, it } from 'vitest'

import { timeOf, toneOf } from './Logs'

describe('a line of the log, as the page draws it', () => {
  it('says an error fails and a warning wants a look, and the rest is said', () => {
    expect(toneOf('error')).toBe('vermillion')
    expect(toneOf('warn')).toBe('brass')
    for (const level of ['info', 'debug', 'trace'] as const) {
      expect(toneOf(level)).toBe('faint')
    }
  })

  it('is timed to the millisecond, and a time it cannot read is shown as sent', () => {
    const at = new Date(2026, 9, 25, 21, 30, 5, 123).toISOString()
    expect(timeOf(at, 'en-GB')).toBe('21:30:05.123')
    expect(timeOf('not a time', 'en-GB')).toBe('not a time')
  })
})
