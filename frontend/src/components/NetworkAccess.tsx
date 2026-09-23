/**
 * Who may call the routes that cannot carry a key, and who has tried.
 *
 * It sits beside the API keys because it is the same decision said a different
 * way: Sonarr and Radarr have their metadata URLs compiled in and can present
 * nothing, so an address is the only credential they have.
 *
 * The two panels are meant to be read together. The callers table is where a
 * refused container appears with its name and its address, and the button
 * beside it is the fix — which is the whole reason for keeping refusals.
 *
 * A rule can also be named, and that is more than a label: settings hang off
 * the name, because the address a container calls from changes every time it
 * restarts and the name does not. An unnamed rule therefore says it is unnamed
 * and asks to be, rather than leaving a blank where a name would go.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import * as fmt from '../lib/format'
import { api } from '../lib/api'
import { useI18n } from '../lib/i18n'
import type { NetworkCaller, NetworkRule } from '../lib/types'
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
  Td,
  Th,
  Tr,
  TableScroll,
} from './ui'
import { ScopeSettingsDialog, type Scope } from './ScopeSettings'

export function NetworkAccess() {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const [cidr, setCidr] = useState('')
  const [note, setNote] = useState('')
  const [removing, setRemoving] = useState<NetworkRule | null>(null)
  const [naming, setNaming] = useState<NetworkRule | null>(null)
  const [draft, setDraft] = useState('')
  const [tuning, setTuning] = useState<{ at: Scope; who: string } | null>(null)

  const rules = useQuery({
    queryKey: ['network', 'rules'],
    queryFn: () => api.get<NetworkRule[]>('/network/rules'),
  })

  const callers = useQuery({
    queryKey: ['network', 'callers'],
    queryFn: () => api.get<NetworkCaller[]>('/network/callers'),
    // A refusal appears the moment somebody tries; this is the one table in the
    // interface worth watching while you fix the thing it is telling you about.
    refetchInterval: 15_000,
  })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['network'] })
  }

  const allow = useMutation({
    mutationFn: (rule: { cidr: string; note?: string }) => api.post('/network/rules', rule),
    onSuccess: () => {
      setCidr('')
      setNote('')
      invalidate()
    },
  })

  const remove = useMutation({
    mutationFn: (id: string) => api.delete(`/network/rules/${id}`),
    onSuccess: () => {
      setRemoving(null)
      invalidate()
    },
  })

  const rename = useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) =>
      api.patch<NetworkRule>(`/network/rules/${id}`, { name: name.trim() || null }),
    onSuccess: () => {
      setNaming(null)
      invalidate()
    },
  })


  return (
    <>
      <Panel label={t.admin.network.rules} className="rise mb-6" style={{ animationDelay: '140ms' }}>
        <PanelHead title={t.admin.network.rules} />

        <p className="max-w-prose px-5 pt-4 text-sm leading-relaxed text-bone-dim">
          {t.admin.network.lead}
        </p>

        <form
          className="flex flex-wrap items-start gap-4 p-5"
          onSubmit={(event) => {
            event.preventDefault()
            allow.mutate({ cidr, note: note || undefined })
          }}
        >
          <div className="min-w-52 flex-1">
            <FormField
              label={t.admin.network.addressOrBlock}
              htmlFor="network-cidr"
              hint={t.admin.network.addressHint}
              error={allow.isError ? allow.error.message : undefined}
            >
              <Input
                id="network-cidr"
                required
                inputMode="numeric"
                placeholder="172.31.0.0/24"
                value={cidr}
                onChange={(event) => setCidr(event.target.value)}
              />
            </FormField>
          </div>

          <div className="min-w-48 flex-1">
            <FormField
              label={t.admin.network.note}
              htmlFor="network-note"
              hint={t.admin.network.noteHint}
            >
              <Input
                id="network-note"
                value={note}
                onChange={(event) => setNote(event.target.value)}
              />
            </FormField>
          </div>

          <Button
            type="submit"
            variant="primary"
            className="mt-6"
            disabled={allow.isPending || !cidr.trim()}
          >
            <Glyph name="check" className="size-4" />
            {t.admin.network.allow}
          </Button>
        </form>

        {rules.isPending ? (
          <div className="space-y-2 border-t border-rule p-4">
            {Array.from({ length: 2 }, (_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : rules.isError ? (
          // Not the empty state: "there are no rules" and "we could not ask"
          // read the same on screen and mean opposite things, and an operator
          // who believes the first starts adding the allowlist back by hand.
          <p
            role="alert"
            className="flex items-center gap-2 border-t border-rule px-5 py-6 text-sm text-vermillion"
          >
            <Glyph name="alert" className="size-4" />
            {t.admin.network.loadFailed}
          </p>
        ) : rules.data.length === 0 ? (
          <div className="border-t border-rule">
            <EmptyState title={t.admin.network.noRules} hint={t.admin.network.noRulesHint} />
          </div>
        ) : (
          <ul className="border-t border-rule">
            {(rules.data ?? []).map((rule) => {
              const editing = naming?.id === rule.id

              return (
                <li key={rule.id} className="border-b border-rule px-5 py-3 last:border-0">
                  <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
                    <div className="min-w-40 flex-1">
                      <p className="flex flex-wrap items-center gap-x-3 gap-y-1">
                        <span className="font-mono text-sm text-bone tabular-nums">{rule.cidr}</span>
                        {editing ? null : rule.name ? (
                          <span className="text-sm text-bone-dim">{rule.name}</span>
                        ) : (
                          <Chip>{t.admin.network.noName}</Chip>
                        )}
                      </p>
                      <p className="mt-0.5 truncate text-xs text-bone-faint">
                        {rule.note === 'from AMS_ALLOWLIST' ? t.admin.network.seeded : rule.note}
                      </p>
                    </div>

                    {editing ? null : (
                      <div className="flex flex-wrap items-center gap-1.5">
                        {rule.name ? (
                          <>
                            <Button
                              size="sm"
                              onClick={() =>
                                setTuning({
                                  at: { scope: 'peer', id: rule.id },
                                  who: rule.name ?? rule.cidr,
                                })
                              }
                            >
                              <Glyph name="settings" className="size-3.5" />
                              {t.settings.open}
                            </Button>
                            <IconButton
                              glyph="pencil"
                              label={t.admin.network.rename}
                              onClick={() => {
                                rename.reset()
                                setDraft(rule.name ?? '')
                                setNaming(rule)
                              }}
                            />
                          </>
                        ) : (
                          <Button
                            size="sm"
                            onClick={() => {
                              rename.reset()
                              setDraft('')
                              setNaming(rule)
                            }}
                          >
                            <Glyph name="pencil" className="size-3.5" />
                            {t.admin.network.nameThis}
                          </Button>
                        )}

                        <Button size="sm" variant="danger" onClick={() => setRemoving(rule)}>
                          {t.admin.network.remove}
                        </Button>
                      </div>
                    )}
                  </div>

                  {/* The form sits under the address rather than replacing it:
                      nobody can name the client behind 172.31.0.7 while the row
                      has stopped saying which address they are naming. */}
                  {editing ? (
                    <form
                      className="mt-3 flex flex-wrap items-start gap-3"
                      onSubmit={(event) => {
                        event.preventDefault()
                        rename.mutate({ id: rule.id, name: draft })
                      }}
                    >
                      <div className="min-w-48 flex-1">
                        <FormField
                          label={t.admin.network.name}
                          htmlFor={`rule-name-${rule.id}`}
                          hint={t.admin.network.nameHint}
                          error={rename.isError ? rename.error.message : undefined}
                        >
                          <Input
                            id={`rule-name-${rule.id}`}
                            autoFocus
                            value={draft}
                            onChange={(event) => setDraft(event.target.value)}
                          />
                        </FormField>
                      </div>
                      <Button
                        type="submit"
                        variant="primary"
                        className="mt-6"
                        disabled={rename.isPending}
                      >
                        {t.common.save}
                      </Button>
                      <Button type="button" className="mt-6" onClick={() => setNaming(null)}>
                        {t.common.cancel}
                      </Button>
                    </form>
                  ) : null}
                </li>
              )
            })}
          </ul>
        )}
      </Panel>

      <Panel label={t.admin.network.callers} className="rise" style={{ animationDelay: '180ms' }}>
        <PanelHead title={t.admin.network.callers} />

        <p className="max-w-prose px-5 pt-4 pb-1 text-sm leading-relaxed text-bone-dim">
          {t.admin.network.callersLead}
        </p>

        {callers.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 3 }, (_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : callers.isError ? (
          <p role="alert" className="flex items-center gap-2 px-5 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {t.admin.network.callersFailed}
          </p>
        ) : callers.data.length === 0 ? (
          <EmptyState title={t.admin.network.noCallers} hint={t.admin.network.noCallersHint} />
        ) : (
          <TableScroll label={t.admin.network.callers}>
            <table className="w-full min-w-[34rem] border-collapse text-left">
              <thead>
                <tr>
                  <Th>{t.admin.network.colAddress}</Th>
                  <Th>{t.admin.network.colClient}</Th>
                  <Th>{t.admin.network.colActivity}</Th>
                  <Th align="right">{t.admin.network.colSeen}</Th>
                  <Th align="right" />
                </tr>
              </thead>
              <tbody>
                {(callers.data ?? []).map((caller) => (
                  <CallerRow
                    key={caller.ip}
                    caller={caller}
                    // The server's answer, not a string comparison here: it
                    // holds the rules and already does the address arithmetic
                    // that decides whether 172.31.0.7 is inside 172.31.0.0/24.
                    allowed={caller.covered}
                    onAllow={() =>
                      allow.mutate({
                        cidr: caller.ip,
                        note: caller.hostname ?? caller.userAgent ?? undefined,
                      })
                    }
                  />
                ))}
              </tbody>
            </table>
          </TableScroll>
        )}
      </Panel>

      <Dialog
        open={Boolean(removing)}
        title={t.admin.network.removeTitle}
        onClose={() => setRemoving(null)}
        footer={
          <>
            <Button onClick={() => setRemoving(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={remove.isPending}
              onClick={() => removing && remove.mutate(removing.id)}
            >
              <Glyph name="trash" className="size-4" />
              {t.admin.network.remove}
            </Button>
          </>
        }
      >
        {removing ? t.admin.network.removeBody(removing.cidr) : null}
      </Dialog>

      <ScopeSettingsDialog
        at={tuning?.at ?? null}
        who={tuning?.who ?? ''}
        onClose={() => setTuning(null)}
      />
    </>
  )
}

function CallerRow({
  caller,
  allowed,
  onAllow,
}: {
  caller: NetworkCaller
  allowed: boolean
  onAllow: () => void
}) {
  const { t, locale } = useI18n()

  return (
    <Tr>
      <Td>
        <span className="font-mono text-sm text-bone tabular-nums">{caller.ip}</span>
        <span className="mt-0.5 block text-xs text-bone-faint">
          {caller.hostname ?? t.admin.network.unnamed}
        </span>
      </Td>

      <Td>
        {/* The user agent names the application where the hostname names the
            machine; between them an operator can tell two Sonarrs apart. */}
        <span className="text-sm text-bone-dim">{caller.userAgent ?? '—'}</span>
        <span className="mt-0.5 block font-mono text-[0.6875rem] tracking-wide text-bone-faint uppercase">
          {caller.lastSurface}
        </span>
      </Td>

      <Td>
        <div className="flex flex-wrap items-center gap-2">
          <Chip tone={caller.lastAllowed ? 'provider' : 'accent'}>
            <Glyph name={caller.lastAllowed ? 'check' : 'alert'} className="size-3" />
            {caller.lastAllowed ? t.admin.network.allowed : t.admin.network.refused}
          </Chip>
          <span className="text-xs text-bone-faint">
            {t.admin.network.callCount(caller.hits)}
            {caller.refusals > 0 ? ` · ${t.admin.network.refusedCount(caller.refusals)}` : ''}
          </span>
        </div>
      </Td>

      <Td align="right">
        <span className="text-xs text-bone-faint">{fmt.relative(caller.lastSeen, locale)}</span>
      </Td>

      <Td align="right">
        {allowed ? (
          <Label>{t.admin.network.alreadyAllowed}</Label>
        ) : (
          <Button size="sm" onClick={onAllow} aria-label={t.admin.network.allowThis(caller.ip)}>
            {t.admin.network.allow}
          </Button>
        )}
      </Td>
    </Tr>
  )
}
