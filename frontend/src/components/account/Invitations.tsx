/**
 * The invitations: the ones handed out, and a form to hand out another.
 *
 * A code is a secret like a key — the server keeps only its hash — so the
 * link that carries it is shown once, when it is made, and never again. The
 * list shows each by the first group of its code, which is enough to tell
 * them apart and nowhere near enough to use one.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { Link, useLocation } from 'react-router'

import { api } from '../../lib/api'
import { relative, shortDate } from '../../lib/format'
import { useAuthOptions } from '../../lib/hooks'
import { useI18n } from '../../lib/i18n'
import type { AccountRole, Invitation, Invitations as InvitationsPage, IssuedInvitation } from '../../lib/types'
import {
  Button,
  Dialog,
  FormField,
  Glyph,
  IconButton,
  Input,
  Lamp,
  Panel,
  PanelHead,
  Segmented,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Th,
  Tr,
} from '../ui'
import { RoleChip } from './people'
import { SecretReveal } from './SecretReveal'

type State = 'usable' | 'used' | 'expired' | 'revoked' | 'void'

const DAYS = ['1', '7', '30', '90'] as const

function stateOf(invitation: Invitation, now: number): State {
  if (invitation.revokedAt) return 'revoked'
  if (!invitation.creatorActive) return 'void'
  if (invitation.uses >= invitation.maxUses) return 'used'
  if (invitation.expiresAt && new Date(invitation.expiresAt).getTime() <= now) return 'expired'
  return 'usable'
}

const TONE: Record<State, 'moss' | 'faint' | 'brass' | 'vermillion'> = {
  usable: 'moss',
  used: 'faint',
  expired: 'faint',
  revoked: 'faint',
  void: 'brass',
}

export function Invitations() {
  const { t, locale } = useI18n()
  const queryClient = useQueryClient()
  const options = useAuthOptions()
  const inv = t.admin.invitations

  const [role, setRole] = useState<AccountRole>('member')
  const [uses, setUses] = useState('1')
  const [days, setDays] = useState<(typeof DAYS)[number]>('7')
  const [note, setNote] = useState('')
  const [issued, setIssued] = useState<IssuedInvitation | null>(null)
  const [withdrawing, setWithdrawing] = useState<Invitation | null>(null)

  // Arrived from the access page's link: this panel is what it pointed at.
  const location = useLocation()
  const here = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (location.hash === '#invitations') here.current?.scrollIntoView({ block: 'start' })
  }, [location.hash])

  const list = useQuery({
    queryKey: ['invitations'],
    queryFn: () => api.get<InvitationsPage>('/invitations'),
  })

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['invitations'] })
    void queryClient.invalidateQueries({ queryKey: ['admin', 'access'] })
  }

  const create = useMutation({
    mutationFn: () =>
      api.post<IssuedInvitation>('/invitations', {
        role,
        maxUses: Number(uses),
        days: Number(days),
        note: note.trim() || undefined,
      }),
    onSuccess: (made) => {
      setIssued(made)
      setNote('')
      refresh()
    },
  })

  const revoke = useMutation({
    mutationFn: (id: string) => api.delete(`/invitations/${id}`),
    onSuccess: () => {
      setWithdrawing(null)
      refresh()
    },
  })

  const count = Number(uses)
  const usesValid = Number.isInteger(count) && count >= 1 && count <= 100
  const now = Date.now()
  const link = issued ? (issued.link ?? `${window.location.origin}/register#invite=${issued.code}`) : ''

  return (
    <Panel id="invitations" label={inv.title} className="rise mt-8" style={{ animationDelay: '160ms' }}>
      <div ref={here} className="scroll-mt-28 lg:scroll-mt-16" />
      <PanelHead
        title={inv.title}
        action={
          list.data ? (
            <span className="font-mono text-xs text-bone-faint tabular-nums">
              {inv.usable(list.data.usable, list.data.places)}
            </span>
          ) : undefined
        }
      />

      {options.data?.registration === 'closed' ? (
        <p className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-rule bg-brass/[0.06] px-5 py-3 text-sm text-bone-dim">
          <Glyph name="alert" className="size-4 shrink-0 text-brass" />
          <span className="min-w-0 flex-1">{inv.closedNote}</span>
          <Link
            to="/admin/access"
            className="inline-flex min-h-11 items-center gap-1 text-vermillion hover:text-vermillion-bright"
          >
            {t.admin.accessPage}
            <Glyph name="chevronRight" className="size-3.5" />
          </Link>
        </p>
      ) : null}

      <div className="grid gap-0 lg:grid-cols-[1.2fr_1fr]">
        <div className="min-w-0 border-rule lg:border-r">
          {list.isPending ? (
            <div className="p-5">
              <Skeleton className="h-32 w-full" />
            </div>
          ) : list.isError ? (
            <p role="alert" className="p-5 text-sm text-vermillion">
              {list.error.message}
            </p>
          ) : list.data.invitations.length === 0 ? (
            <div className="p-5">
              <p className="text-sm text-bone">{inv.none}</p>
              <p className="mt-1 text-sm text-bone-faint">{inv.noneHint}</p>
            </div>
          ) : (
            <TableScroll label={inv.title}>
              <table className="w-full text-left text-sm">
                <thead>
                  <tr>
                    <Th>{inv.code}</Th>
                    <Th>{inv.role}</Th>
                    <Th>{inv.uses}</Th>
                    <Th>{inv.expires}</Th>
                    <Th className="text-right">
                      <span className="sr-only">{inv.revoke}</span>
                    </Th>
                  </tr>
                </thead>
                <tbody>
                  {list.data.invitations.map((invitation) => {
                    const state = stateOf(invitation, now)
                    const live = state === 'usable'
                    return (
                      <Tr key={invitation.id} className={live ? undefined : 'opacity-60'}>
                        <Td>
                          <span className="font-mono text-[0.8125rem] text-bone">{invitation.codePrefix}-…</span>
                          {invitation.note ? (
                            <span className="block max-w-48 truncate text-xs text-bone-faint" title={invitation.note}>
                              {invitation.note}
                            </span>
                          ) : null}
                        </Td>
                        <Td>
                          <RoleChip role={invitation.role} />
                        </Td>
                        <Td className="font-mono text-xs whitespace-nowrap tabular-nums">
                          {invitation.uses} / {invitation.maxUses}
                        </Td>
                        <Td className="text-xs whitespace-nowrap">
                          {live ? (
                            <span className="text-bone-dim" title={shortDate(invitation.expiresAt, locale)}>
                              {relative(invitation.expiresAt, locale) ?? '—'}
                            </span>
                          ) : (
                            <span title={state === 'void' ? inv.voidHint : undefined}>
                              <Lamp tone={TONE[state]}>{inv.states[state]}</Lamp>
                            </span>
                          )}
                          {invitation.createdByName ? (
                            <span className="block text-bone-faint">{inv.by(invitation.createdByName)}</span>
                          ) : null}
                        </Td>
                        <Td className="text-right">
                          {live ? (
                            <IconButton
                              glyph="trash"
                              tone="danger"
                              label={`${inv.revoke} ${invitation.codePrefix}`}
                              onClick={() => setWithdrawing(invitation)}
                            />
                          ) : null}
                        </Td>
                      </Tr>
                    )
                  })}
                </tbody>
              </table>
            </TableScroll>
          )}
        </div>

        <div className="border-t border-rule p-5 lg:border-t-0">
          <p className="text-sm font-medium text-bone">{inv.make}</p>
          <p className="mt-1 mb-4 text-xs leading-relaxed text-bone-faint">{inv.lead}</p>

          {issued ? (
            <div className="space-y-3">
              <SecretReveal title={inv.link} secret={link} note={inv.ready} onDismiss={() => setIssued(null)} wrap />
              <p className="text-xs text-bone-faint">
                {inv.codeAlone}{' '}
                <span className="font-mono text-bone select-all">{issued.code}</span>
              </p>
            </div>
          ) : (
            <form
              className="space-y-4"
              onSubmit={(event) => {
                event.preventDefault()
                if (usesValid) create.mutate()
              }}
            >
              <div className="grid gap-4 sm:grid-cols-2">
                <div className="space-y-1.5">
                  <span className="label block">{inv.role}</span>
                  <Segmented<AccountRole>
                    label={inv.role}
                    value={role}
                    onChange={setRole}
                    options={(['member', 'editor'] as AccountRole[]).map((r) => ({
                      value: r,
                      label: t.labels.roles[r],
                    }))}
                  />
                </div>
                <FormField
                  label={inv.maxUses}
                  htmlFor="invitation-uses"
                  error={usesValid ? undefined : '1 – 100'}
                >
                  <Input
                    id="invitation-uses"
                    type="number"
                    inputMode="numeric"
                    min={1}
                    max={100}
                    value={uses}
                    onChange={(event) => setUses(event.target.value)}
                    className="font-mono tabular-nums"
                  />
                </FormField>
              </div>
              <div className="space-y-1.5">
                <span className="label block">{inv.validity}</span>
                <Segmented
                  label={inv.validity}
                  value={days}
                  onChange={setDays}
                  options={DAYS.map((d) => ({ value: d, label: inv.days(Number(d)) }))}
                />
              </div>
              <FormField label={inv.note} htmlFor="invitation-note" hint={inv.noteHint}>
                <Input
                  id="invitation-note"
                  maxLength={200}
                  placeholder={inv.notePlaceholder}
                  value={note}
                  onChange={(event) => setNote(event.target.value)}
                />
              </FormField>
              <p className="text-xs text-bone-faint">{inv.editorNote}</p>
              {create.isError ? (
                <p role="alert" className="text-sm text-vermillion">
                  {create.error.message}
                </p>
              ) : null}
              <Button type="submit" variant="primary" disabled={!usesValid || create.isPending}>
                {create.isPending ? <Spinner className="size-4" /> : <Glyph name="plus" className="size-4" />}
                {create.isPending ? inv.creating : inv.create}
              </Button>
            </form>
          )}
        </div>
      </div>

      <Dialog
        open={withdrawing !== null}
        title={inv.revokeTitle}
        onClose={() => setWithdrawing(null)}
        footer={
          <>
            <Button onClick={() => setWithdrawing(null)}>{t.account.cancel}</Button>
            <Button
              variant="danger"
              disabled={revoke.isPending}
              onClick={() => withdrawing && revoke.mutate(withdrawing.id)}
            >
              {revoke.isPending ? <Spinner className="size-4" /> : null}
              {inv.revoke}
            </Button>
          </>
        }
      >
        <p className="text-sm leading-relaxed text-bone-dim">
          <span className="font-mono text-bone">{withdrawing?.codePrefix}-…</span> — {inv.revokeBody}
        </p>
      </Dialog>
    </Panel>
  )
}
