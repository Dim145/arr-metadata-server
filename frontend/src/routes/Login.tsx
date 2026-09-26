/**
 * The door to the administration side.
 *
 * Deliberately plain. Everything else in this interface is trying to show you a
 * catalogue; this page has one job, and dressing it up would only slow down the
 * person who came here to do something else.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useNavigate } from 'react-router'

import { Button, FormField, Glyph, Input } from '../components/ui'
import { ApiError, api } from '../lib/api'
import { useI18n } from '../lib/i18n'

export function Login() {
  const { t } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

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
    code === 'account_pending' ? t.auth.pending : code === 'account_disabled' ? t.auth.disabled : undefined
  const wrongCredentials =
    signIn.error instanceof ApiError && (signIn.error.isUnauthorized || signIn.error.status === 403)

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
        <p className="mt-2 mb-8 text-sm leading-relaxed text-bone-dim">{t.auth.lead}</p>

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
              autoFocus
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

        <a
          href="/"
          className="mt-8 inline-flex items-center gap-2 text-sm text-bone-faint transition-colors duration-200 hover:text-bone"
        >
          <Glyph name="arrowLeft" className="size-4" />
          {t.work.back}
        </a>
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
