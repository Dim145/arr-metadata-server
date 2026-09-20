import { useMutation } from '@tanstack/react-query'
import { useState } from 'react'
import { useNavigate } from 'react-router'

import { ApiError, api } from '../lib/api'
import { Alert, Button, Input, Label, Lock, Spinner } from '../components/ui'

export function Login() {
  const navigate = useNavigate()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')

  const signIn = useMutation({
    mutationFn: () => api.post<{ username: string }>('/auth/login', { username, password }),
    onSuccess: () => navigate('/', { replace: true }),
  })

  return (
    <div className="grid min-h-dvh place-items-center px-5">
      {/* An amber wash from the left edge, like a monitor warming up. */}
      <div
        aria-hidden
        className="pointer-events-none fixed inset-0 bloom opacity-60"
      />

      <form
        className="reveal relative w-full max-w-sm"
        onSubmit={(event) => {
          event.preventDefault()
          signIn.mutate()
        }}
      >
        <div className="mb-8 flex items-center gap-2.5 text-phos">
          <Lock className="h-5 w-5" />
          <h1 className="font-display text-3xl leading-none tracking-tight text-paper">
            metadata
          </h1>
        </div>

        <p className="mb-8 max-w-[34ch] text-[13px] leading-relaxed text-faint">
          Sign in to curate the catalogue. Locked fields stay yours — no refresh
          will overwrite them.
        </p>

        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label>Username</Label>
            <Input
              name="username"
              autoComplete="username"
              autoFocus
              required
              value={username}
              onChange={(event) => setUsername(event.target.value)}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label>Password</Label>
            <Input
              name="password"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </div>

          {signIn.isError && (
            <Alert>
              {signIn.error instanceof ApiError && signIn.error.isUnauthorized
                ? 'Those credentials were not accepted.'
                : 'Could not reach the server.'}
            </Alert>
          )}

          <Button type="submit" variant="primary" disabled={signIn.isPending} className="mt-2 justify-center py-2.5">
            {signIn.isPending ? <Spinner className="border-void/40 border-t-void" /> : 'Sign in'}
          </Button>
        </div>

        <p className="mt-10 border-t border-line pt-4 font-mono text-[10px] leading-relaxed tracking-[0.1em] text-faint">
          No account? Set AMS_ADMIN_USERNAME and AMS_ADMIN_PASSWORD, then restart
          the server.
        </p>
      </form>
    </div>
  )
}
