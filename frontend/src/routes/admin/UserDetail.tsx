/**
 * One member, as an administrator sees them: their role and status, what
 * they are called, their password, where they are signed in, and their keys.
 *
 * A change to the role or the status is saved as it is made, and the server
 * has the last word: your own account, and the last administrator, are left
 * as they are with the reason said beside the choice.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router'

import { Initial, RoleChip, StatusLamp } from '../../components/account/people'
import { SecretReveal } from '../../components/account/SecretReveal'
import { useAdminTitle } from '../../components/AdminShell'
import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  Input,
  Panel,
  PanelHead,
  Segmented,
  Skeleton,
  Spinner,
} from '../../components/ui'
import { describeAgent } from '../../lib/agent'
import { api } from '../../lib/api'
import { dateTime, longDate } from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { AccountRole, AccountStatus, Me, User, UserDetail as Detail } from '../../lib/types'

export function UserDetail() {
  const { id = '' } = useParams()
  const { t, locale } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const me = useQuery({ queryKey: ['me'], queryFn: () => api.get<Me>('/auth/me'), staleTime: 5 * 60_000 })
  const detail = useQuery({
    queryKey: ['users', id],
    queryFn: () => api.get<Detail>(`/users/${encodeURIComponent(id)}`),
  })
  useAdminTitle(detail.data?.user.displayName || detail.data?.user.username || null)

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['users'] })
  }

  const change = useMutation({
    mutationFn: (body: Partial<{ role: AccountRole; status: AccountStatus; displayName: string; email: string }>) =>
      api.patch<User>(`/users/${encodeURIComponent(id)}`, body),
    onSuccess: refresh,
  })
  const remove = useMutation({
    mutationFn: () => api.delete(`/users/${encodeURIComponent(id)}`),
    onSuccess: () => {
      refresh()
      navigate('/admin/users', { replace: true })
    },
  })
  const [deleting, setDeleting] = useState(false)

  if (detail.isPending) {
    return (
      <div className="mx-auto max-w-5xl space-y-4">
        <Skeleton className="h-12 w-72" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }
  if (detail.isError) {
    return <EmptyState title={t.admin.users} hint={detail.error.message} />
  }

  const { user, keys, sessions } = detail.data
  const name = user.displayName || user.username
  const mine = me.data?.user?.id === user.id

  return (
    <div className="mx-auto max-w-5xl">
      <Link
        to="/admin/users"
        className="mb-6 inline-flex min-h-11 items-center gap-1.5 text-sm text-bone-dim transition-colors duration-150 hover:text-bone"
      >
        <Glyph name="arrowLeft" className="size-4" />
        {t.admin.people.back}
      </Link>

      <header className="rise mb-8 flex flex-wrap items-center gap-4">
        <Initial name={name} className="size-14 text-2xl" />
        <div className="min-w-0 flex-1">
          <h1 className="font-display text-3xl font-medium text-bone">{name}</h1>
          <p className="mt-1 font-mono text-xs text-bone-faint">
            {[
              user.username,
              user.email,
              t.admin.people.since(longDate(user.createdAt, locale) ?? ''),
            ]
              .filter(Boolean)
              .join(' · ')}
          </p>
        </div>
        <div className="flex items-center gap-3">
          <RoleChip role={user.role} />
          <StatusLamp status={user.status} />
        </div>
      </header>

      <div className="grid items-start gap-6 lg:grid-cols-2">
        <div className="space-y-6">
          <Panel label={t.admin.people.access} className="rise">
            <PanelHead title={t.admin.people.access} />
            <div className="space-y-4 p-5">
              <div className="space-y-1.5">
                <span className="label block">{t.admin.people.role}</span>
                <Segmented<AccountRole>
                  label={t.admin.people.role}
                  value={user.role}
                  disabled={mine || change.isPending}
                  onChange={(role) => change.mutate({ role })}
                  options={(['member', 'editor', 'admin'] as const).map((r) => ({ value: r, label: t.labels.roles[r] }))}
                />
                <p className="text-xs text-bone-faint">{t.account.roleHints[user.role]}</p>
              </div>
              <div className="space-y-1.5">
                <span className="label block">{t.admin.people.status}</span>
                <Segmented<AccountStatus>
                  label={t.admin.people.status}
                  value={user.status}
                  disabled={mine || change.isPending}
                  onChange={(status) => change.mutate({ status })}
                  options={(['active', 'pending', 'disabled'] as const).map((s) => ({
                    value: s,
                    label: t.labels.statuses[s],
                  }))}
                />
              </div>
              {mine ? <p className="text-xs text-brass">{t.admin.people.selfNote}</p> : null}
              {change.isError ? (
                <p role="alert" className="flex items-center gap-1.5 text-xs text-vermillion">
                  <Glyph name="alert" className="size-3.5" />
                  {change.error.message}
                </p>
              ) : null}
            </div>
          </Panel>

          <ProfileEditor user={user} onSaved={refresh} />
          <PasswordReset id={id} />
        </div>

        <div className="space-y-6">
          <SessionsPanel id={id} sessions={sessions} onChanged={refresh} />
          <KeysPanel keys={keys} onChanged={refresh} />

          {mine ? null : (
            <Panel className="rise border-vermillion-deep/60">
              <div className="flex flex-wrap items-center justify-between gap-3 p-5">
                <p className="max-w-prose text-xs leading-relaxed text-bone-faint">{t.admin.people.deleteBody}</p>
                <Button variant="danger" onClick={() => setDeleting(true)}>
                  <Glyph name="trash" className="size-4" />
                  {t.admin.people.delete}
                </Button>
              </div>
            </Panel>
          )}
        </div>
      </div>

      <Dialog
        open={deleting}
        title={t.admin.people.deleteTitle(1)}
        onClose={() => setDeleting(false)}
        footer={
          <>
            <Button onClick={() => setDeleting(false)}>{t.account.cancel}</Button>
            <Button variant="danger" disabled={remove.isPending} onClick={() => remove.mutate()}>
              {remove.isPending ? <Spinner className="size-4" /> : null}
              {t.admin.people.deleteConfirm}
            </Button>
          </>
        }
      >
        {t.admin.people.deleteBody}
        {remove.isError ? (
          <p role="alert" className="mt-2 text-vermillion">
            {remove.error.message}
          </p>
        ) : null}
      </Dialog>
    </div>
  )
}

function ProfileEditor({ user, onSaved }: { user: User; onSaved: () => void }) {
  const { t } = useI18n()
  const [displayName, setDisplayName] = useState(user.displayName ?? '')
  const [email, setEmail] = useState(user.email ?? '')

  const save = useMutation({
    mutationFn: () => api.patch<User>(`/users/${encodeURIComponent(user.id)}`, { displayName, email }),
    onSuccess: onSaved,
  })
  const changed = displayName !== (user.displayName ?? '') || email !== (user.email ?? '')

  return (
    <Panel label={t.account.profile} className="rise" style={{ animationDelay: '60ms' }}>
      <PanelHead title={t.account.profile} />
      <form
        className="grid gap-4 p-5"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate()
        }}
      >
        <FormField label={t.account.displayName} htmlFor="member-name">
          <Input id="member-name" maxLength={80} value={displayName} onChange={(event) => setDisplayName(event.target.value)} />
        </FormField>
        <FormField
          label={t.account.email}
          htmlFor="member-email"
          error={save.isError ? save.error.message : undefined}
        >
          <Input id="member-email" type="email" value={email} onChange={(event) => setEmail(event.target.value)} />
        </FormField>
        <div>
          <Button type="submit" variant="primary" disabled={!changed || save.isPending}>
            {save.isPending ? <Spinner className="size-4" /> : <Glyph name="check" className="size-4" />}
            {t.account.save}
          </Button>
        </div>
      </form>
    </Panel>
  )
}

function PasswordReset({ id }: { id: string }) {
  const { t } = useI18n()
  const [password, setPassword] = useState('')

  const reset = useMutation({
    mutationFn: () =>
      api.post<{ password?: string }>(`/users/${encodeURIComponent(id)}/password`, {
        password: password || undefined,
      }),
    onSuccess: () => setPassword(''),
  })

  return (
    <Panel label={t.admin.people.resetPassword} className="rise" style={{ animationDelay: '100ms' }}>
      <PanelHead title={t.account.password} />
      <form
        className="grid gap-4 p-5"
        onSubmit={(event) => {
          event.preventDefault()
          reset.mutate()
        }}
      >
        <p className="text-xs leading-relaxed text-bone-faint">{t.admin.people.resetBody}</p>
        <FormField
          label={t.admin.people.passwordOptional}
          htmlFor="member-password"
          hint={t.admin.config.passwordRule}
          error={reset.isError ? reset.error.message : undefined}
        >
          <Input
            id="member-password"
            type="password"
            autoComplete="new-password"
            minLength={12}
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </FormField>
        <div>
          <Button type="submit" disabled={reset.isPending || (password !== '' && password.length < 12)}>
            {reset.isPending ? <Spinner className="size-4" /> : <Glyph name="lock" className="size-4" />}
            {t.admin.people.resetConfirm}
          </Button>
        </div>
        {reset.data?.password ? (
          <SecretReveal title={t.admin.people.resetDone} secret={reset.data.password} onDismiss={() => reset.reset()} />
        ) : null}
      </form>
    </Panel>
  )
}

function SessionsPanel({
  id,
  sessions,
  onChanged,
}: {
  id: string
  sessions: Detail['sessions']
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const closeAll = useMutation({
    mutationFn: () => api.delete(`/users/${encodeURIComponent(id)}/sessions`),
    onSuccess: onChanged,
  })

  return (
    <Panel label={t.account.sessions} className="rise" style={{ animationDelay: '40ms' }}>
      <PanelHead
        title={t.account.sessions}
        action={
          sessions.length ? (
            <Button size="sm" variant="quiet" className="text-vermillion" onClick={() => closeAll.mutate()}>
              {t.admin.people.closeSessions}
            </Button>
          ) : null
        }
      />
      {sessions.length ? (
        <ul className="divide-y divide-rule">
          {sessions.map((session) => (
            <li key={session.id} className="px-5 py-3">
              <p className="text-sm text-bone">{describeAgent(session.userAgent) ?? t.account.unknownDevice}</p>
              <p className="font-mono text-xs text-bone-faint">
                {[session.ip, t.account.lastSeen(dateTime(session.lastSeenAt ?? session.createdAt, locale) ?? '')]
                  .filter(Boolean)
                  .join(' · ')}
              </p>
            </li>
          ))}
        </ul>
      ) : (
        <p className="px-5 py-4 text-sm text-bone-faint">{t.admin.people.noSessions}</p>
      )}
    </Panel>
  )
}

function KeysPanel({ keys, onChanged }: { keys: Detail['keys']; onChanged: () => void }) {
  const { t, locale } = useI18n()
  const [revoking, setRevoking] = useState<{ id: string; name: string } | null>(null)

  const revoke = useMutation({
    mutationFn: (keyId: string) => api.delete(`/clients/${encodeURIComponent(keyId)}`),
    onSuccess: () => {
      setRevoking(null)
      onChanged()
    },
  })

  return (
    <Panel label={t.admin.people.keys} className="rise" style={{ animationDelay: '80ms' }}>
      <PanelHead title={t.admin.people.keys} />
      {keys.length ? (
        <ul className="divide-y divide-rule">
          {keys.map((key) => (
            <li key={key.id} className="flex flex-wrap items-center gap-3 px-5 py-3">
              <div className="min-w-0 flex-1">
                <p className="flex flex-wrap items-center gap-2 text-sm text-bone">
                  {key.name}
                  <span className="font-mono text-xs text-bone-faint">{key.keyPrefix}…</span>
                  {key.scopes
                    .filter((s) => s !== 'read')
                    .map((s) => (
                      <Chip key={s} tone="manual">
                        {t.account.scopes[s] ?? s}
                      </Chip>
                    ))}
                </p>
                <p className="mt-0.5 text-xs text-bone-faint">
                  {key.lastUsedAt ? t.account.used(dateTime(key.lastUsedAt, locale) ?? '') : t.account.neverUsed}
                </p>
              </div>
              <Button size="sm" variant="quiet" className="text-vermillion" onClick={() => setRevoking({ id: key.id, name: key.name })}>
                {t.account.revoke}
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="px-5 py-4 text-sm text-bone-faint">{t.admin.people.noKeys}</p>
      )}

      <Dialog
        open={revoking !== null}
        title={t.account.revokeTitle(revoking?.name ?? '')}
        onClose={() => setRevoking(null)}
        footer={
          <>
            <Button onClick={() => setRevoking(null)}>{t.account.cancel}</Button>
            <Button variant="danger" disabled={revoke.isPending} onClick={() => revoking && revoke.mutate(revoking.id)}>
              {t.account.revokeConfirm}
            </Button>
          </>
        }
      >
        {t.account.revokeBody}
      </Dialog>
    </Panel>
  )
}
