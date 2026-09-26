/**
 * What each source gives a work, and which wins.
 *
 * The merge's rules, laid out for the people who have to trust its results:
 * a row per thing a work holds, a column per provider in the order the merge
 * consults them, and in each cell the provider's rank for that row — the
 * first with a value gives it, the next fill in what it lacks. Lists gathered
 * from everyone, and authorities that replace the rest, say so instead of a
 * number. Every mark is a word for a screen reader too.
 */

import { useQuery } from '@tanstack/react-query'
import { Glyph, Label, Panel, PanelHead, Skeleton } from '../../components/ui'
import { api } from '../../lib/api'
import { cn } from '../../lib/cn'
import { useI18n, type Dict } from '../../lib/i18n'
import { providerName } from '../../lib/labels'
import type { SourceRule, SourceRules } from '../../lib/types'

const GROUPS = ['identity', 'release', 'classification', 'people', 'episodes'] as const

type Mark =
  | { kind: 'first' }
  | { kind: 'rank'; rank: number }
  | { kind: 'union' }
  | { kind: 'authority' }
  | { kind: 'either' }
  | { kind: 'none' }

/** What a provider is to a row. */
function markOf(row: SourceRule, provider: string): Mark {
  if (row.authorities?.includes(provider)) return { kind: 'authority' }
  const at = row.suppliers.indexOf(provider)
  if (at === -1) return { kind: 'none' }
  if (row.rule === 'union') return { kind: 'union' }
  if (row.rule === 'either') return { kind: 'either' }
  return at === 0 ? { kind: 'first' } : { kind: 'rank', rank: at + 1 }
}

function spoken(mark: Mark, t: Dict): string {
  const c = t.admin.sourcesPage.cell
  switch (mark.kind) {
    case 'first':
      return c.first
    case 'rank':
      return c.rank(mark.rank)
    case 'union':
      return c.union
    case 'authority':
      return c.authority
    case 'either':
      return c.either
    default:
      return c.none
  }
}

/** A cell's mark, drawn: a filled disc for the first, a ring for the rest. */
function MarkGlyph({ mark }: { mark: Mark }) {
  switch (mark.kind) {
    case 'first':
      return (
        <span className="inline-grid size-6 place-items-center rounded-full bg-slate font-mono text-[0.6875rem] font-semibold text-ink">
          1
        </span>
      )
    case 'rank':
      return (
        <span className="inline-grid size-6 place-items-center rounded-full border border-slate-deep font-mono text-[0.6875rem] text-bone-dim">
          {mark.rank}
        </span>
      )
    case 'union':
      return <span className="font-mono text-sm text-slate">∪</span>
    case 'either':
      return <span className="font-mono text-sm text-slate">∨</span>
    case 'authority':
      return <Glyph name="star" className="size-4 text-brass" />
    default:
      return <span className="text-bone-faint">·</span>
  }
}

export function Sources() {
  const { t } = useI18n()
  const p = t.admin.sourcesPage

  const rules = useQuery({
    queryKey: ['sources', 'rules'],
    queryFn: () => api.get<SourceRules>('/sources/rules'),
  })

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.sourcesNav}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>
      </header>

      {rules.isPending ? (
        <Skeleton className="h-[40rem] w-full" />
      ) : rules.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {p.loadFailed}
        </p>
      ) : (
        <Matrix rules={rules.data} />
      )}

      <div className="mt-6 grid gap-6 md:grid-cols-2">
        {(
          [
            ['manualTitle', 'manual', 'lock'],
            ['imdbTitle', 'imdb', 'star'],
            ['skyhookTitle', 'skyhook', 'tv'],
            ['fankaiTitle', 'fankai', 'film'],
          ] as const
        ).map(([title, body, glyph], index) => (
          <Panel key={title} className="rise" style={{ animationDelay: `${160 + index * 40}ms` }}>
            <PanelHead
              title={
                <span className="inline-flex items-center gap-2">
                  <Glyph name={glyph} className="size-3.5" />
                  {p.notes[title]}
                </span>
              }
            />
            <p className="px-5 py-4 text-sm leading-relaxed text-bone-dim">{p.notes[body]}</p>
          </Panel>
        ))}
      </div>
    </div>
  )
}

function Matrix({ rules }: { rules: SourceRules }) {
  const { t } = useI18n()
  const p = t.admin.sourcesPage
  const providers = rules.providers

  const label = (row: SourceRule) => p.rows[row.field] ?? row.field
  const only = (row: SourceRule) =>
    row.only ? (
      <span className="ml-2 rounded-full border border-rule-bright px-1.5 py-0.5 font-mono text-[0.625rem] tracking-[0.12em] text-bone-faint uppercase">
        {p.only[row.only]}
      </span>
    ) : null

  return (
    <Panel className="rise" style={{ animationDelay: '80ms' }}>
      <PanelHead title={p.matrix} action={<span className="label hidden sm:inline">{p.order}</span>} />

      {/* A phone gets each row as a sentence: who gives it, in order. Ten
          columns do not fit a thumb's width, and a table that scrolls
          sideways hides the one it is asked about. */}
      <div className="md:hidden">
        {GROUPS.map((group) => {
          const rows = rules.rows.filter((row) => row.group === group)
          if (!rows.length) return null
          return (
            <section key={group} aria-label={p.groups[group]}>
              <h2 className="label border-b border-rule bg-ink/40 px-5 py-2.5">{p.groups[group]}</h2>
              <ul className="divide-y divide-rule">
                {rows.map((row) => (
                  <li key={row.field} className="px-5 py-3">
                    <p className="text-sm text-bone">
                      {label(row)}
                      {only(row)}
                    </p>
                    <ol className="mt-2 flex flex-wrap gap-x-3 gap-y-1.5">
                      {[...(row.authorities ?? []), ...row.suppliers.filter((s) => !row.authorities?.includes(s))].map(
                        (provider) => {
                          const mark = markOf(row, provider)
                          const on = providers.find((q) => q.id === provider)?.on ?? false
                          return (
                            <li key={provider} className={cn('flex items-center gap-1.5 text-xs', on ? 'text-bone-dim' : 'text-bone-faint opacity-60')}>
                              <MarkGlyph mark={mark} />
                              {providerName(provider)}
                              <span className="sr-only">
                                {' '}
                                — {spoken(mark, t)}
                                {on ? '' : `, ${p.off}`}
                              </span>
                            </li>
                          )
                        },
                      )}
                    </ol>
                  </li>
                ))}
              </ul>
            </section>
          )
        })}
      </div>

      <div className="hidden overflow-x-auto md:block">
        <table className="w-full min-w-[56rem] border-collapse text-sm">
          <caption className="sr-only">{p.matrix}</caption>
          <thead>
            <tr className="border-b border-rule">
              <th scope="col" className="label px-5 py-3 text-left align-bottom">
                {p.field}
              </th>
              {providers.map((provider) => (
                <th key={provider.id} scope="col" className="px-2 py-3 text-center align-bottom font-normal">
                  <span
                    aria-hidden
                    className={cn(
                      'mx-auto mb-2 block size-2 rounded-full',
                      provider.on ? 'bg-moss' : 'bg-bone-faint',
                    )}
                  />
                  <span className={cn('label block', provider.on ? '' : 'opacity-60')}>{providerName(provider.id)}</span>
                  <span className="mt-1 block font-mono text-[0.625rem] text-bone-faint tabular-nums">{provider.rank}</span>
                  <span className="sr-only">{provider.on ? p.on : p.off}</span>
                </th>
              ))}
            </tr>
          </thead>
          {/* A body per group, its name heading the rows beneath it. */}
          {GROUPS.map((group) => {
            const rows = rules.rows.filter((row) => row.group === group)
            if (!rows.length) return null
            return (
              <tbody key={group}>
                <tr className="border-b border-rule bg-ink/40">
                  <th scope="rowgroup" colSpan={providers.length + 1} className="label px-5 py-2.5 text-left">
                    {p.groups[group]}
                  </th>
                </tr>
                {rows.map((row) => (
                  <tr key={row.field} className="border-b border-rule transition-colors duration-150 hover:bg-ink-high">
                    <th scope="row" className="px-5 py-2.5 text-left font-normal text-bone">
                      {label(row)}
                      {only(row)}
                    </th>
                    {providers.map((provider) => {
                      const mark = markOf(row, provider.id)
                      return (
                        <td key={provider.id} className={cn('px-2 py-2.5 text-center', provider.on ? '' : 'opacity-40')}>
                          <span aria-hidden>
                            <MarkGlyph mark={mark} />
                          </span>
                          {/* The column's header names the provider. */}
                          <span className="sr-only">{spoken(mark, t)}</span>
                        </td>
                      )
                    })}
                  </tr>
                ))}
              </tbody>
            )
          })}
        </table>
      </div>

      <ul className="flex flex-wrap items-center gap-x-5 gap-y-2 border-t border-rule px-5 py-3 text-xs text-bone-faint">
        {(
          [
            [{ kind: 'first' }, p.legend.first],
            [{ kind: 'rank', rank: 3 }, p.legend.rank],
            [{ kind: 'union' }, p.legend.union],
            [{ kind: 'authority' }, p.legend.authority],
            [{ kind: 'either' }, p.legend.either],
          ] as [Mark, string][]
        ).map(([mark, meaning]) => (
          <li key={meaning} className="flex items-center gap-2">
            <span aria-hidden>
              <MarkGlyph mark={mark} />
            </span>
            {meaning}
          </li>
        ))}
        <li className="flex items-center gap-2">
          <span aria-hidden className="block size-2 rounded-full bg-bone-faint" />
          {p.legend.off}
        </li>
      </ul>
    </Panel>
  )
}
