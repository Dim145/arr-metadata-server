import { describe, expect, it } from 'vitest'

import { draftKey, tabOfScope } from './drafts'

describe('a change not saved yet', () => {
  it('is named by its field, and by the language it is typed in where that matters', () => {
    expect(draftKey('item', 'title', '')).toBe('item|title|')
    expect(draftKey('item', 'title', 'fra')).not.toBe(draftKey('item', 'title', ''))
    expect(draftKey('episode:1x2', 'title', '')).toBe('episode:1x2|title|')
  })

  it('marks the tab that holds its field', () => {
    expect(tabOfScope('item')).toBe('record')
    expect(tabOfScope('season:3')).toBe('seasons')
    expect(tabOfScope('episode:3x7')).toBe('seasons')
  })
})
