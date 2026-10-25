import { describe, expect, it } from 'vitest'

import { keyLabels, strokesOf } from './CommandPalette'

describe('a command’s keys', () => {
  it('are read as strokes: what is held, and the key', () => {
    expect(strokesOf('mod+s')).toEqual([{ key: 's', mod: true, shift: false, alt: false }])
    expect(strokesOf('shift+mod+enter')).toEqual([{ key: 'enter', mod: true, shift: true, alt: false }])
    expect(strokesOf('r')).toEqual([{ key: 'r', mod: false, shift: false, alt: false }])
    expect(strokesOf('/')).toEqual([{ key: '/', mod: false, shift: false, alt: false }])
  })

  it('may be two, one after the other', () => {
    expect(strokesOf('g c')).toEqual([
      { key: 'g', mod: false, shift: false, alt: false },
      { key: 'c', mod: false, shift: false, alt: false },
    ])
  })

  it('are shown as this machine’s keyboard writes them', () => {
    expect(keyLabels('mod+s', true)).toEqual([['⌘', 'S']])
    expect(keyLabels('mod+s', false)).toEqual([['Ctrl', 'S']])
    expect(keyLabels('shift+mod+enter', true)).toEqual([['⇧', '⌘', '↵']])
    expect(keyLabels('g c', false)).toEqual([['g'], ['c']])
    expect(keyLabels('r', true)).toEqual([['r']])
  })
})
