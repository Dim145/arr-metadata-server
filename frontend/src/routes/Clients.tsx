import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { api } from '../lib/api'
import type { ApiClient } from '../lib/types'
import {
  Alert,
  Button,
  Display,
  Empty,
  Input,
  Label,
  Mono,
  Panel,
  PanelHead,
  Spinner,
  Tag,
} from '../components/ui'

const SCOPES = ['read', 'write', 'admin'] as const

export function Clients() {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [scopes, setScopes] = useState<string[]>(['read'])
  const [issued, setIssued] = useState<{ name: string; key: string } | null>(null)

  const clients = useQuery({
    queryKey: ['clients'],
    queryFn: () => api.get<ApiClient[]>('/clients'),
  })

  const refresh = () => void queryClient.invalidateQueries({ queryKey: ['clients'] })

  const create = useMutation({
    mutationFn: () => api.post<ApiClient & { key: string }>('/clients', { name, scopes }),
    onSuccess: (result) => {
      setIssued({ name: result.name, key: result.key })
      setName('')
      refresh()
    },
  })

  const toggle = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.patch(`/clients/${id}`, { isEnabled: enabled }),
    onSuccess: refresh,
  })

  const revoke = useMutation({
    mutationFn: (id: string) => api.delete(`/clients/${id}`),
    onSuccess: refresh,
  })

  return (
    <div className="mx-auto max-w-4xl">
      <header className="reveal mb-8">
        <Label>Clients</Label>
        <Display className="mt-2">Who may ask this server</Display>
        <p className="mt-3 max-w-prose text-[13px] leading-relaxed text-dim">
          A key authenticates the native API and the TMDB-compatible surface — in a header, or as
          the <Mono className="text-paper">api_key</Mono> query parameter a TMDB client already
          sends. Sonarr and Radarr cannot send one at all, so those routes are guarded by the IP
          allowlist instead.
        </p>
      </header>

      {issued && (
        <Panel className="reveal mb-6 border-phos/40 bg-phos/[0.05]">
          <div className="p-5">
            <Label className="text-phos">Key for {issued.name}</Label>
            <p className="mt-2 mb-3 text-[13px] text-dim">
              Copy it now. Only its hash is stored — this is the last time it is shown.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <code className="flex-1 rounded-[2px] border border-phos/30 bg-void px-3 py-2 font-mono text-[13px] break-all text-phos">
                {issued.key}
              </code>
              <Button
                variant="primary"
                onClick={() => void navigator.clipboard.writeText(issued.key)}
              >
                Copy
              </Button>
              <Button onClick={() => setIssued(null)}>Done</Button>
            </div>
          </div>
        </Panel>
      )}

      <Panel className="reveal mb-6" style={{ animationDelay: '60ms' }}>
        <PanelHead title="Issue a key" />
        <form
          className="flex flex-wrap items-end gap-4 p-5"
          onSubmit={(event) => {
            event.preventDefault()
            create.mutate()
          }}
        >
          <div className="flex min-w-48 flex-1 flex-col gap-1.5">
            <Label>Name</Label>
            <Input
              required
              placeholder="jellyseerr"
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label>Scopes</Label>
            <div className="flex gap-3 py-2">
              {SCOPES.map((scope) => (
                <label
                  key={scope}
                  className="flex cursor-pointer items-center gap-1.5 font-mono text-[11px] tracking-[0.1em] text-faint uppercase select-none hover:text-dim"
                >
                  <input
                    type="checkbox"
                    className="accent-phos"
                    checked={scopes.includes(scope)}
                    onChange={(event) =>
                      setScopes((current) =>
                        event.target.checked
                          ? [...current, scope]
                          : current.filter((s) => s !== scope),
                      )
                    }
                  />
                  {scope}
                </label>
              ))}
            </div>
          </div>

          <Button type="submit" variant="primary" disabled={create.isPending || !name.trim()}>
            {create.isPending ? <Spinner className="border-void/40 border-t-void" /> : 'Issue'}
          </Button>

          {create.isError && (
            <div className="w-full">
              <Alert>{create.error.message}</Alert>
            </div>
          )}
        </form>
      </Panel>

      <Panel className="reveal" style={{ animationDelay: '100ms' }}>
        <PanelHead title="Issued keys" />
        {clients.isPending ? (
          <div className="px-5 py-12 text-center">
            <Spinner />
          </div>
        ) : clients.isError ? (
          <div className="p-5">
            <Alert>Could not load clients.</Alert>
          </div>
        ) : clients.data.length === 0 ? (
          <Empty title="No keys yet" hint="Issue one above to let a client through." />
        ) : (
          <ul className="divide-y divide-line">
            {clients.data.map((client) => (
              <li key={client.id} className="edge flex flex-wrap items-center gap-4 px-5 py-3.5">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-[15px] text-paper">{client.name}</span>
                    {!client.isEnabled && <Tag tone="bad">disabled</Tag>}
                    {client.scopes.map((scope) => (
                      <Tag key={scope} tone={scope === 'admin' ? 'manual' : 'neutral'}>
                        {scope}
                      </Tag>
                    ))}
                  </div>
                  <div className="mt-1 flex flex-wrap gap-x-4">
                    <Mono className="text-[11px] text-faint">{client.keyPrefix}…</Mono>
                    <Mono className="text-[11px] text-faint">
                      {client.lastUsedAt
                        ? `last used ${new Date(client.lastUsedAt).toLocaleString()}`
                        : 'never used'}
                    </Mono>
                  </div>
                </div>

                <div className="flex gap-1.5">
                  <Button
                    onClick={() => toggle.mutate({ id: client.id, enabled: !client.isEnabled })}
                    disabled={toggle.isPending}
                  >
                    {client.isEnabled ? 'Disable' : 'Enable'}
                  </Button>
                  <Button
                    variant="danger"
                    onClick={() => revoke.mutate(client.id)}
                    disabled={revoke.isPending}
                  >
                    Revoke
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
        {revoke.isError && (
          <div className="p-5 pt-0">
            <Alert>{revoke.error.message}</Alert>
          </div>
        )}
      </Panel>
    </div>
  )
}
