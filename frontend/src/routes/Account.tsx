/**
 * A person's own account: what they are called, how they sign in, where
 * they are signed in, and the keys that act in their name.
 *
 * The same page in both shells: a member reaches it from the catalogue, an
 * editor or an administrator from the administration's sidebar. What it
 * shows depends on the person, never on the shell.
 *
 * Keys follow the server's limit. At one, a person has *their* key — made,
 * regenerated, revoked, never listed. Above one, a list with a name each.
 * At none, a sentence saying who makes them here.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Navigate } from 'react-router'

import { ChangePassword } from '../components/account/ChangePassword'
import { RoleChip } from '../components/account/people'
import { SecretReveal } from '../components/account/SecretReveal'
import {
  Button,
  Chip,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  Input,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Segmented,
  Skeleton,
  Spinner,
} from '../components/ui'
import { describeAgent } from '../lib/agent'
import { ApiError, api } from '../lib/api'
import { dateTime } from '../lib/format'
import { LANGS, useI18n, type Lang } from '../lib/i18n'
import type { AccountKeys, AccountSession, ApiClient, IssuedKey, User } from '../lib/types'

export function Account() {
  const { t } = useI18n()

  const account = useQuery({
    queryKey: ['account'],
    queryFn: () => api.get<User>('/account'),
    retry: false,
  })

  if (account.isPending) {
    return (
      <div className="mx-auto max-w-5xl space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (account.isError) {
    // Nobody signed in: the door, and back here after it.
    if (account.error instanceof ApiError && account.error.isUnauthorized) {
      return <Navigate to="/login?next=/account" replace />
    }
    return <EmptyState title={t.account.eyebrow} hint={account.error.message} />
  }

  const me = account.data

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.account.eyebrow}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {me.displayName || me.username}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.account.lead}</p>
      </header>

      <div className="grid items-start gap-6 lg:grid-cols-2">
        <div className="space-y-6">
          <Profile me={me} />
          <Panel label={t.account.password} className="rise" style={{ animationDelay: '80ms' }}>
            <PanelHead title={t.account.password} />
            {me.hasPassword ? (
              <ChangePassword />
            ) : (
              <p className="p-5 text-sm leading-relaxed text-bone-dim">{t.account.passwordNone}</p>
            )}
          </Panel>
          <Sessions />
        </div>
        <Keys />
      </div>
    </div>
  )
}

/* ── Profile ──────────────────────────────────────────────────────────────── */

function Profile({ me }: { me: User }) {
  const { t, setLang } = useI18n()
  const queryClient = useQueryClient()

  const [displayName, setDisplayName] = useState(me.displayName ?? '')
  const [email, setEmail] = useState(me.email ?? '')
  const [locale, setLocale] = useState<string>(me.locale ?? '')

  const save = useMutation({
    mutationFn: () => api.patch<User>('/account', { displayName, email, locale }),
    onSuccess: (saved) => {
      queryClient.setQueryData(['account'], saved)
      void queryClient.invalidateQueries({ queryKey: ['me'] })
      // Chosen here, it takes at once rather than on the next visit.
      if (saved.locale && (LANGS as string[]).includes(saved.locale)) setLang(saved.locale as Lang)
    },
  })

  const changed =
    displayName !== (me.displayName ?? '') ||
    email !== (me.email ?? '') ||
    locale !== (me.locale ?? '')

  return (
    <Panel label={t.account.profile} className="rise">
      <PanelHead title={t.account.profile} action={<RoleChip role={me.role} />} />
      <form
        className="grid gap-4 p-5"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate()
        }}
      >
        <FormField label={t.account.displayName} htmlFor="account-name" hint={t.account.displayNameHint}>
          <Input
            id="account-name"
            autoComplete="name"
            maxLength={80}
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
          />
        </FormField>

        <FormField label={t.account.username} htmlFor="account-username" hint={t.account.usernameHint}>
          <Input id="account-username" readOnly value={me.username} className="font-mono text-bone-dim" />
        </FormField>

        <FormField
          label={t.account.email}
          htmlFor="account-email"
          hint={t.account.emailHint}
          error={save.isError ? save.error.message : undefined}
        >
          <Input
            id="account-email"
            type="email"
            autoComplete="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
        </FormField>

        <div className="space-y-1.5">
          <span className="label block">{t.account.language}</span>
          <Segmented
            label={t.account.language}
            value={locale}
            onChange={setLocale}
            options={[
              { value: '', label: t.account.languageAuto },
              { value: 'fr', label: 'Français' },
              { value: 'en', label: 'English' },
            ]}
          />
        </div>

        <p className="text-xs leading-relaxed text-bone-faint">{t.account.roleHints[me.role]}</p>

        <div className="flex items-center gap-3">
          <Button type="submit" variant="primary" disabled={!changed || save.isPending}>
            {save.isPending ? <Spinner className="size-4" /> : <Glyph name="check" className="size-4" />}
            {save.isPending ? t.account.saving : t.account.save}
          </Button>
          {save.isSuccess && !changed ? (
            <span role="status" className="text-sm text-moss">
              {t.account.saved}
            </span>
          ) : null}
        </div>
      </form>
    </Panel>
  )
}

/* ── Sessions ─────────────────────────────────────────────────────────────── */

function Sessions() {
  const { t, locale } = useI18n()
  const queryClient = useQueryClient()

  const sessions = useQuery({
    queryKey: ['account', 'sessions'],
    queryFn: () => api.get<AccountSession[]>('/account/sessions'),
  })

  const close = useMutation({
    mutationFn: (id: string) => api.delete(`/account/sessions/${encodeURIComponent(id)}`),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['account', 'sessions'] }),
  })
  const elsewhere = useMutation({
    mutationFn: () => api.delete('/account/sessions'),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['account', 'sessions'] }),
  })

  const others = (sessions.data ?? []).filter((s) => !s.current).length

  return (
    <Panel label={t.account.sessions} className="rise" style={{ animationDelay: '120ms' }}>
      <PanelHead
        title={t.account.sessions}
        action={
          others ? (
            <Button size="sm" variant="quiet" onClick={() => elsewhere.mutate()} disabled={elsewhere.isPending}>
              {t.account.closeOthers}
            </Button>
          ) : null
        }
      />
      <p className="px-5 pt-4 text-xs text-bone-faint">{t.account.sessionsLead}</p>
      <ul className="divide-y divide-rule">
        {(sessions.data ?? []).map((session) => (
          <li key={session.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-5 py-3">
            <div className="min-w-0 flex-1">
              <p className="text-sm text-bone">{describeAgent(session.userAgent) ?? t.account.unknownDevice}</p>
              <p className="font-mono text-xs text-bone-faint">
                {[session.ip, t.account.lastSeen(dateTime(session.lastSeenAt ?? session.createdAt, locale) ?? '')]
                  .filter(Boolean)
                  .join(' · ')}
              </p>
            </div>
            {session.current ? (
              <Chip className="border-moss/50 text-moss">{t.account.thisOne}</Chip>
            ) : (
              <Button
                size="sm"
                variant="quiet"
                className="text-vermillion"
                onClick={() => close.mutate(session.id)}
                disabled={close.isPending}
              >
                {t.account.close}
              </Button>
            )}
          </li>
        ))}
      </ul>
    </Panel>
  )
}

/* ── Keys ─────────────────────────────────────────────────────────────────── */

function Keys() {
  const { t } = useI18n()
  const queryClient = useQueryClient()
  const [issued, setIssued] = useState<IssuedKey | null>(null)

  const keys = useQuery({
    queryKey: ['account', 'keys'],
    queryFn: () => api.get<AccountKeys>('/account/keys'),
  })

  const refresh = () => void queryClient.invalidateQueries({ queryKey: ['account', 'keys'] })

  if (keys.isPending) return <Skeleton className="h-64 w-full" />
  if (keys.isError) return <EmptyState title={t.account.keys} hint={keys.error.message} />

  const { limit, scopes } = keys.data
  const held = keys.data.keys
  // One key, one card — unless keys made while the limit was higher are
  // still here, and each must stay in sight to be revoked.
  const single = limit === 1 && held.length <= 1

  return (
    <Panel
      label={single ? t.account.key : t.account.keys}
      className="rise lg:sticky lg:top-6"
      style={{ animationDelay: '60ms' }}
    >
      <PanelHead
        title={single ? t.account.key : t.account.keys}
        action={
          <span className="font-mono text-xs text-bone-faint tabular-nums">
            {limit === undefined
              ? t.account.noLimit
              : single
                ? t.account.oneKey
                : t.account.keyCount(held.length, limit)}
          </span>
        }
      />
      <div className="space-y-4 p-5">
        <p className="text-sm leading-relaxed text-bone-dim">{t.account.keysLead}</p>

        {issued ? (
          <SecretReveal title={t.account.newSecret} secret={issued.secret} onDismiss={() => setIssued(null)} />
        ) : null}

        {limit === 0 && held.length === 0 ? (
          <p className="text-sm text-bone-faint">{t.account.keysClosed}</p>
        ) : single ? (
          <SingleKey
            current={held[0]}
            onIssued={(next) => {
              setIssued(next)
              refresh()
            }}
            onChanged={refresh}
          />
        ) : (
          <KeyList
            keys={held}
            limit={limit}
            scopes={scopes}
            onIssued={(next) => {
              setIssued(next)
              refresh()
            }}
            onChanged={refresh}
          />
        )}
      </div>
    </Panel>
  )
}

function keyLine(key: ApiClient, t: ReturnType<typeof useI18n>['t'], locale: string) {
  const created = t.account.created(dateTime(key.createdAt, locale) ?? '')
  const used = key.lastUsedAt
    ? key.lastUsedIp
      ? t.account.usedFrom(dateTime(key.lastUsedAt, locale) ?? '', key.lastUsedIp)
      : t.account.used(dateTime(key.lastUsedAt, locale) ?? '')
    : t.account.neverUsed
  return `${created} · ${used}`
}

/** Their key, when the server allows one: made, regenerated, revoked. */
function SingleKey({
  current,
  onIssued,
  onChanged,
}: {
  current: ApiClient | undefined
  onIssued: (issued: IssuedKey) => void
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const [confirm, setConfirm] = useState<'rotate' | 'revoke' | null>(null)

  const make = useMutation({
    mutationFn: () => api.post<IssuedKey>('/account/keys', { name: t.account.key }),
    onSuccess: onIssued,
  })
  const rotate = useMutation({
    mutationFn: (id: string) => api.post<IssuedKey>(`/account/keys/${id}/rotate`),
    onSuccess: (issued) => {
      setConfirm(null)
      onIssued(issued)
    },
  })
  const revoke = useMutation({
    mutationFn: (id: string) => api.delete(`/account/keys/${id}`),
    onSuccess: () => {
      setConfirm(null)
      onChanged()
    },
  })

  if (!current) {
    return (
      <div>
        <Button variant="primary" onClick={() => make.mutate()} disabled={make.isPending}>
          {make.isPending ? <Spinner className="size-4" /> : <Glyph name="key" className="size-4" />}
          {t.account.makeYourKey}
        </Button>
        {make.isError ? (
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {make.error.message}
          </p>
        ) : null}
      </div>
    )
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-panel border border-rule bg-ink p-4">
        <div className="min-w-0">
          <p className="font-mono text-sm text-bone">{current.keyPrefix}…</p>
          <p className="mt-1 text-xs text-bone-faint">{keyLine(current, t, locale)}</p>
        </div>
        <Lamp tone={current.isEnabled ? 'moss' : 'faint'}>
          {current.isEnabled ? t.account.active : t.account.paused}
        </Lamp>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => setConfirm('rotate')}>
          <Glyph name="refresh" className="size-4" />
          {t.account.regenerate}
        </Button>
        <Button variant="quiet" className="text-vermillion" onClick={() => setConfirm('revoke')}>
          {t.account.revoke}
        </Button>
      </div>

      <Dialog
        open={confirm === 'rotate'}
        title={t.account.regenerateTitle}
        onClose={() => setConfirm(null)}
        footer={
          <>
            <Button onClick={() => setConfirm(null)}>{t.account.cancel}</Button>
            <Button variant="danger" onClick={() => rotate.mutate(current.id)} disabled={rotate.isPending}>
              {t.account.regenerateConfirm}
            </Button>
          </>
        }
      >
        {t.account.regenerateBody}
      </Dialog>
      <Dialog
        open={confirm === 'revoke'}
        title={t.account.revokeTitle(current.name)}
        onClose={() => setConfirm(null)}
        footer={
          <>
            <Button onClick={() => setConfirm(null)}>{t.account.cancel}</Button>
            <Button variant="danger" onClick={() => revoke.mutate(current.id)} disabled={revoke.isPending}>
              {t.account.revokeConfirm}
            </Button>
          </>
        }
      >
        {t.account.revokeBody}
      </Dialog>
    </div>
  )
}

/** Several keys, each named for what uses it. */
function KeyList({
  keys,
  limit,
  scopes,
  onIssued,
  onChanged,
}: {
  keys: ApiClient[]
  limit: number | undefined
  scopes: string[]
  onIssued: (issued: IssuedKey) => void
  onChanged: () => void
}) {
  const { t, locale } = useI18n()
  const [name, setName] = useState('')
  // The rights a key may carry, widest last: read, read and write, and all.
  const levels = scopes.map((_, index) => scopes.slice(0, index + 1).join('+'))
  const [level, setLevel] = useState(levels[0] ?? 'read')
  const [confirm, setConfirm] = useState<{ key: ApiClient; what: 'rotate' | 'revoke' } | null>(null)

  const create = useMutation({
    mutationFn: () =>
      api.post<IssuedKey>('/account/keys', { name: name.trim(), scopes: level.split('+') }),
    onSuccess: (issued) => {
      setName('')
      onIssued(issued)
    },
  })
  const toggle = useMutation({
    mutationFn: (key: ApiClient) => api.patch(`/account/keys/${key.id}`, { isEnabled: !key.isEnabled }),
    onSuccess: onChanged,
  })
  const rotate = useMutation({
    mutationFn: (id: string) => api.post<IssuedKey>(`/account/keys/${id}/rotate`),
    onSuccess: (issued) => {
      setConfirm(null)
      onIssued(issued)
    },
  })
  const revoke = useMutation({
    mutationFn: (id: string) => api.delete(`/account/keys/${id}`),
    onSuccess: () => {
      setConfirm(null)
      onChanged()
    },
  })

  const full = limit !== undefined && keys.length >= limit
  const rights = (key: ApiClient) =>
    key.scopes.includes('admin') ? 'admin' : key.scopes.includes('write') ? 'write' : 'read'

  return (
    <div className="space-y-4">
      {keys.length ? (
        <ul className="divide-y divide-rule rounded-panel border border-rule">
          {keys.map((key) => (
            <li key={key.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3">
              <div className="min-w-0 flex-1">
                <p className="flex flex-wrap items-center gap-2 text-sm text-bone">
                  {key.name}
                  <span className="font-mono text-xs text-bone-faint">{key.keyPrefix}…</span>
                  <Chip tone={rights(key) === 'read' ? 'neutral' : 'manual'}>{t.account.scopes[rights(key)]}</Chip>
                  {key.isEnabled ? null : <Chip>{t.account.paused}</Chip>}
                </p>
                <p className="mt-0.5 text-xs text-bone-faint">{keyLine(key, t, locale)}</p>
              </div>
              <div className="flex flex-wrap gap-1">
                <Button size="sm" variant="quiet" onClick={() => toggle.mutate(key)} disabled={toggle.isPending}>
                  {key.isEnabled ? t.account.pause : t.account.resume}
                </Button>
                <Button size="sm" variant="quiet" onClick={() => setConfirm({ key, what: 'rotate' })}>
                  {t.account.regenerate}
                </Button>
                <Button
                  size="sm"
                  variant="quiet"
                  className="text-vermillion"
                  onClick={() => setConfirm({ key, what: 'revoke' })}
                >
                  {t.account.revoke}
                </Button>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <div className="rounded-panel border border-dashed border-rule-bright p-4">
          <p className="text-sm text-bone">{t.account.noKeys}</p>
          <p className="mt-1 text-xs text-bone-faint">{t.account.noKeysHint}</p>
        </div>
      )}

      {full ? (
        <p className="text-xs text-bone-faint">{t.account.limitReached}</p>
      ) : (
        <form
          className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto]"
          onSubmit={(event) => {
            event.preventDefault()
            if (name.trim()) create.mutate()
          }}
        >
          <FormField
            label={t.account.keyName}
            htmlFor="new-key-name"
            error={create.isError ? create.error.message : undefined}
          >
            <Input
              id="new-key-name"
              required
              maxLength={60}
              placeholder={t.account.keyNamePlaceholder}
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </FormField>
          {levels.length > 1 ? (
            <div className="space-y-1.5">
              <span className="label block">{t.account.scope}</span>
              <Segmented
                label={t.account.scope}
                value={level}
                onChange={setLevel}
                options={levels.map((value) => ({
                  value,
                  label: t.account.scopes[value.split('+').at(-1) ?? 'read'],
                }))}
              />
            </div>
          ) : null}
          <div className="sm:col-span-2">
            <Button type="submit" variant="primary" disabled={!name.trim() || create.isPending}>
              {create.isPending ? <Spinner className="size-4" /> : <Glyph name="plus" className="size-4" />}
              {create.isPending ? t.account.creating : t.account.create}
            </Button>
          </div>
        </form>
      )}

      <Dialog
        open={confirm !== null}
        title={confirm?.what === 'rotate' ? t.account.regenerateTitle : t.account.revokeTitle(confirm?.key.name ?? '')}
        onClose={() => setConfirm(null)}
        footer={
          <>
            <Button onClick={() => setConfirm(null)}>{t.account.cancel}</Button>
            <Button
              variant="danger"
              disabled={rotate.isPending || revoke.isPending}
              onClick={() => {
                if (!confirm) return
                if (confirm.what === 'rotate') rotate.mutate(confirm.key.id)
                else revoke.mutate(confirm.key.id)
              }}
            >
              {confirm?.what === 'rotate' ? t.account.regenerateConfirm : t.account.revokeConfirm}
            </Button>
          </>
        }
      >
        {confirm?.what === 'rotate' ? t.account.regenerateBody : t.account.revokeBody}
      </Dialog>
      {rotate.isError || revoke.isError ? (
        <p role="alert" className="text-xs text-vermillion">
          {(rotate.error ?? revoke.error) instanceof ApiError ? (rotate.error ?? revoke.error)?.message : null}
        </p>
      ) : null}
    </div>
  )
}
