import { describe, expect, it } from 'vitest'

import { inline, isInside, paragraphs, plain, problem, MAX_NOTE } from './note'

const text = (value: string) => ({ kind: 'text' as const, text: value })

describe('a note', () => {
  it('is read as text, emphasis, strong emphasis and links', () => {
    expect(inline('A *quiet* film, **very** much [a drama](/genre/drama).')).toEqual([
      text('A '),
      { kind: 'em', children: [text('quiet')] },
      text(' film, '),
      { kind: 'strong', children: [text('very')] },
      text(' much '),
      { kind: 'link', children: [text('a drama')], to: '/genre/drama' },
      text('.'),
    ])
    // Emphasis inside strong emphasis, and the other way round.
    expect(inline('**a *b* c**')).toEqual([
      { kind: 'strong', children: [text('a '), { kind: 'em', children: [text('b')] }, text(' c')] },
    ])
    expect(inline('*a **b** c*')).toEqual([
      { kind: 'em', children: [text('a '), { kind: 'strong', children: [text('b')] }, text(' c')] },
    ])
  })

  it('keeps a marker nothing closes as itself', () => {
    expect(inline('5 * 3 = 15')).toEqual([text('5 * 3 = 15')])
    expect(inline('a *b')).toEqual([text('a *b')])
    expect(inline('**')).toEqual([text('**')])
    expect(inline('[no link]')).toEqual([text('[no link]')])
    expect(inline('[spaced](/a b)')).toEqual([text('[spaced](/a b)')])
    // Escaped, a marker is a character.
    expect(inline('\\*not\\* \\[a\\](/b)')).toEqual([text('*not* [a](/b)')])
  })

  it('is cut into paragraphs at its empty lines', () => {
    expect(paragraphs('One\ntwo\n\nThree')).toEqual(['One two', 'Three'])
    expect(paragraphs('\n\n')).toEqual([])
  })

  it('reads as one line of plain text for a preview', () => {
    expect(plain('A *quiet* film.\n\nSee [the dramas](/genre/drama), **all** of them.')).toBe(
      'A quiet film. See the dramas, all of them.',
    )
  })

  it('links only to pages of this site', () => {
    expect(isInside('/genre/drama')).toBe(true)
    expect(isInside('/browse?genre=drama&kind=movie')).toBe(true)
    for (const outside of ['https://example.com', 'javascript:alert(1)', '//example.com/a', '/a<b', 'genre/drama']) {
      expect(isInside(outside), outside).toBe(false)
    }
  })

  it('says what the server would refuse before it is sent', () => {
    expect(problem('<b>bold</b>')).toBe('html')
    expect(problem('a < b, <3')).toBeUndefined()
    expect(problem('[x](https://example.com)')).toBe('link')
    expect(problem('*[x](mailto:a@b.c)*')).toBe('link')
    expect(problem('[a](/genre/drama)')).toBeUndefined()
    expect(problem('a'.repeat(MAX_NOTE + 1))).toBe('long')
  })
})
