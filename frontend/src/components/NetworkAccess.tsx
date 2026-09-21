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
  Input,
  Label,
  Panel,
  PanelHead,
  Td,
  Th,
  Tr,
  TableScroll,
} from './ui'

export function NetworkAccess() {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const [cidr, setCidr] = useState('')
  const [note, setNote] = useState('')
  const [removing, setRemoving] = useState<NetworkRule | null>(null)

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

  const allowedAlready = (ip: string) =>
    (rules.data ?? []).some((rule) => rule.cidr === ip || rule.cidr.startsWith(`${ip}/`))

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

        {rules.data?.length === 0 ? (
          <div className="border-t border-rule">
            <EmptyState title={t.admin.network.noRules} hint={t.admin.network.noRulesHint} />
          </div>
        ) : (
          <ul className="border-t border-rule">
            {(rules.data ?? []).map((rule) => (
              <li
                key={rule.id}
                className="flex items-center justify-between gap-4 border-b border-rule px-5 py-3 last:border-0"
              >
                <div className="min-w-0">
                  <p className="font-mono text-sm text-bone tabular-nums">{rule.cidr}</p>
                  <p className="mt-0.5 truncate text-xs text-bone-faint">
                    {rule.note === 'from AMS_ALLOWLIST' ? t.admin.network.seeded : rule.note}
                  </p>
                </div>

                <Button size="sm" variant="danger" onClick={() => setRemoving(rule)}>
                  {t.admin.network.remove}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel label={t.admin.network.callers} className="rise" style={{ animationDelay: '180ms' }}>
        <PanelHead title={t.admin.network.callers} />

        <p className="max-w-prose px-5 pt-4 pb-1 text-sm leading-relaxed text-bone-dim">
          {t.admin.network.callersLead}
        </p>

        {callers.data?.length === 0 ? (
          <EmptyState title={t.admin.network.noCallers} hint={t.admin.network.noCallersHint} />
        ) : (
          <TableScroll>
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
                    allowed={allowedAlready(caller.ip)}
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
        <span className="mt-0.5 block font-mono text-[0.625rem] tracking-wide text-bone-faint uppercase">
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
