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
import { useState } from 'react'

import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  IconButton,
  Input,
  Label,
  Panel,
  PanelHead,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Th,
  Tr,
} from '../../components/ui'
import { NetworkAccess } from '../../components/NetworkAccess'
import { ScopeSettingsDialog } from '../../components/ScopeSettings'
import { api } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { ApiClient } from '../../lib/types'

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
  ] as const

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

      <Panel className="rise overflow-hidden" style={{ animationDelay: '100ms' }}>
        <PanelHead title={t.admin.keys.issued} />

        {clients.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 3 }, (_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : clients.isError ? (
          <p role="alert" className="flex items-center gap-2 px-4 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {t.admin.keys.loadFailed}
          </p>
        ) : clients.data.length === 0 ? (
          <EmptyState title={t.admin.keys.empty} hint={t.admin.keys.emptyHint} />
        ) : (
          <TableScroll label={t.admin.keys.issued}>
            <table className="w-full min-w-[20rem] border-collapse text-left">
              <thead>
                <tr>
                  <Th>{t.admin.keys.colName}</Th>
                  <Th className="hidden sm:table-cell">{t.admin.keys.colScopes}</Th>
                  <Th className="hidden lg:table-cell">{t.admin.keys.colPrefix}</Th>
                  <Th align="right" className="hidden md:table-cell">
                    {t.admin.keys.colLastUsed}
                  </Th>
                  <Th align="right">{t.admin.keys.colActions}</Th>
                </tr>
              </thead>
              <tbody>
                {clients.data.map((client) => (
                  <Tr key={client.id} className={client.isEnabled ? '' : 'opacity-60'}>
                    <Td className="w-full max-w-0">
                      <span className="flex items-center gap-2">
                        <span className="truncate text-sm font-medium text-bone">
                          {client.name}
                        </span>
                        {client.isEnabled ? null : (
                          <Chip tone="accent">{t.admin.works.disabled}</Chip>
                        )}
                      </span>
                      <span className="mt-0.5 flex gap-2 font-mono text-[0.6875rem] text-bone-faint sm:hidden">
                        {client.scopes.join(' · ')}
                      </span>
                    </Td>

                    <Td className="hidden sm:table-cell">
                      <span className="flex flex-wrap gap-1.5">
                        {client.scopes.map((scope) => (
                          <Chip key={scope} tone={scope === 'admin' ? 'manual' : 'neutral'}>
                            {scope}
                          </Chip>
                        ))}
                      </span>
                    </Td>

                    <Td className="hidden lg:table-cell">
                      <span className="font-mono text-xs text-bone-faint">{client.keyPrefix}…</span>
                    </Td>

                    <Td align="right" className="hidden whitespace-nowrap md:table-cell">
                      <span
                        className="font-mono text-xs text-bone-faint tabular-nums"
                        title={fmt.dateTime(client.lastUsedAt, locale)}
                      >
                        {fmt.relative(client.lastUsedAt, locale) ?? t.admin.keys.neverUsed}
                      </span>
                    </Td>

                    <Td align="right">
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
              </tbody>
            </table>
          </TableScroll>
        )}
      </Panel>

      {/* The address list belongs on this page, not a page of its own: a key
          and an allowed address are the same decision said two ways, and
          Sonarr and Radarr can only make it the second way. */}
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
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {t.admin.keys.copyFailed}
          </p>
        ) : null}
      </div>
    </Panel>
  )
}
