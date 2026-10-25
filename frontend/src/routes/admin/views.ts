/**
 * The catalogue's views: a search, its filters and its order, as the address
 * says them — a view is its address, so a saved one is the address kept
 * under a name, with the person's account (`catalogue.views`).
 */

/** What a record can lack, as the server's `missing` filter names it, in the order the filters are drawn. */
export const GAPS = ['poster', 'overview', 'imdb', 'refresh', 'divergingLock', 'duplicates', 'mediaFailed'] as const
export type Gap = (typeof GAPS)[number]

/** What the address may say of the register, in the order a view is written in. */
const VIEW_KEYS = ['term', 'kind', 'manual', 'refreshFailed', 'missing', 'sort', 'order'] as const

/** The orders the register's columns give, and the way each starts. */
export const COLUMN_SORTS = { title: 'asc', added: 'desc', refreshed: 'desc' } as const
export type ColumnSort = keyof typeof COLUMN_SORTS

/**
 * The view an address shows, written one way only — the same search, filters
 * and order are the same words — and without what is the register's own
 * way: no `sort=popularity`, no order that is its sort's own.
 */
export function viewOf(search: string | URLSearchParams): string {
  const params = typeof search === 'string' ? new URLSearchParams(search) : search
  const out = new URLSearchParams()
  const sort = params.get('sort')?.trim() ?? ''
  for (const key of VIEW_KEYS) {
    let value = params.get(key)?.trim() ?? ''
    if (key === 'missing') {
      const named = value.split(',').map((gap) => gap.trim())
      value = GAPS.filter((gap) => named.includes(gap)).join(',')
    }
    if (key === 'sort' && value === 'popularity') value = ''
    if (key === 'order' && (!sort || COLUMN_SORTS[sort as ColumnSort] === value || !['asc', 'desc'].includes(value))) value = ''
    if (key === 'refreshFailed' || key === 'manual') value = value === '1' || value === 'true' ? '1' : ''
    if (value) out.set(key, value)
  }
  return out.toString()
}

/** A view kept under a name. */
export interface SavedView {
  name: string
  query: string
}

/** The most views one person keeps, and the longest a view's name may be. */
export const VIEWS_MOST = 30
export const VIEW_NAME = 40

/** The views as they were kept, when they read as views; what does not is left out. */
export function readViews(raw: unknown): SavedView[] | undefined {
  if (!Array.isArray(raw)) return undefined
  return raw
    .filter(
      (view): view is SavedView =>
        typeof view === 'object' && view !== null && typeof (view as SavedView).name === 'string' && typeof (view as SavedView).query === 'string',
    )
    .map((view) => ({ name: view.name, query: viewOf(view.query) }))
    .slice(0, VIEWS_MOST)
}

/** The views the register offers of itself, before any a person saves. */
export const BUILT_IN = [
  { id: 'all', query: '' },
  { id: 'series', query: 'kind=series' },
  { id: 'films', query: 'kind=movie' },
  { id: 'added', query: 'sort=added' },
  { id: 'refreshed', query: 'sort=refreshed' },
] as const

export type BuiltInView = (typeof BUILT_IN)[number]['id']

/**
 * Why a view cannot be saved under `name` beside `saved`, or nothing when it
 * can: a name given, short, not one of the person's already; a view not
 * saved already — theirs, or one the register offers — and room for it.
 */
export function refusal(
  name: string,
  query: string,
  saved: SavedView[],
): { why: 'empty' | 'long' | 'taken' | 'many' } | { why: 'same'; saved?: string; builtIn?: BuiltInView } | null {
  const trimmed = name.trim()
  const same = saved.find((view) => view.query === query)
  if (same) return { why: 'same', saved: same.name }
  const offered = BUILT_IN.find((view) => view.query === query)
  if (offered) return { why: 'same', builtIn: offered.id }
  if (!trimmed) return { why: 'empty' }
  if (trimmed.length > VIEW_NAME) return { why: 'long' }
  if (saved.some((view) => view.name.toLocaleLowerCase() === trimmed.toLocaleLowerCase())) return { why: 'taken' }
  if (saved.length >= VIEWS_MOST) return { why: 'many' }
  return null
}
