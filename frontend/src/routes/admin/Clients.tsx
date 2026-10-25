/**
 * Who may ask this server.
 *
 * The screen turns on one moment: the few seconds a new key is on screen and
 * nowhere else. That panel gets the accent, the width and the largest type on
 * the page, and it does not go away on its own — it waits to be dismissed by
 * somebody who says they have the key, because the alternative is a client that
 * can never be made to work again.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { type MouseEvent as ReactMouseEvent, useEffect, useMemo, useRef, useState } from 'react'

import {
  Button,
  Chip,
  DataTable,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  IconButton,
  Input,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Skeleton,
  Spinner,
  TableOptions,
  TBody,
  Td,
  Th,
  THead,
  Toggle,
  Tr,
  sortRows,
  useDataTable,
  type Column,
} from '../../components/ui'
import { NetworkAccess } from '../../components/NetworkAccess'
import { ScopeSettingsDialog, useServerValues } from '../../components/ScopeSettings'
import { api } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { MCP_SETTING, claudeCommand, freeAgentName, mcpAddress } from '../../lib/mcp'
import { useI18n } from '../../lib/i18n'
import type { AccessReport, ApiClient, Settings } from '../../lib/types'

type Asking = { client: ApiClient; what: 'revoke' | 'disable' }

export function Clients() {
  const { t, locale } = useI18n()
  const queryClient = useQueryClient()

  const [name, setName] = useState('')
  const [scopes, setScopes] = useState<string[]>(['read'])
  const [issued, setIssued] = useState<{ name: string; key: string } | null>(null)
  const [asking, setAsking] = useState<Asking | null>(null)
  const [tuning, setTuning] = useState<ApiClient | null>(null)

  const clients = useQuery({ queryKey: ['clients'], queryFn: () => api.get<ApiClient[]>('/clients') })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['clients'] })
    void queryClient.invalidateQueries({ queryKey: ['stats'] })
  }

  const create = useMutation({
    mutationFn: () => api.post<ApiClient & { key: string }>('/clients', { name, scopes }),
    onSuccess: (result) => {
      setIssued({ name: result.name, key: result.key })
      setName('')
      invalidate()
    },
  })

  const setEnabled = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.patch(`/clients/${id}`, { isEnabled: enabled }),
    onSuccess: () => {
      setAsking(null)
      invalidate()
    },
  })

  const revoke = useMutation({
    mutationFn: (id: string) => api.delete(`/clients/${id}`),
    onSuccess: () => {
      setAsking(null)
      invalidate()
    },
  })

  const SCOPES = [
    ['read', t.admin.keys.read],
    ['write', t.admin.keys.write],
    ['admin', t.admin.keys.administer],
    ['metrics', t.admin.keys.metrics],
    ['mcp', t.admin.keys.mcp],
  ] as const

  // A key is known by its name. Its prefix only tells two keys of one name
  // apart, which a phone can do without.
  const columns: Column[] = useMemo(
    () => [
      { id: 'name', label: t.admin.keys.colName, kind: 'primary', always: true, sort: 'asc' },
      { id: 'scopes', label: t.admin.keys.colScopes },
      { id: 'prefix', label: t.admin.keys.colPrefix, from: 'lg', card: false },
      { id: 'lastUsed', label: t.admin.keys.colLastUsed, align: 'right', sort: 'desc', from: 'md' },
      { id: 'actions', label: t.admin.keys.colActions, kind: 'actions', always: true, align: 'right' },
    ],
    [t],
  )
  const table = useDataTable({ id: 'clients', columns })
  const rows = sortRows(
    clients.data ?? [],
    table.sort,
    {
      name: (client) => client.name,
      lastUsed: (client) => (client.lastUsedAt ? Date.parse(client.lastUsedAt) : null),
    },
    locale,
  )

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {t.admin.clients}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.admin.keys.lead}</p>
      </header>

      {/* Keyed by the key: the next one issued is a panel of its own, not this one
          still marked as copied. */}
      {issued ? <IssuedKey key={issued.key} issued={issued} onDismiss={() => setIssued(null)} /> : null}

      <Panel className="rise mb-6" style={{ animationDelay: '60ms' }}>
        <PanelHead title={t.admin.keys.issue} />
        <form
          className="flex flex-wrap items-start gap-5 p-5"
          onSubmit={(event) => {
            event.preventDefault()
            create.mutate()
          }}
        >
          <div className="min-w-48 flex-1">
            <FormField
              label={t.admin.keys.name}
              htmlFor="client-name"
              error={create.isError ? create.error.message : undefined}
            >
              <Input
                id="client-name"
                required
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </FormField>
          </div>

          <fieldset className="min-w-0">
            <legend className="label mb-1.5">{t.admin.keys.scopes}</legend>
            <div className="flex flex-wrap gap-2">
              {SCOPES.map(([scope, label]) => {
                const on = scopes.includes(scope)
                return (
                  <Button
                    key={scope}
                    size="sm"
                    variant="ghost"
                    // Pressed looks like the language toggle's pressed half:
                    // bone. The primary gradient is for the one action a
                    // screen asks for, and this is not that.
                    className={on ? 'border-bone bg-bone text-ink hover:bg-bone' : undefined}
                    aria-pressed={on}
                    onClick={() =>
                      setScopes((current) =>
                        on ? current.filter((s) => s !== scope) : [...current, scope],
                      )
                    }
                  >
                    {on ? <Glyph name="check" className="size-3.5" /> : null}
                    {label}
                  </Button>
                )
              })}
            </div>
            {scopes.includes('metrics') ? (
              <p className="mt-1.5 text-xs text-bone-dim">{t.admin.keys.metricsHint}</p>
            ) : null}
            {scopes.includes('mcp') ? (
              <p className="mt-1.5 text-xs text-bone-dim">{t.admin.keys.mcpHint}</p>
            ) : null}
          </fieldset>

          <Button
            type="submit"
            variant="primary"
            className="sm:mt-6"
            disabled={create.isPending || !name.trim() || scopes.length === 0}
          >
            {create.isPending ? <Spinner className="size-4" /> : <Glyph name="key" className="size-4" />}
            {t.admin.keys.issue}
          </Button>
        </form>
      </Panel>

      <Panel className="rise overflow-clip" style={{ animationDelay: '100ms' }}>
        <PanelHead
          title={t.admin.keys.issued}
          action={clients.data?.length ? <TableOptions table={table} /> : undefined}
        />

        {clients.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 3 }, (_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : clients.isError ? (
          <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-status-error">
            <Glyph name="alert" className="size-4" />
            {t.admin.keys.loadFailed}
          </p>
        ) : clients.data.length === 0 ? (
          <EmptyState title={t.admin.keys.empty} hint={t.admin.keys.emptyHint} />
        ) : (
          <DataTable table={table} label={t.admin.keys.issued}>
            <THead>
              <Th column="name" />
              <Th column="scopes" />
              <Th column="prefix" />
              <Th column="lastUsed" />
              <Th column="actions" />
            </THead>
            <TBody>
              {rows.map((client) => (
                <Tr key={client.id} row={client.id} className={client.isEnabled ? '' : 'opacity-60'}>
                  <Td column="name" className="w-full max-w-0">
                    <span className="flex items-center gap-2">
                      <span className="truncate text-sm font-medium text-bone">
                        {client.name}
                      </span>
                      {client.isEnabled ? null : (
                        <Chip tone="accent">{t.admin.works.disabled}</Chip>
                      )}
                    </span>
                  </Td>

                  <Td column="scopes">
                    <span className="flex flex-wrap gap-1.5">
                      {client.scopes.map((scope) => (
                        <Chip key={scope} tone={scope === 'admin' ? 'manual' : 'neutral'}>
                          {scope}
                        </Chip>
                      ))}
                    </span>
                  </Td>

                  <Td column="prefix">
                    <span className="font-mono text-xs text-bone-faint">{client.keyPrefix}…</span>
                  </Td>

                  <Td column="lastUsed" className="whitespace-nowrap">
                    <span
                      className="font-mono text-xs text-bone-faint tabular-nums"
                      title={fmt.dateTime(client.lastUsedAt, locale)}
                    >
                      {fmt.relative(client.lastUsedAt, locale) ?? t.admin.keys.neverUsed}
                    </span>
                  </Td>

                  <Td column="actions">
                    <span className="flex items-center justify-end gap-0.5">
                      <IconButton
                        glyph="settings"
                        label={t.settings.openFor(client.name)}
                        onClick={() => setTuning(client)}
                      />
                      <IconButton
                        glyph="power"
                        label={client.isEnabled ? t.admin.works.disable : t.admin.works.enable}
                        busy={setEnabled.isPending && setEnabled.variables.id === client.id}
                        onClick={() =>
                          client.isEnabled
                            ? setAsking({ client, what: 'disable' })
                            : setEnabled.mutate({ id: client.id, enabled: true })
                        }
                      />
                      <IconButton
                        glyph="trash"
                        tone="danger"
                        label={t.admin.keys.revoke}
                        onClick={() => setAsking({ client, what: 'revoke' })}
                      />
                    </span>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </DataTable>
        )}
      </Panel>

      {/* Plex is let in by its address too, so the panel that tells how to
          give Plex this server sits right above the list it leads to. */}
      <PlexPanel />

      {/* An agent is let in by a key alone, made for it: beside Plex, the
          other client given an address to call. */}
      <McpPanel />

      {/* The address list belongs on this page, not a page of its own: a key
          and an allowed address are the same decision said two ways, and
          Sonarr, Radarr and Plex can only make it the second way. */}
      <div className="mt-6">
        <NetworkAccess />
      </div>

      <Dialog
        open={asking?.what === 'revoke'}
        title={t.admin.keys.revokeTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={revoke.isPending}
              onClick={() => asking && revoke.mutate(asking.client.id)}
            >
              <Glyph name="trash" className="size-4" />
              {t.admin.keys.revoke}
            </Button>
          </>
        }
      >
        {asking ? t.admin.keys.revokeBody(asking.client.name) : null}
      </Dialog>

      <Dialog
        open={asking?.what === 'disable'}
        title={t.admin.keys.disableTitle}
        onClose={() => setAsking(null)}
        footer={
          <>
            <Button onClick={() => setAsking(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={setEnabled.isPending}
              onClick={() =>
                asking && setEnabled.mutate({ id: asking.client.id, enabled: false })
              }
            >
              {t.admin.works.disable}
            </Button>
          </>
        }
      >
        {asking ? t.admin.keys.disableBody(asking.client.name) : null}
      </Dialog>

      {/* What this one client is answered differently. The key is the handle:
          a client that presents one is identified on every request, so the
          server can look its settings up before it answers. */}
      <ScopeSettingsDialog
        at={tuning ? { scope: 'client', id: tuning.id } : null}
        who={tuning?.name ?? ''}
        onClose={() => setTuning(null)}
      />
    </div>
  )
}

/**
 * Plex's provider: the address to paste into Plex, the switch that answers
 * it, and what letting Plex in takes.
 *
 * The address is the public one when the deployment names it — what a Plex
 * elsewhere on the network reaches — else the one this page was opened at.
 * Plex presents no credential to a provider, so the decision that lets it in
 * is the list of allowed addresses just below; the panel says so and leads
 * there, to the field an address is typed in.
 *
 * Unless the provider asks for a key (`AMS_PLEX_AUTH=apikey`): then Plex is
 * given one in its address, `/plex/<key>`, and the panel makes a key for it —
 * read only, named Plex — and shows the address it goes in, once, as a key
 * is only ever shown once.
 */
function PlexPanel() {
  const { t } = useI18n()
  const p = t.admin.plex
  const queryClient = useQueryClient()
  const field = useRef<HTMLInputElement>(null)
  // What the copy button last did: copied, or left the address selected for
  // the reader to copy where the clipboard is out of reach — a plain-HTTP
  // page on a LAN address, which is where this panel is usually read.
  const [copied, setCopied] = useState<'done' | 'manual' | null>(null)
  // The key just made for Plex, for as long as this page is open.
  const [made, setMade] = useState<string | null>(null)

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/settings'),
    staleTime: 5 * 60_000,
  })
  const access = useQuery({
    queryKey: ['admin', 'access'],
    queryFn: () => api.get<AccessReport>('/admin/access'),
  })

  const write = useMutation({
    mutationFn: (on: boolean) => api.put('/settings/server/-', { key: 'api.plex', value: String(on) }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['admin', 'access'] })
      void queryClient.invalidateQueries({ queryKey: ['settings'] })
    },
  })

  const makeKey = useMutation({
    mutationFn: () => api.post<ApiClient & { key: string }>('/clients', { name: 'Plex', scopes: ['read'] }),
    onSuccess: (result) => {
      setMade(result.key)
      void queryClient.invalidateQueries({ queryKey: ['clients'] })
      void queryClient.invalidateQueries({ queryKey: ['stats'] })
    },
  })

  useEffect(() => {
    if (copied !== 'done') return
    const timer = setTimeout(() => setCopied(null), 2_000)
    return () => clearTimeout(timer)
  }, [copied])

  const base = (settings.data?.publicUrl ?? window.location.origin).replace(/\/+$/, '')
  const plex = access.data?.apis.find((x) => x.api === 'plex')
  const open = plex?.policy === 'open'
  const keyed = plex?.policy === 'apikey'
  const address = keyed ? `${base}/plex/${made ?? '<key>'}` : `${base}/plex`
  // Nothing to copy yet: the address waits for its key.
  const waiting = keyed && !made

  const copy = async () => {
    try {
      if (!navigator.clipboard) throw new Error('no clipboard')
      await navigator.clipboard.writeText(address)
      setCopied('done')
    } catch {
      field.current?.focus()
      field.current?.select()
      setCopied('manual')
    }
  }

  // To the field of the list below that an address is typed in, without the
  // glide for whoever asked for less motion.
  const toList = (event: ReactMouseEvent<HTMLAnchorElement>) => {
    const target = document.getElementById('network-cidr')
    if (!target) return
    event.preventDefault()
    const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    target.scrollIntoView({ behavior: still ? 'auto' : 'smooth', block: 'center' })
    target.focus({ preventScroll: true })
  }

  return (
    <Panel label={p.title} className="rise mt-6" style={{ animationDelay: '120ms' }}>
      <PanelHead
        title={p.title}
        action={plex ? <Lamp tone={plex.enabled ? 'moss' : 'faint'}>{plex.enabled ? p.on : p.off}</Lamp> : undefined}
      />
      <div className="space-y-4 p-5">
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{p.lead}</p>

        <div>
          <label htmlFor="plex-address" className="label mb-1.5 block">
            {p.address}
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <input
              id="plex-address"
              ref={field}
              readOnly
              value={address}
              aria-describedby={keyed ? 'plex-key-note' : undefined}
              onFocus={(event) => event.currentTarget.select()}
              className="min-h-11 min-w-0 flex-1 rounded-card border border-rule bg-ink px-3 font-mono text-sm text-bone"
            />
            {waiting ? (
              <Button variant="primary" onClick={() => makeKey.mutate()} disabled={makeKey.isPending}>
                {makeKey.isPending ? <Spinner className="size-4" /> : <Glyph name="key" className="size-4" />}
                {p.makeKey}
              </Button>
            ) : (
              <Button onClick={() => void copy()}>
                <Glyph name={copied === 'done' ? 'check' : 'copy'} className="size-4" />
                {copied === 'done' ? p.copied : p.copy}
              </Button>
            )}
          </div>
          {keyed ? (
            <p id="plex-key-note" className="mt-2 max-w-prose text-xs leading-relaxed text-bone-dim">
              {made ? p.keyOnce : p.keyed}
            </p>
          ) : null}
          {makeKey.isError ? (
            <p role="alert" className="mt-2 flex items-center gap-2 text-xs text-status-error">
              <Glyph name="alert" className="size-3.5" />
              {makeKey.error.message}
            </p>
          ) : null}
          {copied === 'manual' ? (
            <p role="status" className="mt-2 text-xs text-bone-dim">
              {p.copyManual}
            </p>
          ) : null}
        </div>

        <div className="flex items-center justify-between gap-4 rounded-card border border-rule bg-ink px-4 py-1.5">
          <div className="min-w-0">
            <span className="block text-sm text-bone">{p.toggle}</span>
            {plex ? (
              <span className="block text-xs leading-relaxed text-bone-faint">
                {plex.enabled ? p.calls(plex.served, plex.refused) : p.offHint}
              </span>
            ) : null}
          </div>
          <Toggle
            id="plex-switch"
            checked={plex?.enabled ?? false}
            label={p.toggle}
            onChange={(next) => write.mutate(next)}
            disabled={!plex || write.isPending}
          />
        </div>
        {write.isError ? (
          <p role="alert" className="flex items-center gap-2 text-xs text-status-error">
            <Glyph name="alert" className="size-3.5" />
            {write.error.message}
          </p>
        ) : null}

        <p className="flex items-start gap-2 text-sm leading-relaxed text-bone">
          <Glyph name="shield" className="mt-0.5 size-4 shrink-0 text-brass" />
          <span>
            {open ? p.openPolicy : keyed ? p.keyCredential : p.credential}{' '}
            {open || keyed ? null : (
              <a
                href="#network-cidr"
                onClick={toList}
                className="text-vermillion underline-offset-4 hover:text-vermillion-bright hover:underline"
              >
                {p.toList}
              </a>
            )}
          </span>
        </p>
        <p className="max-w-prose text-xs leading-relaxed text-bone-faint">{keyed ? p.keyTravels : p.localOnly}</p>
      </div>
    </Panel>
  )
}

/**
 * The MCP server, for agents: the address to give one, the switch that
 * answers it, a key made for it, and the line that connects Claude Code.
 *
 * The address is the public one when the deployment names it, else the one
 * this page was opened at: an agent is refused under a name the server was
 * not told it has, and this one it was. A key made here carries the `mcp`
 * scope alone, and is shown once, in the command it goes in.
 */
function McpPanel() {
  const { t } = useI18n()
  const m = t.admin.mcp
  const queryClient = useQueryClient()
  // The key just made for an agent, for as long as this page is open.
  const [made, setMade] = useState<string | null>(null)
  // Which of the two lines was last copied: done, or left selected for the
  // reader to copy where the clipboard is out of reach.
  const [copied, setCopied] = useState<{ what: 'address' | 'command'; how: 'done' | 'manual' } | null>(null)
  const addressField = useRef<HTMLInputElement>(null)
  const commandField = useRef<HTMLTextAreaElement>(null)

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/settings'),
    staleTime: 5 * 60_000,
  })
  const values = useServerValues()
  const clients = useQuery({ queryKey: ['clients'], queryFn: () => api.get<ApiClient[]>('/clients') })

  const write = useMutation({
    mutationFn: (on: boolean) => api.put('/settings/server/-', { key: MCP_SETTING, value: String(on) }),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['settings'] }),
  })

  const makeKey = useMutation({
    mutationFn: () =>
      api.post<ApiClient & { key: string }>('/clients', {
        name: freeAgentName(
          m.keyName,
          (clients.data ?? []).filter((client) => !client.ownerId).map((client) => client.name),
        ),
        scopes: ['mcp'],
      }),
    onSuccess: (result) => {
      setMade(result.key)
      void queryClient.invalidateQueries({ queryKey: ['clients'] })
      void queryClient.invalidateQueries({ queryKey: ['stats'] })
    },
  })

  useEffect(() => {
    if (copied?.how !== 'done') return
    const timer = setTimeout(() => setCopied(null), 2_000)
    return () => clearTimeout(timer)
  }, [copied])

  const state = values.data?.find((value) => value.key === MCP_SETTING)
  const on = state?.value === 'true'
  const address = mcpAddress(settings.data?.publicUrl ?? window.location.origin)
  const command = claudeCommand(address, made)

  const copy = async (what: 'address' | 'command') => {
    try {
      if (!navigator.clipboard) throw new Error('no clipboard')
      await navigator.clipboard.writeText(what === 'address' ? address : command)
      setCopied({ what, how: 'done' })
    } catch {
      const field = what === 'address' ? addressField.current : commandField.current
      field?.focus()
      field?.select()
      setCopied({ what, how: 'manual' })
    }
  }

  return (
    <Panel label={m.title} className="rise mt-6" style={{ animationDelay: '140ms' }}>
      <PanelHead
        title={m.title}
        action={state ? <Lamp tone={on ? 'moss' : 'faint'}>{on ? m.on : m.off}</Lamp> : undefined}
      />
      <div className="space-y-4 p-5">
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{m.lead}</p>

        <div className="flex items-center justify-between gap-4 rounded-card border border-rule bg-ink px-4 py-1.5">
          <div className="min-w-0">
            <span className="block text-sm text-bone">{m.toggle}</span>
            <span className="block text-xs leading-relaxed text-bone-faint">{on ? m.onHint : m.offHint}</span>
          </div>
          <Toggle
            id="mcp-switch"
            checked={on}
            label={m.toggle}
            onChange={(next) => write.mutate(next)}
            disabled={!state || write.isPending}
          />
        </div>
        {write.isError ? (
          <p role="alert" className="flex items-center gap-2 text-xs text-status-error">
            <Glyph name="alert" className="size-3.5" />
            {write.error.message}
          </p>
        ) : null}

        <div>
          <label htmlFor="mcp-address" className="label mb-1.5 block">
            {m.address}
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <input
              id="mcp-address"
              ref={addressField}
              readOnly
              value={address}
              onFocus={(event) => event.currentTarget.select()}
              className="min-h-11 min-w-0 flex-1 rounded-card border border-rule bg-ink px-3 font-mono text-sm text-bone"
            />
            <Button onClick={() => void copy('address')}>
              <Glyph name={copied?.what === 'address' && copied.how === 'done' ? 'check' : 'copy'} className="size-4" />
              {copied?.what === 'address' && copied.how === 'done' ? m.copied : m.copy}
            </Button>
          </div>
        </div>

        <div>
          <label htmlFor="mcp-command" className="label mb-1.5 block">
            {m.claude}
          </label>
          <textarea
            id="mcp-command"
            ref={commandField}
            readOnly
            rows={3}
            value={command}
            aria-describedby="mcp-key-note"
            onFocus={(event) => event.currentTarget.select()}
            className="block min-h-20 w-full resize-none rounded-card border border-rule bg-ink px-3 py-2 font-mono text-sm leading-relaxed text-bone field-sizing-content"
          />
          <div className="mt-2 flex flex-wrap items-center gap-2">
            {made ? null : (
              <Button variant="primary" onClick={() => makeKey.mutate()} disabled={makeKey.isPending}>
                {makeKey.isPending ? <Spinner className="size-4" /> : <Glyph name="key" className="size-4" />}
                {m.makeKey}
              </Button>
            )}
            <Button onClick={() => void copy('command')}>
              <Glyph name={copied?.what === 'command' && copied.how === 'done' ? 'check' : 'copy'} className="size-4" />
              {copied?.what === 'command' && copied.how === 'done' ? m.copied : m.copyCommand}
            </Button>
          </div>
          <p id="mcp-key-note" className="mt-2 max-w-prose text-xs leading-relaxed text-bone-dim">
            {made ? m.keyOnce : m.keyHint}
          </p>
          {makeKey.isError ? (
            <p role="alert" className="mt-2 flex items-center gap-2 text-xs text-status-error">
              <Glyph name="alert" className="size-3.5" />
              {makeKey.error.message}
            </p>
          ) : null}
          {copied?.how === 'manual' ? (
            <p role="status" className="mt-2 text-xs text-bone-dim">
              {m.copyManual}
            </p>
          ) : null}
        </div>

        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{m.openWebUi}</p>

        <p className="flex items-start gap-2 text-sm leading-relaxed text-bone">
          <Glyph name="shield" className="mt-0.5 size-4 shrink-0 text-brass" />
          <span>{m.reads}</span>
        </p>
        <p className="max-w-prose text-xs leading-relaxed text-bone-faint">{m.names}</p>
      </div>
    </Panel>
  )
}

/**
 * The one moment this key exists anywhere a person can read it.
 *
 * Only a hash of it reaches the database, so there is no second chance and no
 * "show again". Hence the accent frame, the key at a size you can read across a
 * desk, and a dismissal that states what it is confirming.
 */
function IssuedKey({
  issued,
  onDismiss,
}: {
  issued: { name: string; key: string }
  onDismiss: () => void
}) {
  const { t } = useI18n()
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(issued.key)
      setCopied('yes')
    } catch {
      // A browser that refuses the clipboard is not a reason to lose the key.
      setCopied('failed')
    }
  }

  return (
    <Panel className="rise mb-6 border-vermillion-deep bg-vermillion/[0.06]">
      <div className="p-5">
        <span className="label flex items-center gap-2 text-vermillion">
          <Glyph name="key" className="size-3.5" />
          {t.admin.keys.keyFor(issued.name)}
        </span>

        <p className="mt-2 mb-4 max-w-prose text-sm leading-relaxed text-bone">
          {t.admin.keys.onlyOnce}
        </p>

        <label htmlFor="issued-key" className="label mb-1.5 block">
          {t.admin.keys.theKey}
        </label>
        <div className="flex flex-wrap items-center gap-2">
          <input
            id="issued-key"
            readOnly
            value={issued.key}
            onFocus={(event) => event.currentTarget.select()}
            className={cn(
              'min-h-11 min-w-0 flex-1 rounded-card border border-vermillion-deep bg-ink px-3',
              'font-mono text-sm text-bone selection:bg-vermillion selection:text-ink',
            )}
          />
          <Button variant="primary" onClick={() => void copy()}>
            <Glyph name={copied === 'yes' ? 'check' : 'copy'} className="size-4" />
            {copied === 'yes' ? t.admin.keys.copied : t.admin.keys.copy}
          </Button>
          <Button onClick={onDismiss}>{t.admin.keys.dismiss}</Button>
        </div>

        {copied === 'failed' ? (
          <p role="alert" className="mt-2 text-xs text-status-error">
            {t.admin.keys.copyFailed}
          </p>
        ) : null}
      </div>
    </Panel>
  )
}
