/**
 * The fact table: one register for every history and list the administration
 * shows.
 *
 * Two ways to draw one. `TableScroll` with `Th`, `Td` and `Tr` is the plain
 * register: the row height, the hairline between rows, the right-aligned
 * figures, decided here rather than on each screen. `DataTable` is the same
 * register made dense, for the tables a person works through rather than
 * glances at:
 *
 * - its head stays at the top while the rows scroll under it;
 * - a column whose head is a button orders the rows, and says how (`aria-sort`);
 * - columns can be hidden and shown again, and the table can be set tighter,
 *   both kept with the person's account (`table.<id>`, `lib/preferences.ts`);
 * - under 600 pixels of its own width the rows become cards: the row's name
 *   at their head, each value after its column's name;
 * - the rows are walked one at a time with the arrow keys, opened with Enter
 *   and chosen with the space bar.
 *
 * A page describes its columns once (`Column`), asks `useDataTable` for the
 * table's state, and writes its rows as before, naming each cell's column.
 */

import {
  createContext,
  use,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
} from 'react'

import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import { usePreference } from '../../lib/preferences'
import { Button } from './buttons'
import { Segmented } from './choice'
import { Glyph } from './glyphs'
import { Menu } from './overlay'

/**
 * The plain register's scroller.
 *
 * Exists because a table wide enough to need it must scroll inside its own
 * frame, never by dragging the page sideways.
 */
export function TableScroll({
  children,
  className,
  label,
}: {
  children: ReactNode
  className?: string
  /** What the table is, for the region a keyboard user lands on. */
  label: string
}) {
  const ref = useRef<HTMLDivElement>(null)
  const overflowing = useOverflow(ref)

  // A tab stop only while there is something to scroll to. On a phone these
  // tables are wider than the screen, and on the screens whose rows hold no
  // link or button — the job history, the audit trail — a keyboard had no way
  // to reach the columns off the edge. On a desktop, where nothing overflows,
  // an extra stop per table would only be in the way.
  return (
    <div
      ref={ref}
      role={overflowing ? 'region' : undefined}
      aria-label={overflowing ? label : undefined}
      tabIndex={overflowing ? 0 : undefined}
      className={cn('w-full max-w-full overflow-x-auto overscroll-x-contain', className)}
    >
      {children}
    </div>
  )
}

/** Whether what a box holds is wider than the box, measured again as either changes size. */
function useOverflow(ref: React.RefObject<HTMLElement | null>): boolean {
  const [overflowing, setOverflowing] = useState(false)

  useEffect(() => {
    const element = ref.current
    if (!element) return
    const measure = () => setOverflowing(element.scrollWidth > element.clientWidth + 1)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    // The table itself too: a column shown again widens it without the box moving.
    if (element.firstElementChild) observer.observe(element.firstElementChild)
    return () => observer.disconnect()
  }, [ref])

  return overflowing
}

/* ── The dense table's description and state ─────────────────────────────── */

export type SortDirection = 'asc' | 'desc'

/** How the rows are ordered: by which column, which way. */
export interface Sort {
  column: string
  direction: SortDirection
}

/** How tightly the rows are set. */
export type Density = 'comfortable' | 'compact'

/** One column, described once for its head, its cells, the list of columns and the cards. */
export interface Column {
  id: string
  /** Its name: in its head, in the list of columns, and before its value on a card. */
  label: string
  /** What its cell is on a card: the row's name, the box that chooses it, its actions, or a line (the default). */
  kind?: 'primary' | 'select' | 'actions'
  /** Never hidden: the column a row is known by, the box that chooses it, its actions. */
  always?: boolean
  /** Hidden until somebody shows it. */
  hidden?: boolean
  /** Orders the rows when its head is pressed: this way first. */
  sort?: SortDirection
  align?: 'left' | 'right'
  /** As a table, shown from this width of it up (40, 52 or 64 rem); on a card, always. */
  from?: 'md' | 'lg' | 'xl'
  /** Whether a card shows it: `false` for what a phone can do without, such as identifiers. */
  card?: boolean
  /** The head's own classes: a width, a padding. */
  className?: string
}

/** What is kept of a table's layout: the changes from its own way, so a column added later is shown. */
interface Layout {
  hidden?: string[]
  shown?: string[]
  density?: Density
  sort?: Sort
}

function readLayout(raw: unknown): Layout | undefined {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return undefined
  const kept = raw as Record<string, unknown>
  const names = (value: unknown) =>
    Array.isArray(value) ? value.filter((one): one is string => typeof one === 'string') : undefined
  const sort = kept.sort as Partial<Sort> | undefined
  return {
    hidden: names(kept.hidden),
    shown: names(kept.shown),
    density: kept.density === 'compact' || kept.density === 'comfortable' ? kept.density : undefined,
    sort:
      sort && typeof sort.column === 'string' && (sort.direction === 'asc' || sort.direction === 'desc')
        ? { column: sort.column, direction: sort.direction }
        : undefined,
  }
}

/** A table's state: which columns show, how tight, in what order. */
export interface DataTableState {
  id: string
  columns: Column[]
  /** Whether a column is shown. */
  shows: (column: string) => boolean
  /** Show or hide a column. */
  toggle: (column: string, shown: boolean) => void
  density: Density
  setDensity: (density: Density) => void
  sort: Sort | undefined
  /** Order by a column: its own way first, the other way when it orders already. */
  sortBy: (column: string) => void
  /** Whether anything differs from the table's own way. */
  changed: boolean
  /** Back to the table's own way. */
  reset: () => void
  column: (id: string) => Column | undefined
}

/**
 * A table's state, its layout kept with the person's account under
 * `table.<id>`. The order is the page's to keep when it is the server's
 * (`sort` and `onSort`, as the catalogue keeps it in the address); else it
 * is kept with the layout, from `defaultSort`.
 */
export function useDataTable({
  id,
  columns,
  sort: asked,
  onSort,
  defaultSort,
}: {
  id: string
  columns: Column[]
  sort?: Sort
  onSort?: (sort: Sort) => void
  defaultSort?: Sort
}): DataTableState {
  const [layout, setLayout] = usePreference<Layout>(`table.${id}`, {}, readLayout)
  const byId = useMemo(() => new Map(columns.map((column) => [column.id, column])), [columns])

  const shows = useCallback(
    (name: string) => {
      const column = byId.get(name)
      if (!column) return true
      if (column.always) return true
      return column.hidden ? Boolean(layout.shown?.includes(name)) : !layout.hidden?.includes(name)
    },
    [byId, layout],
  )

  const keep = (next: Layout) => {
    const tidy: Layout = {
      hidden: next.hidden?.length ? next.hidden : undefined,
      shown: next.shown?.length ? next.shown : undefined,
      density: next.density === 'compact' ? 'compact' : undefined,
      sort: next.sort,
    }
    const empty = !tidy.hidden && !tidy.shown && !tidy.density && !tidy.sort
    setLayout(empty ? undefined : tidy)
  }

  const toggle = (name: string, shown: boolean) => {
    const column = byId.get(name)
    if (!column || column.always) return
    const without = (list: string[] | undefined) => (list ?? []).filter((one) => one !== name)
    if (column.hidden) keep({ ...layout, shown: shown ? [...without(layout.shown), name] : without(layout.shown) })
    else keep({ ...layout, hidden: shown ? without(layout.hidden) : [...without(layout.hidden), name] })
  }

  const sortable = (name: string | undefined) => Boolean(name && byId.get(name)?.sort)
  const sort = onSort ? asked : sortable(layout.sort?.column) ? layout.sort : defaultSort

  const sortBy = (name: string) => {
    const column = byId.get(name)
    if (!column?.sort) return
    const next: Sort =
      sort?.column === name
        ? { column: name, direction: sort.direction === 'asc' ? 'desc' : 'asc' }
        : { column: name, direction: column.sort }
    if (onSort) onSort(next)
    else keep({ ...layout, sort: next })
  }

  return {
    id,
    columns,
    shows,
    toggle,
    density: layout.density ?? 'comfortable',
    setDensity: (density) => keep({ ...layout, density }),
    sort,
    sortBy,
    changed: Boolean(layout.hidden?.length || layout.shown?.length || layout.density || (!onSort && layout.sort)),
    reset: () => keep({}),
    column: (name) => byId.get(name),
  }
}

/** A value to order rows by: text, a number, or nothing — which goes last whichever the way. */
export type SortValue = string | number | null | undefined

/**
 * The rows in the order a table's state asks for, read through `by`: one
 * reader per column that orders. Text in the reader's own alphabet, figures
 * by their value; a row with nothing goes last, the way it is ordered or not.
 */
export function sortRows<R>(rows: R[], sort: Sort | undefined, by: Record<string, (row: R) => SortValue>, locale?: string): R[] {
  const read = sort ? by[sort.column] : undefined
  if (!sort || !read) return rows
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' })
  const sign = sort.direction === 'asc' ? 1 : -1
  return rows
    .map((row, index) => ({ row, index, value: read(row) }))
    .sort((a, b) => {
      const none = (value: SortValue) => value === null || value === undefined || value === ''
      if (none(a.value) || none(b.value)) {
        return none(a.value) === none(b.value) ? a.index - b.index : none(a.value) ? 1 : -1
      }
      const order =
        typeof a.value === 'number' && typeof b.value === 'number'
          ? a.value - b.value
          : collator.compare(String(a.value), String(b.value))
      return order === 0 ? a.index - b.index : order * sign
    })
    .map(({ row }) => row)
}

/* ── The dense table, drawn ──────────────────────────────────────────────── */

interface TableContextValue {
  table: DataTableState
  /** Called with a row's id when Enter is pressed on it. */
  onOpen?: (row: string) => void
  /** Called with the row the focus is on, or rests on last. */
  onActive?: (row: string | null) => void
}

const TableContext = createContext<TableContextValue | null>(null)

/**
 * The roles a table keeps however it is laid out. A table drawn as cards is
 * no longer laid out as a table, and some engines then stop saying it is
 * one: the roles are given outright, in both layouts, so a screen reader
 * walks the cards as it walks the rows.
 */
const ROLE = {
  table: { role: 'table' },
  rowgroup: { role: 'rowgroup' },
  row: { role: 'row' },
  columnheader: { role: 'columnheader' },
  cell: { role: 'cell' },
} as const

/** Where a cell of each width tier is shown from, as a table. */
const FROM: Record<NonNullable<Column['from']>, string> = { md: 'md', lg: 'lg', xl: 'xl' }

/**
 * The dense table's frame. Its head stays at the top of the page while the
 * rows scroll; where the table is wider than its frame, the frame scrolls
 * both ways itself, its head at its own top.
 */
export function DataTable({
  table,
  label,
  children,
  onOpen,
  onActive,
  minWidth,
  className,
}: {
  table: DataTableState
  /** What the table is: its name to assistive technology. */
  label: string
  children: ReactNode
  onOpen?: (row: string) => void
  onActive?: (row: string | null) => void
  /** The narrowest the table is drawn as a table, below which its frame scrolls: `46rem`. */
  minWidth?: string
  className?: string
}) {
  const frame = useRef<HTMLDivElement>(null)
  const overflowing = useOverflow(frame)
  const context = useMemo(() => ({ table, onOpen, onActive }), [table, onOpen, onActive])

  // One row takes the Tab key at a time: the one the focus was on last, or
  // the first. Set on the rows themselves after each drawing, which knows
  // their order better than any count kept here.
  const active = useRef<string | null>(null)
  useLayoutEffect(() => {
    const rows = [...(frame.current?.querySelectorAll<HTMLTableRowElement>('tbody > tr[data-row]') ?? [])]
    if (!rows.length) return
    const current = rows.find((row) => row.dataset.row === active.current) ?? rows[0]!
    for (const row of rows) row.tabIndex = row === current ? 0 : -1
  })

  return (
    <TableContext value={context}>
      <div
        ref={frame}
        data-scrolls={overflowing ? '' : undefined}
        data-density={table.density}
        role={overflowing ? 'region' : undefined}
        aria-label={overflowing ? label : undefined}
        tabIndex={overflowing ? 0 : undefined}
        onFocus={(event) => {
          const row = (event.target as HTMLElement).closest<HTMLTableRowElement>('tbody > tr[data-row]')
          if (!row || row.dataset.row === active.current) return
          active.current = row.dataset.row ?? null
          for (const other of frame.current?.querySelectorAll<HTMLTableRowElement>('tbody > tr[data-row]') ?? []) {
            other.tabIndex = other === row ? 0 : -1
          }
          onActive?.(active.current)
        }}
        className={cn('dense w-full max-w-full', overflowing ? 'overflow-auto overscroll-contain' : 'overflow-visible', className)}
        style={minWidth ? ({ '--table-min': minWidth } as CSSProperties) : undefined}
      >
        <table {...ROLE.table} aria-label={label} className="w-full border-collapse text-left">
          {children}
        </table>
      </div>
    </TableContext>
  )
}

/** The head of a dense table: its one row of column heads. */
export function THead({ children }: { children: ReactNode }) {
  return (
    <thead {...ROLE.rowgroup}>
      <tr {...ROLE.row}>{children}</tr>
    </thead>
  )
}

/**
 * The rows of a dense table, walked with the arrow keys from row to row,
 * Home and End to the first and the last; Enter opens the row the focus is
 * on, the space bar ticks its box. Only while the row itself has the focus:
 * a field or a button inside it keeps its own keys.
 */
export function TBody({ children }: { children: ReactNode }) {
  const context = use(TableContext)

  const onKeyDown = (event: KeyboardEvent<HTMLTableSectionElement>) => {
    const row = event.target as HTMLElement
    if (!(row instanceof HTMLTableRowElement) || row.dataset.row === undefined) return
    if (event.altKey || event.ctrlKey || event.metaKey) return
    const rows = [...event.currentTarget.querySelectorAll<HTMLTableRowElement>(':scope > tr[data-row]')]
    const at = rows.indexOf(row)
    const next =
      event.key === 'ArrowDown'
        ? rows[at + 1]
        : event.key === 'ArrowUp'
          ? rows[at - 1]
          : event.key === 'Home'
            ? rows[0]
            : event.key === 'End'
              ? rows[rows.length - 1]
              : undefined
    if (next) {
      event.preventDefault()
      next.focus()
      next.scrollIntoView({ block: 'nearest' })
      return
    }
    if (event.key === 'Enter' && context?.onOpen) {
      event.preventDefault()
      context.onOpen(row.dataset.row)
    } else if (event.key === ' ') {
      const box = row.querySelector<HTMLInputElement>('input[type="checkbox"]')
      if (box) {
        event.preventDefault()
        box.click()
      }
    }
  }

  return (
    // eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- the keys are the rows', which take the focus one at a time
    <tbody {...ROLE.rowgroup} onKeyDown={onKeyDown}>
      {children}
    </tbody>
  )
}

/**
 * A column's head. In a dense table, named by `column`: hidden with it, and
 * a button that orders the rows where the column orders.
 */
export function Th({
  children,
  align,
  className,
  column,
}: {
  children?: ReactNode
  align?: 'left' | 'right'
  className?: string
  /** The column it heads, in a `DataTable`. */
  column?: string
}) {
  const context = use(TableContext)
  const { t } = useI18n()
  const described = column ? context?.table.column(column) : undefined

  if (!context || !column) {
    return (
      <th
        scope="col"
        className={cn('label border-b border-rule px-4 py-2.5 whitespace-nowrap', align === 'right' ? 'text-right' : 'text-left', className)}
      >
        {children}
      </th>
    )
  }

  const { table } = context
  if (!table.shows(column)) return null

  const right = (align ?? described?.align) === 'right'
  const sorted = table.sort?.column === column ? table.sort.direction : undefined
  const content = children ?? described?.label

  return (
    <th
      {...ROLE.columnheader}
      scope="col"
      data-column={column}
      data-cell={described?.kind ?? 'line'}
      data-from={described?.from ? FROM[described.from] : undefined}
      data-card={described?.card === false ? 'false' : undefined}
      aria-sort={sorted ? (sorted === 'asc' ? 'ascending' : 'descending') : undefined}
      className={cn(
        'dense-head label border-b border-rule px-4 py-2.5 whitespace-nowrap',
        right ? 'text-right' : 'text-left',
        described?.className,
        className,
      )}
    >
      {described?.sort ? (
        <button
          type="button"
          onClick={() => table.sortBy(column)}
          title={t.admin.table.sortBy(described.label)}
          className={cn(
            'label -mx-1.5 inline-flex min-h-8 cursor-pointer items-center gap-1 rounded-[6px] px-1.5 transition-colors duration-150',
            'hover:bg-ink-high hover:text-bone',
            sorted ? 'text-bone' : '',
            right && 'flex-row-reverse',
          )}
        >
          {content}
          <Glyph
            name={sorted === 'desc' ? 'chevronDown' : sorted === 'asc' ? 'chevronUp' : 'sort'}
            className={cn('size-3 shrink-0', sorted ? 'text-vermillion' : 'opacity-50')}
          />
        </button>
      ) : (
        content
      )}
    </th>
  )
}

export function Td({
  children,
  align,
  className,
  column,
}: {
  children?: ReactNode
  align?: 'left' | 'right'
  className?: string
  /** The column it is a cell of, in a `DataTable`. */
  column?: string
}) {
  const context = use(TableContext)
  const described = column ? context?.table.column(column) : undefined
  if (context && column && !context.table.shows(column)) return null

  const right = (align ?? described?.align) === 'right'
  const dense = Boolean(context && column)
  return (
    <td
      {...(dense ? ROLE.cell : {})}
      data-column={dense ? column : undefined}
      data-cell={dense ? (described?.kind ?? 'line') : undefined}
      data-label={dense ? described?.label : undefined}
      data-from={described?.from ? FROM[described.from] : undefined}
      data-card={described?.card === false ? 'false' : undefined}
      className={cn('px-4 py-2 align-middle text-sm text-bone-dim', right ? 'text-right' : 'text-left', className)}
    >
      {children}
    </td>
  )
}

/**
 * A body row: 45px of it, and a colour change on hover. Nothing moves. Any
 * other attribute — a `data-` handle on what the row is about — goes through.
 * In a dense table, `row` names it: it takes the focus, one row at a time.
 */
export function Tr({
  children,
  className,
  row,
  ...rest
}: {
  children: ReactNode
  className?: string
  /** What the row is, in a `DataTable`: what Enter opens, and the focus comes back to. */
  row?: string
} & Omit<HTMLAttributes<HTMLTableRowElement>, 'className' | 'children'>) {
  const context = use(TableContext)
  const dense = Boolean(context && row !== undefined)

  return (
    <tr
      {...rest}
      {...(dense ? ROLE.row : {})}
      data-row={dense ? row : undefined}
      // Reachable by the arrow keys; the one row Tab lands on is set by the table.
      tabIndex={dense ? -1 : undefined}
      className={cn(
        // Positioned so a row's primary link can stretch a pseudo-element over
        // the whole row: a title is 20px tall and a row is not, and the gap is
        // where taps land on nothing.
        'relative h-[45px] border-b border-rule transition-colors duration-150 last:border-0',
        'hover:bg-ink-high',
        dense && 'dense-row focus-visible:bg-ink-high focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-vermillion',
        className,
      )}
    >
      {children}
    </tr>
  )
}

/**
 * Which columns a table shows and how tightly it is set, behind one button:
 * a box per column that can be hidden, the two densities, and the way back
 * to the table's own. Kept with the person's account.
 */
export function TableOptions({ table, className }: { table: DataTableState; className?: string }) {
  const { t } = useI18n()
  const o = t.admin.table
  const optional = table.columns.filter((column) => !column.always)

  return (
    <Menu
      label={o.options}
      title={o.options}
      buttonClassName={(open) =>
        cn(
          'inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-full border border-rule-bright px-3 text-body-sm font-medium text-bone',
          'transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high',
          open && 'bg-ink-high',
          className,
        )
      }
      button={
        <>
          <Glyph name="sliders" className="size-4" />
          <span>{o.columns}</span>
          {table.changed ? (
            <span aria-hidden className="size-1.5 rounded-full bg-brass" />
          ) : null}
        </>
      }
      // Within the screen however short it is, the rest a scroll away.
      className="max-h-[min(36rem,calc(100dvh-5rem))] w-72 overflow-y-auto overscroll-contain p-3"
    >
      {/* The way back first, where it is in sight however tall the panel. */}
      {table.changed ? (
        <Button size="sm" variant="quiet" className="mb-2 -ml-2" onClick={table.reset}>
          <Glyph name="refresh" className="size-4" />
          {o.reset}
        </Button>
      ) : null}
      <fieldset>
        <legend className="label mb-1.5">{o.columns}</legend>
        <ul className="space-y-0.5">
          {optional.map((column) => (
            <li key={column.id}>
              <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-[6px] px-2 text-sm text-bone hover:bg-ink-high">
                <input
                  type="checkbox"
                  className="size-4 accent-vermillion"
                  checked={table.shows(column.id)}
                  onChange={(event) => table.toggle(column.id, event.target.checked)}
                />
                {column.label}
              </label>
            </li>
          ))}
        </ul>
      </fieldset>
      <div className="mt-3 space-y-1.5 border-t border-rule pt-3">
        <span className="label block">{o.density}</span>
        <Segmented<Density>
          label={o.density}
          value={table.density}
          onChange={table.setDensity}
          options={[
            { value: 'comfortable', label: o.comfortable },
            { value: 'compact', label: o.compact },
          ]}
        />
      </div>
      <p className="mt-3 text-xs leading-relaxed text-bone-faint">{o.kept}</p>
    </Menu>
  )
}
