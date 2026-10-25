/**
 * The styleguide: every component of `components/ui/` in each of its states,
 * in both themes and both languages, side by side — one gallery a component.
 *
 * What each state looks like is written beside each component; this draws
 * it, so that a change to a component is seen in every state at once, and so
 * that the visual tests (e2e/visual.spec.ts) have one picture of each gallery
 * to hold the interface to.
 *
 * Not in what ships: App.tsx mounts it in a development build, and in a build
 * asked for it (`vite build --mode styleguide`), which is what the visual
 * tests read. Its own words — the galleries' names, the states' — are written
 * here, in English, rather than in the dictionaries, which every reader of
 * the site downloads for a page none of them is sent. The components' words
 * come from the dictionaries, in each column's language.
 *
 * Three things make a component show a state it is not in:
 *
 * - a theme worn by a part of the page (`data-theme`, styles.css), the page
 *   itself kept dark while the styleguide is open;
 * - a language spoken by a part of the page (`LanguageScope`, lib/i18n.tsx);
 * - the states a pointer or a keyboard puts a control in, forced: each rule
 *   of the stylesheet that asks for `:hover`, `:focus`, `:focus-visible`,
 *   `:focus-within` or `:active` is widened, once, to an attribute as well
 *   (`[data-sg-hover]`…), which a cell sets on what it shows hovered, reached
 *   or pressed. Nothing is styled for the styleguide: a forced state is drawn
 *   by the very rule that draws the real one.
 */

import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { Link, useParams } from 'react-router'

import {
  Button,
  ButtonLink,
  Chip,
  DataTable,
  Dialog,
  EmptyState,
  Field,
  FormField,
  GLYPHS,
  Genre,
  Glyph,
  IconButton,
  Input,
  Label,
  Lamp,
  MENU_ITEM,
  Menu,
  Mono,
  OnThisPage,
  Panel,
  PanelHead,
  PolicyChip,
  Provenance,
  SectionTitle,
  Segmented,
  Select,
  Shelf,
  Skeleton,
  Spinner,
  TBody,
  THead,
  TableOptions,
  TableScroll,
  Td,
  Textarea,
  Th,
  Toggle,
  Tr,
  type Column,
  type DataTableState,
  type LampTone,
} from '../components/ui'
import { cn } from '../lib/cn'
import { LANGS, LANGUAGES, LanguageScope, useI18n, type Dict, type Lang } from '../lib/i18n'
import { applyTheme, type Theme } from '../lib/theme'

/* ── States a pointer or a keyboard puts a control in ────────────────────── */

type Forced = 'hover' | 'focus' | 'focus-within' | 'active'

/**
 * Each pseudo-class a pointer or a keyboard sets, as a selector reads it:
 * whole (`:focus`, not the start of `:focus-visible`), and not an escaped
 * colon in a class's name (`.hover\:bg-ink-high`).
 */
const PSEUDO = /(?<![\\:]):(hover|focus-visible|focus-within|focus|active)(?![\w-])/g

/** Every rule that asks for one of those states, widened to the attribute that forces it as well. */
function widen(rules: CSSRuleList) {
  for (const rule of Array.from(rules)) {
    if (rule instanceof CSSStyleRule && !rule.selectorText.includes('data-sg-')) {
      const widened = rule.selectorText.replace(PSEUDO, (_, state: string) => `:is(:${state},[data-sg-${state}])`)
      if (widened !== rule.selectorText) rule.selectorText = widened
    }
    if (rule instanceof CSSImportRule) {
      if (rule.styleSheet) widen(rule.styleSheet.cssRules)
    } else if ('cssRules' in rule && rule.cssRules instanceof CSSRuleList) {
      widen(rule.cssRules)
    }
  }
}

function widenAll() {
  for (const sheet of Array.from(document.styleSheets)) {
    try {
      widen(sheet.cssRules)
    } catch {
      // A sheet from another origin cannot be read; none of the interface's is one.
    }
  }
}

/**
 * The stylesheets widened once the styleguide is drawn, and again whenever one
 * is added or replaced, as the development server does on every edit. A
 * widened rule still answers to the real state, so the page behaves as before,
 * here and after.
 */
function useForcedStates() {
  useLayoutEffect(() => {
    widenAll()
    const observer = new MutationObserver(widenAll)
    observer.observe(document.head, { childList: true, subtree: true, characterData: true })
    document.addEventListener('load', widenAll, true)
    return () => {
      observer.disconnect()
      document.removeEventListener('load', widenAll, true)
    }
  }, [])
}

/**
 * What a pointer resting on an element, a keyboard reaching it or a press on
 * it sets: on the element, and on what holds it, up to the cell — as the
 * browser marks a hovered element's ancestors hovered too.
 */
function force(element: Element, cell: Element, state: Forced) {
  if (state === 'focus') {
    element.setAttribute('data-sg-focus', '')
    element.setAttribute('data-sg-focus-visible', '')
  }
  for (let node: Element | null = element; node; node = node === cell ? null : node.parentElement) {
    if (state === 'focus' || state === 'focus-within') node.setAttribute('data-sg-focus-within', '')
    else node.setAttribute('data-sg-hover', '')
    if (state === 'active') node.setAttribute('data-sg-active', '')
  }
}

/**
 * The page in the dark theme while the styleguide is open: the light columns
 * wear their theme themselves, and the dark ones would otherwise wear the
 * light one a reader chose. Put back on the way out, and never remembered.
 */
function useDarkPage() {
  useLayoutEffect(() => {
    const was: Theme = document.documentElement.dataset.theme === 'light' ? 'light' : 'dark'
    applyTheme('dark', false)
    return () => applyTheme(was, false)
  }, [])
}

/* ── The matrix ──────────────────────────────────────────────────────────── */

const THEMES: Theme[] = ['dark', 'light']
const THEME_NAMES: Record<Theme, string> = { dark: 'Dark', light: 'Light' }

/** One state of a component: a row of its matrix. */
interface Row {
  /** Its name, as the component's comment names it. */
  state: string
  /** A state the pointer or the keyboard puts a control in, forced on `target`. */
  force?: Forced
  /** What in the cell takes the forced state: a selector, or else what the cell holds. */
  target?: string
  /** What opens over the page — a menu's panel, a tip — shown open: a selector. */
  open?: string
  /** Room for what opens over the rows below: a height. */
  room?: string
  /** The component in this state, in the cell's words. */
  draw: (t: Dict) => ReactNode
}

/**
 * A component's states down, its themes and languages across: four columns,
 * or the two themes alone where four would be too narrow for it (`wide`),
 * each state then drawn once in each language. A gallery without words
 * (`words={false}`) has the themes alone.
 */
function Matrix({ rows, wide = false, words = true }: { rows: Row[]; wide?: boolean; words?: boolean }) {
  const langs: Lang[] = words ? LANGS : ['en']
  const columns = THEMES.flatMap((theme): { theme: Theme; lang?: Lang }[] =>
    wide || !words ? [{ theme }] : langs.map((lang) => ({ theme, lang })),
  )
  const lines = rows.flatMap((row): { row: Row; lang?: Lang }[] =>
    wide && words ? langs.map((lang) => ({ row, lang })) : [{ row }],
  )

  return (
    <table className="w-full table-fixed border-separate border-spacing-0.5">
      <thead>
        <tr>
          <td className="w-32" />
          {columns.map(({ theme, lang }) => (
            <th key={`${theme}-${lang ?? ''}`} scope="col" className="label px-1 pb-2 text-left">
              {lang ? `${THEME_NAMES[theme]}, ${LANGUAGES[lang].name}` : THEME_NAMES[theme]}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {lines.map(({ row, lang }) => (
          <tr key={`${row.state}-${lang ?? ''}`}>
            <th scope="row" className="label py-4 pr-3 text-left align-top">
              {lang ? `${row.state}, ${LANGUAGES[lang].name}` : row.state}
            </th>
            {columns.map((column) => {
              const spoken = column.lang ?? lang ?? 'en'
              return (
                <td
                  key={`${column.theme}-${column.lang ?? ''}`}
                  data-theme={column.theme}
                  data-column={`${column.theme}-${spoken}`}
                  className="rounded-md border border-rule bg-ink p-4 align-top text-bone"
                >
                  <LanguageScope lang={spoken}>
                    <Drawn row={row} />
                  </LanguageScope>
                </td>
              )
            })}
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/**
 * A selector read inside the cell alone. Without `:scope`, `tbody tr` would
 * find a head's row too: the gallery's own table holds every cell in a body.
 */
function within(selector: string): string {
  return selector
    .split(',')
    .map((part) => `:scope ${part.trim()}`)
    .join(', ')
}

/** One state drawn in one cell: in the cell's words, with what is forced set and what opens opened. */
function Drawn({ row }: { row: Row }) {
  const { t } = useI18n()
  const cell = useRef<HTMLDivElement>(null)

  useLayoutEffect(() => {
    const root = cell.current
    if (!root || !row.force) return
    for (const element of root.querySelectorAll(row.target ? within(row.target) : ':scope > *')) force(element, root, row.force)
  })

  useEffect(() => {
    const root = cell.current
    if (!root || !row.open) return
    for (const popover of root.querySelectorAll<HTMLElement>(within(row.open))) {
      // Each its own, so that one opening does not close the others; and
      // where it opens, whatever the room: a panel near the foot of the
      // window would otherwise turn up over the row above, and a picture of
      // the gallery would depend on where the page was scrolled.
      popover.popover = 'manual'
      popover.style.setProperty('position-try-fallbacks', 'none')
      if (!popover.matches(':popover-open')) popover.showPopover()
    }
  })

  return (
    <div ref={cell} className={cn('relative', row.room)}>
      {row.draw(t)}
    </div>
  )
}

/* ── Samples ─────────────────────────────────────────────────────────────── */

const noop = () => {}

/** Works the samples hold, by their own titles, which no language translates. */
const FILMS = ['Paris, Texas', 'Stalker', 'Playtime', 'Ran', 'Vertigo', 'Persona', 'Le Samouraï', 'Tokyo Story', 'Close-Up', 'Yi Yi']
const WORKS: [title: string, year: number, kind: 'films' | 'series'][] = [
  ['Paris, Texas', 1984, 'films'],
  ['Twin Peaks', 1990, 'series'],
  ['Stalker', 1979, 'films'],
]

const SWATCHES: { group: string; names: string[] }[] = [
  { group: 'surfaces', names: ['--color-ink', '--color-ink-raised', '--color-ink-high', '--color-ink-top', '--color-rule', '--color-rule-bright', '--surface-overlay'] },
  { group: 'text', names: ['--color-bone', '--color-bone-dim', '--color-bone-faint'] },
  {
    group: 'palette',
    names: ['--color-vermillion', '--color-vermillion-bright', '--color-vermillion-deep', '--color-brass', '--color-brass-deep', '--color-slate', '--color-slate-deep', '--color-moss'],
  },
  { group: 'states', names: ['--color-status-ok', '--color-status-warn', '--color-status-error', '--color-selection'] },
  { group: 'film stock', names: [1, 2, 3, 4, 5, 6, 7, 8].map((n) => `--color-stock-${n}`) },
]

function Swatches({ names }: { names: string[] }) {
  return (
    <ul className="grid grid-cols-2 gap-x-4 gap-y-2.5 lg:grid-cols-3">
      {names.map((name) => (
        <li key={name} className="flex min-w-0 items-center gap-2.5">
          <span aria-hidden className="size-7 shrink-0 rounded-md border border-rule" style={{ backgroundColor: `var(${name})` }} />
          <Mono className="truncate text-bone-dim">{name.replace('--color-', '')}</Mono>
        </li>
      ))}
    </ul>
  )
}

/** The type scale, smallest first (styles.css), each step in the face it is usually set in. */
const STEPS: { step: string; className: string; words: (t: Dict) => string }[] = [
  { step: 'label · 11', className: 'font-mono text-label font-medium tracking-[0.12em] text-bone-faint uppercase', words: (t) => t.browse.filters },
  { step: 'caption · 12', className: 'label', words: (t) => t.browse.year },
  { step: 'xs · 12', className: 'text-xs text-bone-faint', words: (t) => t.browse.noResultsHint },
  { step: 'body-sm · 13', className: 'text-body-sm text-bone-dim', words: (t) => t.browse.noResultsHint },
  { step: 'sm · 14', className: 'text-sm', words: (t) => t.home.emptyHint },
  { step: 'lead · 15', className: 'text-lead', words: (t) => t.home.emptyHint },
  { step: 'base · 16', className: 'text-base', words: (t) => t.home.emptyHint },
  { step: 'lg · 18', className: 'font-display text-lg', words: (t) => t.home.empty },
  { step: 'xl · 20', className: 'font-display text-xl font-medium', words: (t) => t.browse.title },
  { step: '2xl · 24', className: 'font-display text-2xl font-medium', words: (t) => t.browse.title },
  { step: '3xl · 30', className: 'font-display text-3xl', words: (t) => t.home.title },
  { step: '4xl · 36', className: 'font-display text-4xl', words: (t) => t.home.title },
  { step: 'plate · 28–48', className: 'font-display text-plate leading-tight', words: () => 'Paris, Texas' },
]

const VARIANTS = ['primary', 'ghost', 'quiet', 'danger'] as const

function Buttons({ t, ...props }: { t: Dict } & Omit<React.ComponentProps<typeof Button>, 'variant'>) {
  const verbs = { primary: t.common.save, ghost: t.common.edit, quiet: t.common.cancel, danger: t.common.delete }
  return (
    <div className="flex flex-wrap gap-2">
      {VARIANTS.map((variant) => (
        <Button key={variant} variant={variant} {...props}>
          {verbs[variant]}
        </Button>
      ))}
    </div>
  )
}

function IconButtons({ t, disabled, busy }: { t: Dict; disabled?: boolean; busy?: boolean }) {
  return (
    <div className="flex gap-1">
      <IconButton glyph="pencil" label={t.common.edit} onClick={noop} disabled={disabled} busy={busy} />
      <IconButton glyph="trash" tone="danger" label={t.common.delete} onClick={noop} disabled={disabled} busy={busy} />
      <IconButton glyph="lock" pressed label={t.work.edited} onClick={noop} disabled={disabled} busy={busy} />
    </div>
  )
}

const LAMPS: [LampTone, (t: Dict) => string][] = [
  ['moss', (t) => t.labels.statuses.active],
  ['brass', (t) => t.labels.statuses.pending],
  ['vermillion', (t) => t.common.actionFailed],
  ['faint', (t) => t.labels.statuses.disabled],
]

function Choice({ t, disabled }: { t: Dict; disabled?: boolean }) {
  return (
    <Segmented
      label={t.browse.kind}
      value="all"
      onChange={noop}
      disabled={disabled}
      options={[
        { value: 'all', label: t.browse.all },
        { value: 'series', label: t.browse.series },
        { value: 'films', label: t.browse.films },
      ]}
    />
  )
}

/** A field of each kind in one state: a year with its name, an order, a note. */
function Fields({ t, state }: { t: Dict; state: 'rest' | 'filled' | 'disabled' | 'invalid' | 'hint' }) {
  const id = useId()
  const filled = state === 'filled' || state === 'disabled' || state === 'invalid'
  const invalid = state === 'invalid'
  const control = { disabled: state === 'disabled', 'aria-invalid': invalid || undefined }

  return (
    <div className="space-y-3">
      <FormField
        label={t.browse.year}
        htmlFor={`${id}-year`}
        hint={state === 'hint' ? t.browse.noResultsHint : undefined}
        error={invalid ? t.admin.works.compose.invalid.year : undefined}
      >
        <Input
          id={`${id}-year`}
          inputMode="numeric"
          placeholder={t.browse.anyYear}
          defaultValue={filled ? (invalid ? '84' : '1984') : undefined}
          {...control}
        />
      </FormField>
      <Select aria-label={t.browse.sort} defaultValue="popular" {...control}>
        <option value="relevant">{t.browse.orders.relevant}</option>
        <option value="popular">{t.browse.orders.popular}</option>
        <option value="rated">{t.browse.orders.rated}</option>
      </Select>
      <Textarea
        aria-label={t.nav.search}
        rows={2}
        placeholder={t.nav.searchPlaceholder}
        defaultValue={filled ? 'Paris, Texas' : undefined}
        {...control}
      />
    </div>
  )
}

function Switches({ t, disabled }: { t: Dict; disabled?: boolean }) {
  return (
    <div className="flex items-center gap-3">
      <Toggle checked={false} label={t.browse.manualOnly} onChange={noop} disabled={disabled} />
      <Toggle checked label={t.browse.manualOnly} onChange={noop} disabled={disabled} />
    </div>
  )
}

function PanelSample({ t, empty }: { t: Dict; empty?: boolean }) {
  return (
    <Panel label={t.browse.filters}>
      <PanelHead
        title={t.browse.filters}
        action={
          <Button size="sm" variant="quiet">
            {t.common.edit}
          </Button>
        }
      />
      {empty ? (
        <EmptyState size="line" title={t.home.empty} />
      ) : (
        <dl className="py-2">
          <Field label={t.browse.year}>1984</Field>
          <Field label={t.browse.kind} hint={t.work.fromProvider}>
            {t.browse.films}
          </Field>
        </dl>
      )}
    </Panel>
  )
}

/** Two dialogs, each opened from its own column, in that column's theme and words. */
function Dialogs({ t }: { t: Dict }) {
  const [open, setOpen] = useState<'question' | 'wide' | null>(null)
  const close = () => setOpen(null)

  return (
    <div className="flex flex-wrap gap-2">
      <Button onClick={() => setOpen('question')}>Open the question</Button>
      <Button onClick={() => setOpen('wide')}>Open the wide one</Button>
      <Dialog
        open={open === 'question'}
        title={t.admin.curated.deleteTitle}
        onClose={close}
        footer={
          <>
            <Button variant="quiet" onClick={close}>
              {t.common.cancel}
            </Button>
            <Button variant="danger" onClick={close}>
              {t.common.delete}
            </Button>
          </>
        }
      >
        <p>{t.admin.curated.deleteBody('Halloween')}</p>
      </Dialog>
      <Dialog
        size="lg"
        open={open === 'wide'}
        title={t.admin.table.options}
        onClose={close}
        footer={
          <Button variant="primary" onClick={close}>
            {t.common.save}
          </Button>
        }
      >
        <ul className="grid grid-cols-4 gap-3">
          {FILMS.slice(0, 8).map((title) => (
            <li key={title}>
              <div className="aspect-2/3 rounded-card border border-rule bg-ink-high" />
              <p className="mt-1.5 truncate text-caption text-bone-faint">{title}</p>
            </li>
          ))}
        </ul>
      </Dialog>
    </div>
  )
}

function MenuSample({ t }: { t: Dict }) {
  return (
    <Menu
      label={t.common.more}
      align="start"
      className="w-56"
      button={<Glyph name="more" className="size-4" />}
      buttonClassName={(open) =>
        cn(
          'grid size-11 cursor-pointer place-items-center rounded-card text-bone-faint transition-colors duration-150',
          'hover:bg-ink-high hover:text-bone',
          open && 'bg-ink-high text-bone',
        )
      }
    >
      <button type="button" className={MENU_ITEM}>
        <Glyph name="pencil" className="size-4" />
        {t.common.edit}
      </button>
      <button type="button" className={MENU_ITEM}>
        <Glyph name="refresh" className="size-4" />
        {t.common.refresh}
      </button>
      <button type="button" className={MENU_ITEM} disabled>
        <Glyph name="trash" className="size-4" />
        {t.common.delete}
      </button>
    </Menu>
  )
}

// Its cards are stand-ins, with nothing in them to focus: the row itself takes
// the keyboard, as a shelf of nothing focusable must (`tabIndex`).
function ShelfSample({ t }: { t: Dict }) {
  return (
    <Shelf label={t.browse.title} tabIndex={0} listClassName="gap-3 pb-1">
      {FILMS.map((title) => (
        <li key={title} className="w-28 shrink-0 snap-start">
          <div className="aspect-2/3 rounded-card border border-rule bg-ink-high" />
          <p className="mt-2 truncate text-sm text-bone">{title}</p>
        </li>
      ))}
    </Shelf>
  )
}

function columnsOf(t: Dict): Column[] {
  return [
    { id: 'title', label: t.browse.sortTitle, kind: 'primary', always: true, sort: 'asc' },
    { id: 'year', label: t.browse.year, sort: 'desc', align: 'right' },
    { id: 'kind', label: t.browse.kind },
  ]
}

/** A table's state held still: a layout nobody can change here, and no preference written. */
function tableOf(columns: Column[], state: Partial<DataTableState> = {}): DataTableState {
  return {
    id: 'styleguide',
    columns,
    shows: () => true,
    toggle: noop,
    density: 'comfortable',
    setDensity: noop,
    sort: undefined,
    sortBy: noop,
    changed: false,
    reset: noop,
    column: (id) => columns.find((column) => column.id === id),
    ...state,
  }
}

/** Three works, as the plain register, or as the dense table in a given state. */
function Works({ t, state }: { t: Dict; state?: Partial<DataTableState> }) {
  const kind = (of: 'films' | 'series') => (of === 'series' ? t.browse.series : t.browse.films)

  if (!state) {
    return (
      <TableScroll label={t.browse.title}>
        <table className="w-full text-left">
          <thead>
            <tr>
              <Th>{t.browse.sortTitle}</Th>
              <Th align="right">{t.browse.year}</Th>
              <Th>{t.browse.kind}</Th>
            </tr>
          </thead>
          <tbody>
            {WORKS.map(([title, year, of]) => (
              <Tr key={title}>
                <Td className="text-bone">{title}</Td>
                <Td align="right">{year}</Td>
                <Td>{kind(of)}</Td>
              </Tr>
            ))}
          </tbody>
        </table>
      </TableScroll>
    )
  }

  return (
    <DataTable table={tableOf(columnsOf(t), state)} label={t.browse.title}>
      <THead>
        <Th column="title" />
        <Th column="year" />
        <Th column="kind" />
      </THead>
      <TBody>
        {WORKS.map(([title, year, of]) => (
          <Tr key={title} row={title}>
            <Td column="title" className="text-bone">
              {title}
            </Td>
            <Td column="year">{year}</Td>
            <Td column="kind">{kind(of)}</Td>
          </Tr>
        ))}
      </TBody>
    </DataTable>
  )
}

/* ── The galleries ───────────────────────────────────────────────────────── */

/** What a menu's panel is, against the tip its button also has. */
const PANELS = '[popover]:not([data-tip])'

interface Gallery {
  id: string
  name: string
  /** What the component is for, in a line. */
  about: string
  draw: () => ReactNode
}

const GALLERIES: Gallery[] = [
  {
    id: 'colours',
    name: 'Colours',
    about: 'The tokens of styles.css, by what they are for. A page asks for a state’s colour (status-ok, -warn, -error), not the colour it happens to be.',
    draw: () => (
      <Matrix
        words={false}
        rows={SWATCHES.map(({ group, names }) => ({ state: group, draw: () => <Swatches names={names} /> }))}
      />
    ),
  },
  {
    id: 'type',
    name: 'Type',
    about: 'The scale, smallest first: the named steps of @theme and Tailwind’s own between them. Twelve pixels is the floor for anything read.',
    draw: () => (
      <Matrix rows={STEPS.map(({ step, className, words }) => ({ state: step, draw: (t) => <p className={className}>{words(t)}</p> }))} />
    ),
  },
  {
    id: 'glyphs',
    name: 'Glyphs',
    about: 'The icon set, drawn as strokes in the colour of the text around it.',
    draw: () => (
      <Matrix
        words={false}
        rows={[
          {
            state: 'every glyph',
            draw: () => (
              <ul className="grid grid-cols-4 gap-x-3 gap-y-4 lg:grid-cols-6">
                {GLYPHS.map((name) => (
                  <li key={name} className="flex flex-col items-center gap-1.5">
                    <Glyph name={name} className="size-5 text-bone" />
                    <span className="text-caption text-bone-faint">{name}</span>
                  </li>
                ))}
              </ul>
            ),
          },
        ]}
      />
    ),
  },
  {
    id: 'buttons',
    name: 'Buttons',
    about: 'An action in one of four weights: primary, ghost, quiet and danger.',
    draw: () => (
      <Matrix
        rows={[
          { state: 'rest', draw: (t) => <Buttons t={t} /> },
          { state: 'hover', force: 'hover', target: 'button', draw: (t) => <Buttons t={t} /> },
          { state: 'focus', force: 'focus', target: 'button', draw: (t) => <Buttons t={t} /> },
          { state: 'active', force: 'active', target: 'button', draw: (t) => <Buttons t={t} /> },
          { state: 'disabled', draw: (t) => <Buttons t={t} disabled /> },
          {
            state: 'loading',
            draw: (t) => (
              <div className="flex flex-wrap gap-2">
                <Button variant="primary" disabled>
                  <Spinner />
                  {t.common.saving}
                </Button>
                <Button disabled>
                  <Spinner />
                  {t.common.saving}
                </Button>
              </div>
            ),
          },
          { state: 'small', draw: (t) => <Buttons t={t} size="sm" /> },
          {
            state: 'as a link',
            draw: (t) => (
              <div className="flex flex-wrap gap-2">
                <ButtonLink variant="primary" href="/__styleguide/buttons">
                  {t.common.more}
                </ButtonLink>
                <ButtonLink href="/__styleguide/buttons">{t.common.more}</ButtonLink>
              </div>
            ),
          },
        ]}
      />
    ),
  },
  {
    id: 'glyph-buttons',
    name: 'Glyph buttons',
    about: 'A row’s action reduced to its glyph, its name said to a screen reader and shown after a rest. Quiet, danger, and pressed.',
    draw: () => (
      <Matrix
        rows={[
          { state: 'rest', draw: (t) => <IconButtons t={t} /> },
          { state: 'hover', force: 'hover', target: 'button', draw: (t) => <IconButtons t={t} /> },
          { state: 'focus', force: 'focus', target: 'button', draw: (t) => <IconButtons t={t} /> },
          { state: 'active', force: 'active', target: 'button', draw: (t) => <IconButtons t={t} /> },
          { state: 'disabled', draw: (t) => <IconButtons t={t} disabled /> },
          { state: 'loading', draw: (t) => <IconButtons t={t} busy /> },
        ]}
      />
    ),
  },
  {
    id: 'chips',
    name: 'Chips',
    about: 'A state or a source in a word: neutral, a person’s hand (brass), a machine’s answer (slate), the accent. Genres in their own film stock.',
    draw: () => (
      <Matrix
        rows={[
          {
            state: 'tones',
            draw: (t) => (
              <div className="flex flex-wrap gap-2">
                <Chip>{t.labels.status.continuing}</Chip>
                <Chip tone="manual">{t.work.edited}</Chip>
                <Chip tone="provider">{t.work.fromProvider}</Chip>
                <Chip tone="accent">{t.labels.policy.open}</Chip>
              </div>
            ),
          },
          {
            state: 'provenance',
            draw: (t) => (
              <div className="flex flex-wrap gap-2">
                <Provenance manual label={t.work.edited} />
                <Provenance manual={false} label={t.work.fromProvider} />
              </div>
            ),
          },
          {
            state: 'policies',
            draw: () => (
              <div className="flex flex-wrap gap-2">
                {['apikey', 'allowlist', 'open', 'disabled'].map((policy) => (
                  <PolicyChip key={policy} policy={policy} />
                ))}
              </div>
            ),
          },
          {
            state: 'genres',
            draw: () => (
              <div className="flex flex-wrap gap-2">
                {['Drama', 'Comedy', 'Crime', 'Science Fiction', 'Animation', 'Documentary'].map((name) => (
                  <Genre key={name} name={name} />
                ))}
              </div>
            ),
          },
          { state: 'genre, hover', force: 'hover', target: 'a', draw: () => <Genre name="Drama" to="/genre/drama" /> },
          { state: 'genre, focus', force: 'focus', target: 'a', draw: () => <Genre name="Drama" to="/genre/drama" /> },
        ]}
      />
    ),
  },
  {
    id: 'lamps',
    name: 'Lamps',
    about: 'A state in a word with a lit dot: well, wanting a look, failed, off. The word says it; the colour helps.',
    draw: () => (
      <Matrix
        rows={[
          {
            state: 'tones',
            draw: (t) => (
              <div className="flex flex-col gap-2.5">
                {LAMPS.map(([tone, word]) => (
                  <Lamp key={tone} tone={tone}>
                    {word(t)}
                  </Lamp>
                ))}
              </div>
            ),
          },
        ]}
      />
    ),
  },
  {
    id: 'choices',
    name: 'Choices',
    about: 'One of a few, in a row of pills: radio buttons, hidden and labelled.',
    draw: () => (
      <Matrix
        rows={[
          { state: 'rest', draw: (t) => <Choice t={t} /> },
          { state: 'hover', force: 'hover', target: 'label:nth-of-type(2)', draw: (t) => <Choice t={t} /> },
          { state: 'focus', force: 'focus', target: 'input:checked', draw: (t) => <Choice t={t} /> },
          { state: 'active', force: 'active', target: 'label:nth-of-type(2)', draw: (t) => <Choice t={t} /> },
          { state: 'disabled', draw: (t) => <Choice t={t} disabled /> },
        ]}
      />
    ),
  },
  {
    id: 'fields',
    name: 'Fields',
    about: 'A line, a list and a paragraph to fill in, and the name, the hint and the error a FormField gives them.',
    draw: () => (
      <Matrix
        rows={[
          { state: 'empty', draw: (t) => <Fields t={t} state="rest" /> },
          { state: 'filled', draw: (t) => <Fields t={t} state="filled" /> },
          { state: 'hover', force: 'hover', target: 'input, select, textarea', draw: (t) => <Fields t={t} state="filled" /> },
          { state: 'focus', force: 'focus', target: 'input, select, textarea', draw: (t) => <Fields t={t} state="filled" /> },
          { state: 'disabled', draw: (t) => <Fields t={t} state="disabled" /> },
          { state: 'error', draw: (t) => <Fields t={t} state="invalid" /> },
          { state: 'with a hint', draw: (t) => <Fields t={t} state="hint" /> },
        ]}
      />
    ),
  },
  {
    id: 'switches',
    name: 'Switches',
    about: 'A flag, off and on.',
    draw: () => (
      <Matrix
        rows={[
          { state: 'rest', draw: (t) => <Switches t={t} /> },
          { state: 'hover', force: 'hover', target: 'button', draw: (t) => <Switches t={t} /> },
          { state: 'focus', force: 'focus', target: 'button', draw: (t) => <Switches t={t} /> },
          { state: 'active', force: 'active', target: 'button', draw: (t) => <Switches t={t} /> },
          { state: 'disabled', draw: (t) => <Switches t={t} disabled /> },
        ]}
      />
    ),
  },
  {
    id: 'labels',
    name: 'Labels and headings',
    about: 'The caption above a value, figures that sit in columns, and a section’s heading.',
    draw: () => (
      <Matrix
        rows={[
          { state: 'label', draw: (t) => <Label>{t.browse.year}</Label> },
          { state: 'figures', draw: () => <Mono>1984 · 02:25:00 · tt0087884</Mono> },
          {
            state: 'section title',
            draw: (t) => (
              <SectionTitle
                className="mb-0"
                action={
                  <Button size="sm" variant="quiet">
                    {t.common.more}
                  </Button>
                }
              >
                {t.browse.title}
              </SectionTitle>
            ),
          },
        ]}
      />
    ),
  },
  {
    id: 'panels',
    name: 'Panels',
    about: 'A frame a page divides into: its head, its label-and-value rows, and the line it says when it holds nothing.',
    draw: () => (
      <Matrix
        wide
        rows={[
          { state: 'rest', draw: (t) => <PanelSample t={t} /> },
          { state: 'holding nothing', draw: (t) => <PanelSample t={t} empty /> },
        ]}
      />
    ),
  },
  {
    id: 'page-index',
    name: 'A page’s own index',
    about: 'A long page’s sections in a row that stays at the top. The section in view is marked as the reader scrolls, which a still page cannot show.',
    draw: () => {
      const entries = (t: Dict) => [
        { id: 'styleguide-one', label: t.browse.title },
        { id: 'styleguide-two', label: t.browse.filters },
        { id: 'styleguide-three', label: t.nav.seasons },
      ]
      return (
        <Matrix
          rows={[
            { state: 'rest', draw: (t) => <OnThisPage label={t.nav.mainNavigation} entries={entries(t)} /> },
            {
              state: 'hover',
              force: 'hover',
              target: 'li:nth-child(2) a',
              draw: (t) => <OnThisPage label={t.nav.mainNavigation} entries={entries(t)} />,
            },
            {
              state: 'focus',
              force: 'focus',
              target: 'li:first-child a',
              draw: (t) => <OnThisPage label={t.nav.mainNavigation} entries={entries(t)} />,
            },
          ]}
        />
      )
    },
  },
  {
    id: 'empty',
    name: 'Nothing to show',
    about: 'Nothing yet, nothing under these filters, or no answer — as a screen’s, or as one line of a panel’s. Offline, a failure says so instead (the visual tests draw it with the browser offline).',
    draw: () => (
      <Matrix
        wide
        rows={[
          { state: 'empty', draw: (t) => <EmptyState title={t.home.empty} hint={t.home.emptyHint} /> },
          {
            state: 'filtered',
            draw: (t) => (
              <EmptyState kind="filtered" title={t.browse.noResults} hint={t.browse.noResultsHint} action={<Button>{t.browse.clear}</Button>} />
            ),
          },
          {
            state: 'error',
            draw: (t) => <EmptyState kind="error" title={t.home.loadFailed} action={<Button>{t.common.retry}</Button>} />,
          },
          { state: 'a line', draw: (t) => <EmptyState size="line" title={t.home.empty} /> },
          {
            state: 'a line, failed',
            draw: (t) => (
              <EmptyState
                size="line"
                kind="error"
                title={t.common.actionFailed}
                action={
                  <Button size="sm" variant="quiet">
                    {t.common.retry}
                  </Button>
                }
              />
            ),
          },
        ]}
      />
    ),
  },
  {
    id: 'waiting',
    name: 'Waiting',
    about: 'The shape of what is on its way, which shows only after a beat.',
    draw: () => (
      <Matrix
        words={false}
        rows={[
          {
            state: 'posters',
            draw: () => (
              <div className="flex gap-3">
                <Skeleton className="aspect-2/3 w-28" />
                <Skeleton className="aspect-2/3 w-28" />
                <Skeleton className="aspect-2/3 w-28" />
              </div>
            ),
          },
          {
            state: 'lines',
            draw: () => (
              <div className="space-y-2">
                <Skeleton className="h-4 w-3/4" />
                <Skeleton className="h-4 w-1/2" />
              </div>
            ),
          },
          { state: 'a panel', draw: () => <Skeleton className="h-24 w-full rounded-panel" /> },
        ]}
      />
    ),
  },
  {
    id: 'dialogs',
    name: 'Dialogs',
    about: 'Asking before something cannot be taken back, and a choice that wants the room. Each column opens its own; the visual tests draw them open.',
    draw: () => <Matrix rows={[{ state: 'closed', draw: (t) => <Dialogs t={t} /> }]} />,
  },
  {
    id: 'menus',
    name: 'Menus and tips',
    about: 'A panel of actions under its button, and the name of a button that shows a glyph alone.',
    draw: () => (
      <Matrix
        rows={[
          { state: 'closed', draw: (t) => <MenuSample t={t} /> },
          { state: 'hover', force: 'hover', target: 'button[aria-expanded]', draw: (t) => <MenuSample t={t} /> },
          { state: 'focus', force: 'focus', target: 'button[aria-expanded]', draw: (t) => <MenuSample t={t} /> },
          { state: 'open', open: PANELS, room: 'h-52', draw: (t) => <MenuSample t={t} /> },
          {
            state: 'open, an item under the pointer',
            open: PANELS,
            force: 'hover',
            target: '[popover] button:nth-child(2)',
            room: 'h-52',
            draw: (t) => <MenuSample t={t} />,
          },
          {
            state: 'open, an item reached',
            open: PANELS,
            force: 'focus',
            target: '[popover] button:first-child',
            room: 'h-52',
            draw: (t) => <MenuSample t={t} />,
          },
          {
            state: 'a tip',
            open: '[data-tip]',
            room: 'pt-10',
            draw: (t) => <IconButton glyph="pencil" label={t.common.edit} onClick={noop} />,
          },
        ]}
      />
    ),
  },
  {
    id: 'shelves',
    name: 'Shelves',
    about: 'A row that scrolls sideways, its edge fading where it goes on, and two arrows to walk it.',
    draw: () => (
      <Matrix
        wide
        rows={[
          { state: 'rest', draw: (t) => <ShelfSample t={t} /> },
          { state: 'hover', force: 'hover', draw: (t) => <ShelfSample t={t} /> },
          { state: 'focus within', force: 'focus-within', draw: (t) => <ShelfSample t={t} /> },
        ]}
      />
    ),
  },
  {
    id: 'tables',
    name: 'Tables',
    about: 'The plain register, and the dense table a person works through: ordered, tighter, a row reached, as cards, and its options.',
    draw: () => (
      <Matrix
        wide
        rows={[
          { state: 'plain, a row under the pointer', force: 'hover', target: 'tbody tr:nth-child(2)', draw: (t) => <Works t={t} /> },
          { state: 'dense, ordered', draw: (t) => <Works t={t} state={{ sort: { column: 'year', direction: 'desc' } }} /> },
          { state: 'compact', draw: (t) => <Works t={t} state={{ density: 'compact' }} /> },
          { state: 'a row reached', force: 'focus', target: 'tbody tr:first-child', draw: (t) => <Works t={t} state={{}} /> },
          {
            state: 'as cards',
            draw: (t) => (
              <div className="max-w-sm">
                <Works t={t} state={{}} />
              </div>
            ),
          },
          {
            state: 'its options',
            draw: (t) => (
              <div className="flex flex-wrap gap-3">
                <TableOptions table={tableOf(columnsOf(t))} />
                <TableOptions table={tableOf(columnsOf(t), { changed: true })} />
              </div>
            ),
          },
          {
            state: 'its options, open',
            open: PANELS,
            room: 'h-96',
            draw: (t) => <TableOptions table={tableOf(columnsOf(t))} />,
          },
        ]}
      />
    ),
  },
]

/* ── The page ────────────────────────────────────────────────────────────── */

function GallerySection({ gallery }: { gallery: Gallery }) {
  const title = `${gallery.id}-name`
  return (
    <section aria-labelledby={title} data-gallery={gallery.id} className="mt-14 first:mt-0">
      <h2 id={title} className="font-display text-2xl font-medium text-bone">
        {gallery.name}
      </h2>
      <p className="mt-1.5 mb-5 max-w-3xl text-sm text-bone-dim">{gallery.about}</p>
      {gallery.draw()}
    </section>
  )
}

export function Styleguide() {
  const { gallery } = useParams()
  useDarkPage()
  useForcedStates()

  const shown = gallery ? GALLERIES.filter((entry) => entry.id === gallery) : GALLERIES

  // Its own words are English whatever the reader's language (see the top of
  // this file); each column says its own.
  return (
    <div lang="en" className="min-h-dvh bg-ink px-8 py-8 text-bone">
      <header className="mb-10 border-b border-rule pb-6">
        <h1 className="font-display text-4xl">Styleguide</h1>
        <p className="mt-2 max-w-3xl text-sm text-bone-dim">
          Every component of the interface in each of its states, in both themes and both languages. What each
          state is for is written beside each component, in <Mono>frontend/src/components/ui</Mono>; this page is
          only in a development build.
        </p>
        <nav aria-label="Galleries" className="mt-5">
          <ul className="flex flex-wrap gap-1.5">
            <li>
              <Link
                to="/__styleguide"
                aria-current={gallery ? undefined : 'page'}
                className="inline-flex min-h-9 items-center rounded-full px-3 text-body-sm text-bone-dim transition-colors duration-150 hover:text-bone aria-[current=page]:bg-ink-top aria-[current=page]:text-bone"
              >
                All
              </Link>
            </li>
            {GALLERIES.map((entry) => (
              <li key={entry.id}>
                <Link
                  to={`/__styleguide/${entry.id}`}
                  aria-current={entry.id === gallery ? 'page' : undefined}
                  className="inline-flex min-h-9 items-center rounded-full px-3 text-body-sm text-bone-dim transition-colors duration-150 hover:text-bone aria-[current=page]:bg-ink-top aria-[current=page]:text-bone"
                >
                  {entry.name}
                </Link>
              </li>
            ))}
          </ul>
        </nav>
      </header>
      <main>
        {shown.length ? (
          shown.map((entry) => <GallerySection key={entry.id} gallery={entry} />)
        ) : (
          <EmptyState title="No gallery by that name." hint="The galleries are listed above." />
        )}
      </main>
    </div>
  )
}
