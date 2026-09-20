import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'

import { api } from '../lib/api'
import type { Settings, Stats } from '../lib/types'
import { Display, Label, Lock, Mono, Panel, PanelHead, Spinner, Tag } from '../components/ui'

export function Dashboard() {
  const stats = useQuery({ queryKey: ['stats'], queryFn: () => api.get<Stats>('/stats') })
  const settings = useQuery({ queryKey: ['settings'], queryFn: () => api.get<Settings>('/settings') })

  return (
    <div className="mx-auto max-w-5xl">
      <header className="reveal mb-10">
        <Label>Overview</Label>
        <Display className="mt-2">
          One source of truth
          <br />
          <span className="text-faint italic">for the whole stack.</span>
        </Display>
      </header>

      {stats.isPending ? (
        <Spinner />
      ) : stats.isError ? (
        <p className="text-[13px] text-rust">Could not load statistics.</p>
      ) : (
        <>
          {/* Asymmetric on purpose: the locked-field count is the number that
              says whether anyone is actually curating. */}
          <div className="grid gap-px overflow-hidden rounded-[2px] border border-line bg-line sm:grid-cols-2 lg:grid-cols-4">
            <Metric
              className="sm:col-span-2 lg:row-span-2"
              label="Locked fields"
              value={stats.data.overrides}
              hint="Manual edits no refresh will touch"
              big
              accent
            />
            <Metric label="Series" value={stats.data.series} />
            <Metric label="Movies" value={stats.data.movies} />
            <Metric label="API clients" value={stats.data.clients} />
            <Metric
              label="Audit entries"
              value={stats.data.auditEntries}
              hint="changes recorded"
            />
          </div>

          <div className="mt-8 grid gap-6 lg:grid-cols-2">
            <Panel className="reveal" style={{ animationDelay: '120ms' }}>
              <PanelHead
                title="Surfaces"
                aside={
                  <Link
                    to="/settings"
                    className="font-mono text-[10px] uppercase tracking-[0.12em] text-faint hover:text-phos"
                  >
                    configure
                  </Link>
                }
              />
              <dl className="divide-y divide-line">
                <Surface
                  path="/v1/tvdb/*"
                  who="Sonarr"
                  policy={settings.data?.arrPolicy}
                />
                <Surface path="/v1/movie/*" who="Radarr" policy={settings.data?.arrPolicy} />
                <Surface path="/3/*" who="TMDB clients" policy={settings.data?.tmdbPolicy} />
                <Surface path="/api/v1/*" who="This UI" policy={settings.data?.nativePolicy} />
              </dl>
            </Panel>

            <Panel className="reveal" style={{ animationDelay: '180ms' }}>
              <PanelHead title="How locking works" />
              <div className="space-y-4 px-5 py-5 text-[13px] leading-relaxed text-dim">
                <p className="flex gap-3">
                  <span className="mt-0.5 text-signal">
                    <Dot />
                  </span>
                  <span>
                    Providers write into <em className="not-italic text-paper">snapshots</em>. Every
                    refresh replaces them wholesale.
                  </span>
                </p>
                <p className="flex gap-3">
                  <span className="mt-0.5 text-phos">
                    <Lock />
                  </span>
                  <span>
                    Your edits live in a <em className="not-italic text-paper">separate table</em>{' '}
                    the refresh path never writes to. That is the lock — structural, not a
                    convention.
                  </span>
                </p>
                <p className="flex gap-3">
                  <span className="mt-0.5 text-faint">
                    <Dot />
                  </span>
                  <span>
                    Unlock a field and it goes back to whatever the provider says, on the next
                    refresh.
                  </span>
                </p>
              </div>
            </Panel>
          </div>
        </>
      )}
    </div>
  )
}

function Metric({
  label,
  value,
  hint,
  big,
  accent,
  className,
}: {
  label: string
  value: number
  hint?: string
  big?: boolean
  accent?: boolean
  className?: string
}) {
  return (
    <div className={`reveal flex flex-col justify-between gap-3 bg-surface px-5 py-5 ${className ?? ''}`}>
      <Label>{label}</Label>
      <div>
        <p
          className={`tabular font-display leading-none ${big ? 'text-7xl' : 'text-4xl'} ${
            accent ? 'text-phos' : 'text-paper'
          }`}
        >
          {value.toLocaleString()}
        </p>
        {hint && <p className="mt-2 text-[12px] text-faint">{hint}</p>}
      </div>
    </div>
  )
}

function Surface({ path, who, policy }: { path: string; who: string; policy?: string }) {
  return (
    <div className="flex items-center justify-between gap-4 px-5 py-3">
      <div className="min-w-0">
        <Mono className="text-paper">{path}</Mono>
        <p className="text-[12px] text-faint">{who}</p>
      </div>
      {policy && (
        <Tag tone={policy === 'open' ? 'bad' : policy === 'allowlist' ? 'auto' : 'good'}>
          {policy}
        </Tag>
      )}
    </div>
  )
}

function Dot() {
  return <span className="inline-block h-[13px] w-[13px] rounded-full border-2 border-current" />
}
