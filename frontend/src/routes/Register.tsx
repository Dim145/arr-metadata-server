/**
 * Opening an account, where the server allows it.
 *
 * As plain as the sign-in page it sits beside. What it asks depends on how the
 * server takes sign-ups: an invitation code first when one is required, and
 * last, as an option, when it only spares the wait. An invitation link carries
 * its code after the `#`, which the browser never sends anywhere, and the code
 * is checked as soon as it is whole, so a used-up one is said before anything
 * else is typed.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Link, useLocation, useNavigate } from 'react-router'

import { Button, FormField, Glyph, Input, Spinner } from '../components/ui'
import { ApiError, api } from '../lib/api'
import { longDate } from '../lib/format'
import { useAuthOptions } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import type { InvitationOffer } from '../lib/types'

/** The code a link brought, after the `#` where the browser keeps it to itself. */
function codeFromAddress(): string {
  return (new URLSearchParams(window.location.hash.replace(/^#/, '')).get('invite') ?? '').trim()
}

/**
 * Whether a typed code has enough letters to be one: sixteen, once the spaces
 * and dashes between the groups are gone — every kind a keyboard or a mail
 * client puts there, as the server strips them.
 */
function whole(code: string) {
  return code.replace(/[\s\u002D\u2010-\u2015\u2212]/g, '').length === 16
}

const USERNAME = /^[A-Za-z0-9._-]{2,40}$/

export function Register() {
  const { t, locale } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const options = useAuthOptions()

  const [code, setCode] = useState(codeFromAddress)

  // A link opened while this page already is: only the part after the `#`
  // changes, and nothing mounts again to read it.
  const { hash } = useLocation()
  useEffect(() => {
    const brought = codeFromAddress()
    if (!brought) return
    setCode(brought)
    // Read, then taken out of the address: a code left in the history is one
    // anybody at this browser could use again.
    navigate('/register', { replace: true })
  }, [hash, navigate])
  const [username, setUsername] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [touched, setTouched] = useState({ username: false, password: false })
  const [pending, setPending] = useState<string | null>(null)

  const trimmed = code.trim()
  const offer = useQuery({
    queryKey: ['auth', 'invitation', trimmed.toUpperCase()],
    queryFn: () => api.post<InvitationOffer>('/auth/invitations/check', { code: trimmed }),
    enabled: whole(trimmed),
    retry: false,
    staleTime: 60_000,
  })

  const register = useMutation({
    mutationFn: () =>
      api.post<{ canWrite?: boolean; pending?: boolean; username: string }>('/auth/register', {
        username: username.trim(),
        password,
        displayName: displayName.trim() || undefined,
        email: email.trim() || undefined,
        code: trimmed || undefined,
      }),
    onSuccess: async (answer) => {
      if (answer.pending) {
        setPending(displayName.trim() || answer.username)
        return
      }
      // Signed in: nothing cached before was answered for this person.
      await queryClient.invalidateQueries()
      navigate(answer.canWrite ? '/admin' : '/')
    },
  })

  if (options.isPending) {
    return (
      <Door>
        <Spinner className="size-6 text-bone-dim" />
      </Door>
    )
  }

  const mode = options.data?.registration ?? 'closed'

  if (pending !== null) {
    return (
      <Door>
        <h1 className="font-display text-3xl font-medium text-bone">{t.register.pendingTitle}</h1>
        <p className="mt-3 text-sm leading-relaxed text-bone-dim">{t.register.pendingBody(pending)}</p>
        <SignInLink />
      </Door>
    )
  }

  if (mode === 'closed') {
    return (
      <Door>
        <h1 className="font-display text-3xl font-medium text-bone">{t.register.closedTitle}</h1>
        <p className="mt-3 text-sm leading-relaxed text-bone-dim">{t.register.closedBody}</p>
        <SignInLink />
      </Door>
    )
  }

  const required = mode === 'invite'
  const usernameWrong = touched.username && username.trim() !== '' && !USERNAME.test(username.trim())
  const passwordShort = touched.password && password !== '' && [...password].length < 12
  const codeState = !trimmed
    ? undefined
    : !whole(trimmed)
      ? undefined
      : offer.isPending
        ? 'checking'
        : offer.isSuccess
          ? 'valid'
          : 'invalid'

  const refusal = register.error instanceof ApiError ? register.error : undefined
  const refusalText = refusal
    ? ((t.register.errors as Record<string, string | undefined>)[refusal.code] ??
      (refusal.status === 400 ? refusal.message : t.common.error))
    : undefined

  const ready =
    USERNAME.test(username.trim()) &&
    [...password].length >= 12 &&
    (!required || codeState === 'valid') &&
    codeState !== 'invalid' &&
    codeState !== 'checking'

  const codeField = (
    <FormField
      label={required ? t.register.code : t.register.codeOptional}
      htmlFor="invite"
      hint={
        codeState === 'checking'
          ? t.register.checking
          : codeState === 'valid' && offer.data
            ? undefined
            : required
              ? t.register.codeHint
              : t.register.codeOptionalHint
      }
      error={codeState === 'invalid' ? t.register.invalid : undefined}
    >
      <Input
        id="invite"
        name="invite"
        autoComplete="off"
        spellCheck={false}
        required={required}
        autoFocus={required && !trimmed}
        placeholder="K7QM-2XRP-9DHT-4WCN"
        className="font-mono tracking-wider uppercase"
        value={code}
        onChange={(event) => setCode(event.target.value)}
      />
      {codeState === 'valid' && offer.data ? (
        <p className="flex items-center gap-1.5 text-xs text-moss" role="status">
          <Glyph name="check" className="size-3.5 shrink-0" />
          {t.register.valid(t.labels.roles[offer.data.role], longDate(offer.data.expiresAt, locale))}
        </p>
      ) : null}
    </FormField>
  )

  return (
    <Door>
      <h1 className="font-display text-3xl font-medium text-bone">{t.register.title}</h1>
      <p className="mt-2 mb-8 text-sm leading-relaxed text-bone-dim">{t.register.lead[mode]}</p>

      <form
        className="space-y-5"
        noValidate
        onSubmit={(event) => {
          event.preventDefault()
          setTouched({ username: true, password: true })
          if (ready) register.mutate()
        }}
      >
        {required ? codeField : null}

        <FormField
          label={t.register.username}
          htmlFor="username"
          hint={t.register.usernameHint}
          error={usernameWrong ? t.register.usernameHint : undefined}
        >
          <Input
            id="username"
            name="username"
            autoComplete="username"
            autoCapitalize="none"
            spellCheck={false}
            required
            autoFocus={!required || Boolean(trimmed)}
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            onBlur={() => setTouched((was) => ({ ...was, username: true }))}
          />
        </FormField>

        <FormField label={t.register.displayName} htmlFor="display-name" hint={t.register.displayNameHint}>
          <Input
            id="display-name"
            name="name"
            autoComplete="nickname"
            maxLength={80}
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
          />
        </FormField>

        <FormField label={t.register.email} htmlFor="email" hint={t.register.emailHint}>
          <Input
            id="email"
            name="email"
            type="email"
            autoComplete="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
        </FormField>

        <FormField
          label={t.register.password}
          htmlFor="password"
          hint={t.register.passwordHint}
          error={passwordShort ? t.register.passwordHint : undefined}
        >
          <Input
            id="password"
            name="password"
            type="password"
            autoComplete="new-password"
            required
            minLength={12}
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            onBlur={() => setTouched((was) => ({ ...was, password: true }))}
          />
        </FormField>

        {required ? null : codeField}

        {refusalText ? (
          <p role="alert" className="flex items-start gap-2 text-sm text-vermillion">
            <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
            {refusalText}
          </p>
        ) : null}

        <Button type="submit" variant="primary" className="w-full" disabled={register.isPending}>
          {register.isPending ? t.register.submitting : t.register.submit}
        </Button>
      </form>

      <SignInLink />
    </Door>
  )
}

/** The page's frame: the sign-in page's, word for word. */
function Door({ children }: { children: React.ReactNode }) {
  const { t } = useI18n()

  return (
    <div className="grain flex min-h-dvh items-center justify-center overflow-x-clip px-4 py-12">
      <div className="strike relative z-10 w-full max-w-sm">
        <a
          href="/"
          className="mb-10 flex items-baseline gap-2 text-bone transition-colors duration-200 hover:text-vermillion"
        >
          <span className="font-display text-2xl font-medium tracking-tight">{t.brand.name}</span>
          <span className="label">arr</span>
        </a>
        {children}
      </div>
    </div>
  )
}

function SignInLink() {
  const { t } = useI18n()

  return (
    <p className="mt-8 text-sm text-bone-faint">
      {t.register.haveAccount}{' '}
      <Link
        to="/login"
        className="inline-flex min-h-11 items-center text-vermillion transition-colors duration-150 hover:text-vermillion-bright"
      >
        {t.register.signIn}
      </Link>
    </p>
  )
}
