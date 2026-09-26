/**
 * The door to the administration side.
 *
 * Deliberately plain. Everything else in this interface is trying to show you a
 * catalogue; this page has one job, and dressing it up would only slow down the
 * person who came here to do something else.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useNavigate } from 'react-router'

import { Button, ButtonLink, FormField, Glyph, Input } from '../components/ui'
import { ApiError, api } from '../lib/api'
import { useAuthOptions } from '../lib/hooks'
import { useI18n } from '../lib/i18n'

export function Login() {
  const { t } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const options = useAuthOptions()
  const closedSite = options.data?.site === 'private'
  const registration = options.data?.registration ?? 'closed'
  const provider = options.data?.oidc
  const passwords = options.data?.passwordLogin ?? true

  // Back from the identity provider with a refusal, said in a word.
  const params = new URLSearchParams(window.location.search)
  const sso = params.get('sso')
  // Own keys only: `?sso=constructor` is not a message.
  const ssoRefusal = sso
    ? Object.hasOwn(t.auth.ssoErrors, sso)
      ? (t.auth.ssoErrors as Record<string, string>)[sso]
      : t.common.error
    : undefined
  const startSso = `/api/v1/auth/oidc/start?next=${encodeURIComponent(sameSite(params.get('next')) ?? '/')}`

  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')

  const signIn = useMutation({
    mutationFn: () => api.post<{ canWrite: boolean }>('/auth/login', { username, password }),
    onSuccess: async (answer) => {
      // The whole interface keys off who you are; nothing cached before the
      // sign-in was answered for this person.
      await queryClient.invalidateQueries()
      // Where each belongs: the catalogue's maintainers to the administration,
      // a member back to the catalogue — or to the page they were sent from.
      navigate(sameSite(new URLSearchParams(window.location.search).get('next')) ?? (answer.canWrite ? '/admin' : '/'))
    },
  })

  const failed = signIn.isError
  const code = signIn.error instanceof ApiError ? signIn.error.code : undefined
  const refusal =
    code === 'account_pending'
      ? t.auth.pending
      : code === 'account_disabled'
        ? t.auth.disabled
        : code === 'password_login_off'
          ? t.auth.ssoErrors.password_login_off
          : undefined
  const wrongCredentials =
    signIn.error instanceof ApiError && (signIn.error.isUnauthorized || signIn.error.status === 403)

  const form = (
    <form
      className="space-y-5"
      onSubmit={(event) => {
        event.preventDefault()
        signIn.mutate()
      }}
    >
      <FormField label={t.auth.username} htmlFor="username">
        <Input
          id="username"
          name="username"
          autoComplete="username"
          required
          autoFocus={passwords && !provider}
          value={username}
          onChange={(event) => setUsername(event.target.value)}
        />
      </FormField>

      <FormField
        label={t.auth.password}
        htmlFor="password"
        error={failed ? (refusal ?? (wrongCredentials ? t.auth.failed : t.common.error)) : undefined}
      >
        <Input
          id="password"
          name="password"
          type="password"
          autoComplete="current-password"
          required
          value={password}
          onChange={(event) => setPassword(event.target.value)}
        />
      </FormField>

      <Button
        type="submit"
        variant="primary"
        className="w-full"
        disabled={signIn.isPending || !username || !password}
      >
        {signIn.isPending ? t.auth.signingIn : t.auth.submit}
      </Button>
    </form>
  )

  return (
    <div className="grain flex min-h-dvh items-center justify-center overflow-x-clip px-4">
      <div className="strike relative z-10 w-full max-w-sm">
        <a
          href="/"
          className="mb-10 flex items-baseline gap-2 text-bone transition-colors duration-200 hover:text-vermillion"
        >
          <span className="font-display text-2xl font-medium tracking-tight">{t.brand.name}</span>
          <span className="label">arr</span>
        </a>

        <h1 className="font-display text-3xl font-medium text-bone">{t.auth.signIn}</h1>
        <p className="mt-2 mb-8 text-sm leading-relaxed text-bone-dim">
          {closedSite ? t.auth.privateLead : t.auth.lead}
        </p>

        {ssoRefusal ? (
          <p
            role="alert"
            className="mb-6 flex items-start gap-2 rounded-card border border-vermillion-deep bg-vermillion/[0.06] px-4 py-3 text-sm text-bone"
          >
            <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-vermillion" />
            {ssoRefusal}
          </p>
        ) : null}

        {provider ? (
          <ButtonLink href={startSso} variant={passwords ? 'ghost' : 'primary'} className="w-full">
            <Glyph name="external" className="size-4" />
            {provider.label || t.auth.sso}
          </ButtonLink>
        ) : null}

        {provider && passwords ? (
          <div className="my-6 flex items-center gap-3 font-mono text-[0.6875rem] tracking-[0.16em] text-bone-faint uppercase">
            <span aria-hidden className="h-px flex-1 bg-rule" />
            {t.auth.or}
            <span aria-hidden className="h-px flex-1 bg-rule" />
          </div>
        ) : null}

        {passwords ? (
          form
        ) : (
          // Passwords off: the door the environment's administrator keeps, out
          // of everybody else's way.
          <details className="mt-8 rounded-card border border-rule px-4 py-3">
            <summary className="flex min-h-11 cursor-pointer items-center text-sm text-bone-dim">
              {t.auth.adminPassword}
            </summary>
            <p className="mt-2 mb-4 text-xs leading-relaxed text-bone-faint">{t.auth.passwordOff}</p>
            {form}
          </details>
        )}

        {registration === 'closed' ? null : (
          <p className="mt-8 text-sm text-bone-faint">
            {t.auth.noAccount}{' '}
            <Link
              to="/register"
              className="inline-flex min-h-11 items-center text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
            >
              {registration === 'invite'
                ? t.auth.withInvitation
                : registration === 'approval'
                  ? t.auth.askForOne
                  : t.auth.createOne}
            </Link>
          </p>
        )}

        {/* Nothing to go back to on a private site: the catalogue is behind this door. */}
        {closedSite ? null : (
          <a
            href="/"
            className="mt-6 inline-flex items-center gap-2 text-sm text-bone-faint transition-colors duration-200 hover:text-bone"
          >
            <Glyph name="arrowLeft" className="size-4" />
            {t.work.back}
          </a>
        )}
      </div>
    </div>
  )
}

/**
 * A page of this site to go back to, or nothing. Resolved the way the browser
 * would resolve it, then compared by origin: `/\evil.example` and a tab after
 * the slash both look like paths and both leave the site.
 */
function sameSite(next: string | null): string | undefined {
  if (!next?.startsWith('/')) return undefined
  try {
    const url = new URL(next, window.location.origin)
    return url.origin === window.location.origin ? `${url.pathname}${url.search}${url.hash}` : undefined
  } catch {
    return undefined
  }
}
