import { describe, expect, it } from 'vitest'

import { sortRows } from './tables'

type Row = { name: string; count?: number | null }

const rows: Row[] = [
  { name: 'beta', count: 10 },
  { name: 'Alpha', count: 2 },
  { name: 'Gamma' },
  { name: 'alpha 10', count: 2 },
  { name: 'alpha 9', count: null },
]
const by = { name: (row: Row) => row.name, count: (row: Row) => row.count }
const names = (sorted: Row[]) => sorted.map((row) => row.name)

describe('rows ordered by a column', () => {
  it('are left as they came when nothing orders them', () => {
    expect(sortRows(rows, undefined, by)).toBe(rows)
    expect(sortRows(rows, { column: 'unknown', direction: 'asc' }, by)).toBe(rows)
  })

  it('order text as its reader’s alphabet does, figures within it by their value', () => {
    expect(names(sortRows(rows, { column: 'name', direction: 'asc' }, by, 'en'))).toEqual([
      'Alpha',
      'alpha 9',
      'alpha 10',
      'beta',
      'Gamma',
    ])
    expect(names(sortRows(rows, { column: 'name', direction: 'desc' }, by, 'en'))).toEqual([
      'Gamma',
      'beta',
      'alpha 10',
      'alpha 9',
      'Alpha',
    ])
  })

  it('order figures by their value, those with none last either way, ties as they came', () => {
    expect(names(sortRows(rows, { column: 'count', direction: 'asc' }, by))).toEqual(['Alpha', 'alpha 10', 'beta', 'Gamma', 'alpha 9'])
    expect(names(sortRows(rows, { column: 'count', direction: 'desc' }, by))).toEqual(['beta', 'Alpha', 'alpha 10', 'Gamma', 'alpha 9'])
  })

  it('leave the rows they were given as they were', () => {
    const before = names(rows)
    sortRows(rows, { column: 'count', direction: 'desc' }, by)
    expect(names(rows)).toEqual(before)
  })
})
