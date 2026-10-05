/**
 * The identity provider, as an administrator sets it up.
 *
 * One form, saved at once, because its fields only make sense together: an
 * issuer without a client is not a provider, and a provider half set up is
 * not offered anywhere. The secret goes one way — typed, saved, never shown
 * again — and the return address, which the provider has to be told, is
 * there to be copied. Discovery can be tried before anything is saved.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'

import { api } from '../../lib/api'
import { useI18n } from '../../lib/i18n'
import type { OidcConfiguration, OidcTest } from '../../lib/types'
import { Button, Chip, FormField, Glyph, Input, Lamp, Panel, PanelHead, Skeleton, Spinner, Toggle } from '../ui'

type Draft = Omit<
  OidcConfiguration,
  'secretSet' | 'secretFromEnv' | 'redirectUri' | 'ready' | 'breakGlass' | 'passwordForced'
>

function draftOf(config: OidcConfiguration): Draft {
  const {
    secretSet: _s,
    secretFromEnv: _e,
    redirectUri: _r,
    ready: _y,
    breakGlass: _b,
    passwordForced: _f,
    ...draft
  } = config
  return draft
}

export function OidcSettings({ number, delay }: { number: number; delay: number }) {
  const { t } = useI18n()
  const o = t.admin.oidc
  const queryClient = useQueryClient()

  const config = useQuery({
    queryKey: ['admin', 'oidc'],
    queryFn: () => api.get<OidcConfiguration>('/admin/oidc'),
  })

  const [draft, setDraft] = useState<Draft | null>(null)
  // The secret as typed, when one is being typed; the stored one stays
  // unless something is, or it is asked to go.
  const [secret, setSecret] = useState<string | null>(null)
  const [clearSecret, setClearSecret] = useState(false)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    if (config.data && draft === null) setDraft(draftOf(config.data))
  }, [config.data, draft])

  const save = useMutation({
    mutationFn: (body: Draft & { clientSecret?: string }) => api.put<OidcConfiguration>('/admin/oidc', body),
    onSuccess: (saved) => {
      queryClient.setQueryData(['admin', 'oidc'], saved)
      setDraft(draftOf(saved))
      setSecret(null)
      setClearSecret(false)
      void queryClient.invalidateQueries({ queryKey: ['auth', 'options'] })
      void queryClient.invalidateQueries({ queryKey: ['admin', 'access'] })
    },
  })

  const test = useMutation({
    mutationFn: (issuer: string) => api.post<OidcTest>('/admin/oidc/test', { issuer }),
  })

  // The failure first: a read that never came back leaves no draft to wait
  // for, and was a skeleton that shimmered for as long as the page was open.
  if (config.isError) {
    return (
      <p role="alert" className="text-sm text-vermillion">
        {config.error.message}
      </p>
    )
  }
  if (config.isPending || draft === null) return <Skeleton className="h-96 w-full" />

  const current = config.data
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft({ ...draft, [key]: value })
  const text = (key: keyof Draft, label: string, hint?: string, extra?: React.InputHTMLAttributes<HTMLInputElement>) => (
    <FormField label={label} htmlFor={`oidc-${key}`} hint={hint}>
      <Input
        id={`oidc-${key}`}
        value={String(draft[key] ?? '')}
        onChange={(event) => set(key, event.target.value as never)}
        spellCheck={false}
        autoComplete="off"
        {...extra}
      />
    </FormField>
  )
  const toggle = (key: keyof Draft, label: string, hint: string, disabled = false) => (
    <div className="flex items-start justify-between gap-4 py-2">
      <div className="min-w-0">
        <p className="text-sm text-bone">{label}</p>
        <p className="mt-0.5 text-xs leading-relaxed text-bone-faint">{hint}</p>
      </div>
      <Toggle
        checked={Boolean(draft[key])}
        label={label}
        onChange={(next) => set(key, next as never)}
        disabled={disabled || save.isPending}
      />
    </div>
  )

  const copy = async () => {
    if (!current.redirectUri) return
    try {
      await navigator.clipboard.writeText(current.redirectUri)
      setCopied(true)
    } catch {
      setCopied(false)
    }
  }

  return (
    <Panel id="oidc" label={o.title} className="rise" style={{ animationDelay: `${delay}ms` }}>
      <PanelHead
        title={
          <span>
            <span className="text-vermillion">{number}</span> · {o.title}
          </span>
        }
        action={<Lamp tone={current.ready ? 'moss' : 'faint'}>{current.ready ? o.ready : o.notReady}</Lamp>}
      />

      <form
        className="space-y-6 p-5"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate({
            ...draft,
            ...(clearSecret ? { clientSecret: '' } : secret ? { clientSecret: secret } : {}),
          })
        }}
      >
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{o.lead}</p>

        {toggle('enabled', o.enabled, o.enabledHint)}

        <div className="grid gap-5 md:grid-cols-2">
          <div className="space-y-2 md:col-span-2">
            {text('issuer', o.issuer, o.issuerHint, { type: 'url', placeholder: 'https://auth.example.org/application/o/cinematheque/' })}
            <div className="flex flex-wrap items-center gap-3">
              <Button
                size="sm"
                disabled={!draft.issuer.trim() || test.isPending}
                onClick={() => test.mutate(draft.issuer.trim())}
              >
                {test.isPending ? <Spinner className="size-4" /> : <Glyph name="refresh" className="size-4" />}
                {test.isPending ? o.testing : o.test}
              </Button>
              {test.data?.ok && test.data.discovery ? (
                <span role="status" className="flex items-center gap-1.5 text-xs text-moss">
                  <Glyph name="check" className="size-3.5" />
                  {o.found(test.data.discovery.keys, test.data.discovery.signingAlgorithms.join(', '))}
                </span>
              ) : test.data && !test.data.ok ? (
                <span role="alert" className="text-xs break-words text-vermillion">
                  {o.testFailed} {test.data.error}
                </span>
              ) : test.isError ? (
                <span role="alert" className="text-xs text-vermillion">
                  {o.testFailed} {test.error.message}
                </span>
              ) : null}
            </div>
          </div>

          <div className="space-y-1.5 md:col-span-2">
            <span className="label block">{o.redirect}</span>
            {current.redirectUri ? (
              <div className="flex flex-wrap items-center gap-2">
                <code className="min-w-0 flex-1 rounded-card border border-rule bg-ink px-3 py-2.5 font-mono text-[0.8125rem] break-all text-bone">
                  {current.redirectUri}
                </code>
                <Button size="sm" onClick={() => void copy()}>
                  <Glyph name={copied ? 'check' : 'copy'} className="size-4" />
                  {copied ? t.account.copied : t.account.copy}
                </Button>
              </div>
            ) : (
              <p className="flex items-start gap-2 text-sm text-brass">
                <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
                {o.redirectMissing}
              </p>
            )}
          </div>

          {text('clientId', o.clientId)}

          <div className="space-y-1.5">
            <label className="label block" htmlFor="oidc-secret">
              {o.secret}
            </label>
            {current.secretFromEnv ? (
              <p className="text-sm text-bone-dim">{o.secretEnv}</p>
            ) : current.secretSet && clearSecret ? (
              <div className="flex flex-wrap items-center gap-2">
                <Chip tone="accent">{o.secretClear}</Chip>
                <Button size="sm" variant="quiet" onClick={() => setClearSecret(false)}>
                  {o.secretKeep}
                </Button>
              </div>
            ) : current.secretSet && secret === null ? (
              <div className="flex flex-wrap items-center gap-2">
                <Chip tone="provider">
                  <Glyph name="lock" className="size-3" />
                  {o.secretSet}
                </Chip>
                <Button size="sm" onClick={() => setSecret('')}>
                  {o.secretReplace}
                </Button>
                <Button size="sm" variant="quiet" onClick={() => setClearSecret(true)}>
                  {o.secretClear}
                </Button>
              </div>
            ) : (
              <div className="space-y-2">
                <Input
                  id="oidc-secret"
                  type="password"
                  autoComplete="new-password"
                  value={secret ?? ''}
                  onChange={(event) => setSecret(event.target.value)}
                />
                {current.secretSet ? (
                  <div className="flex flex-wrap gap-2">
                    <Button size="sm" variant="quiet" onClick={() => setSecret(null)}>
                      {o.secretKeep}
                    </Button>
                  </div>
                ) : null}
              </div>
            )}
            <p className="text-xs text-bone-faint">{o.secretHint}</p>
          </div>

          {text('scopes', o.scopes, o.scopesHint, { placeholder: 'openid profile email' })}
          {text('buttonLabel', o.button, o.buttonHint, { placeholder: t.auth.sso })}
        </div>

        <div className="divide-y divide-rule border-y border-rule">
          {toggle('autoRegister', o.autoRegister, o.autoRegisterHint)}
        </div>

        <div className="grid gap-5 md:grid-cols-3">
          {text('roleClaim', o.roleClaim, o.roleClaimHint, { placeholder: 'groups' })}
          {text('adminValues', o.adminValues, o.valuesHint, { placeholder: 'cine-admins' })}
          {text('editorValues', o.editorValues, o.valuesHint, { placeholder: 'cine-editors' })}
        </div>

        <div className="border-t border-rule pt-2">
          {toggle(
            'passwordLogin',
            o.passwordLogin,
            current.passwordForced
              ? o.passwordForced
              : !current.ready && draft.passwordLogin
                ? o.passwordLoginLocked
                : current.breakGlass
                  ? o.passwordLoginHint(current.breakGlass)
                  : o.passwordLoginNoDoor,
            current.passwordForced || (!current.ready && draft.passwordLogin),
          )}
          {!current.passwordForced && draft.passwordLogin ? (
            <p className="mt-1 text-xs leading-relaxed text-bone-faint">{o.passwordOffNeeds}</p>
          ) : null}
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Button type="submit" variant="primary" disabled={save.isPending}>
            {save.isPending ? <Spinner className="size-4" /> : null}
            {save.isPending ? o.saving : o.save}
          </Button>
          {save.isSuccess ? (
            <span role="status" className="flex items-center gap-1.5 text-sm text-moss">
              <Glyph name="check" className="size-4" />
              {o.saved}
            </span>
          ) : null}
          {save.isError ? (
            <span role="alert" className="text-sm text-vermillion">
              {save.error.message}
            </span>
          ) : null}
        </div>
      </form>
    </Panel>
  )
}
