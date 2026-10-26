import { describe, expect, it } from 'vitest'

import { ADULT_POSTERS_PREFERENCE, readChoice } from './adultPosters'

describe('the choice of the posters for adults, kept with the account', () => {
  it('is kept under a name the server takes', () => {
    // `clean_preference_key`, in src/api/native/account.rs: words of
    // letters, digits and dashes joined by dots, the first starting with a
    // small letter, 64 characters at most. A name it refuses would be a
    // choice never kept, and sent again on every page.
    expect(ADULT_POSTERS_PREFERENCE.length).toBeLessThanOrEqual(64)
    expect(ADULT_POSTERS_PREFERENCE).toMatch(/^[a-z][A-Za-z\d-]*(?:\.[A-Za-z\d-]+)*$/)
  })

  it('reads as shown or veiled, and anything else as no choice at all', () => {
    expect(readChoice(true)).toBe(true)
    expect(readChoice(false)).toBe(false)
    for (const raw of ['shown', 'true', 1, 0, null, undefined, {}, []]) {
      expect(readChoice(raw), JSON.stringify(raw)).toBeUndefined()
    }
  })
})
