import { useMutation, useQuery } from '@tanstack/react-query'

import { api } from '../lib/api'
import type { Settings } from '../lib/types'
import {
  Alert,
  Button,
  Display,
  Label,
  Mono,
  Panel,
  PanelHead,
  Spinner,
  Tag,
} from '../components/ui'

export function SettingsPage() {
  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/settings'),
  })

  const clearCache = useMutation({ mutationFn: () => api.post('/cache/clear') })

  if (settings.isPending) return <Spinner />
  if (settings.isError) return <Alert>Could not load settings.</Alert>

  const config = settings.data

  return (
    <div className="mx-auto max-w-3xl">
      <header className="reveal mb-8">
        <Label>Settings</Label>
        <Display className="mt-2">How this server is running</Display>
        <p className="mt-3 max-w-prose text-[13px] leading-relaxed text-dim">
          Configuration comes from the environment and is read once at startup. This page reports
          what took effect; change a value and restart to alter it.
        </p>
      </header>

      {config.authDisabled && (
        <div className="reveal mb-6">
          <Alert>
            <strong>Authentication is switched off.</strong> Every surface is open to anyone who can
            reach this server. Unset AMS_AUTH_DISABLED before exposing it.
          </Alert>
        </div>
      )}

      <div className="grid gap-6 lg:grid-cols-2">
        <Panel className="reveal" style={{ animationDelay: '60ms' }}>
          <PanelHead title="Runtime" />
          <dl className="divide-y divide-line">
            <Row label="version" value={config.version} />
            <Row label="database" value={config.database} />
            <Row label="public url" value={config.publicUrl ?? '—'} />
            <Row
              label="tmdb"
              value={config.tmdbConfigured ? `configured · ${config.tmdbLanguage}` : 'no API key'}
              tone={config.tmdbConfigured ? 'good' : 'bad'}
            />
            <Row
              label="skyhook fallback"
              value={config.skyhookFallback ? 'enabled' : 'disabled'}
            />
            <Row
              label="auto refresh"
              value={config.refreshEnabled ? 'enabled' : 'disabled'}
              tone={config.refreshEnabled ? 'auto' : 'neutral'}
            />
          </dl>
        </Panel>

        <Panel className="reveal" style={{ animationDelay: '100ms' }}>
          <PanelHead title="Access policy" />
          <dl className="divide-y divide-line">
            <Policy path="/api/v1/*" policy={config.nativePolicy} />
            <Policy path="/3/*" policy={config.tmdbPolicy} />
            <Policy path="/v1/*" policy={config.arrPolicy} />
          </dl>
          <div className="border-t border-line px-5 py-4">
            <p className="text-[12px] leading-relaxed text-faint">
              <Mono className="text-dim">allowlist</Mono> means the caller's address must match
              AMS_ARR_ALLOWLIST. Sonarr and Radarr have their metadata URLs compiled in and cannot
              present a key, so that is the only control available to them.
            </p>
          </div>
        </Panel>
      </div>

      <Panel className="reveal mt-6" style={{ animationDelay: '140ms' }}>
        <PanelHead title="Maintenance" />
        <div className="flex flex-wrap items-center justify-between gap-4 p-5">
          <div>
            <p className="text-[14px] text-paper">Clear the in-process cache</p>
            <p className="mt-1 max-w-prose text-[12px] text-faint">
              Discards cached entities and search results. The database is untouched, and the next
              request re-reads it and re-applies every lock.
            </p>
          </div>
          <Button onClick={() => clearCache.mutate()} disabled={clearCache.isPending}>
            {clearCache.isSuccess ? 'Cleared' : 'Clear'}
          </Button>
        </div>
      </Panel>
    </div>
  )
}

function Row({
  label,
  value,
  tone,
}: {
  label: string
  value: string
  tone?: 'good' | 'bad' | 'auto' | 'neutral'
}) {
  return (
    <div className="flex items-baseline justify-between gap-4 px-5 py-2.5">
      <Label>{label}</Label>
      {tone ? <Tag tone={tone}>{value}</Tag> : <Mono className="text-dim">{value}</Mono>}
    </div>
  )
}

function Policy({ path, policy }: { path: string; policy: string }) {
  return (
    <div className="flex items-baseline justify-between gap-4 px-5 py-2.5">
      <Mono className="text-paper">{path}</Mono>
      <Tag tone={policy === 'open' ? 'bad' : policy === 'allowlist' ? 'auto' : 'good'}>
        {policy}
      </Tag>
    </div>
  )
}
