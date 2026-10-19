import { describe, expect, it } from 'vitest'

import { cn } from './cn'

describe('classes merged', () => {
  it('keep a named step of type beside a colour', () => {
    expect(cn('text-caption', 'text-bone-faint')).toBe('text-caption text-bone-faint')
    expect(cn('font-mono text-label uppercase', 'text-brass')).toBe('font-mono text-label uppercase text-brass')
  })

  it('let a later size replace an earlier one, named or not', () => {
    expect(cn('text-caption', 'text-sm')).toBe('text-sm')
    expect(cn('text-sm', 'text-body-sm')).toBe('text-body-sm')
    expect(cn('text-lead', 'text-[0.6875rem]')).toBe('text-[0.6875rem]')
  })
})
