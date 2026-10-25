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
import { useEffect, useRef, useState } from 'react'
import { Link, useLocation, useNavigate } from 'react-router'

import { Button, FormField, Glyph, Input, Spinner } from '../components/ui'
import { ApiError, api } from '../lib/api'
import { longDate } from '../lib/format'
import { useAuthOptions } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import type { InvitationOffer } from '../lib/types'
import { WRONG, firstWrong, useDone, useValidity } from './fields'

const FIELDS = ['invite', 'username', 'email', 'password'] as const

/**
 * An address as the browser's own check reads one: a name, an `@`, and a
 * domain of dot-separated labels — the pattern the HTML standard gives, so
 * the words under the field and the browser agree.
 */
const EMAIL =
  /^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/

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
  const done = useDone(FIELDS)
  const fields = useRef<HTMLFormElement>(null)
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

  const mode = options.data?.registration ?? 'closed'
  const required = mode === 'invite'
  const codeState = !trimmed
    ? undefined
    : !whole(trimmed)
      ? undefined
      : offer.isPending
        ? 'checking'
        : offer.isSuccess
          ? 'valid'
          : 'invalid'

  // What is wrong with each field, whether or not it is said yet: kept as the
  // field's own validity, and said under it once the reader is done with it.
  const wrong = {
    invite: required && !trimmed ? t.register.codeMissing : codeState === 'invalid' ? t.register.invalid : undefined,
    username: !username.trim()
      ? t.auth.usernameMissing
      : USERNAME.test(username.trim())
        ? undefined
        : t.register.usernameHint,
    email: email.trim() && !EMAIL.test(email.trim()) ? t.register.emailWrong : undefined,
    password: !password ? t.auth.passwordMissing : [...password].length < 12 ? t.register.passwordHint : undefined,
  }
  useValidity(fields, wrong)
  // A code found unusable is said at once, whoever typed it: the link that
  // brought it was the reader's doing too.
  const said = (key: (typeof FIELDS)[number]) =>
    done.done(key) || (key === 'invite' && codeState === 'invalid') ? wrong[key] : undefined

  if (options.isPending) {
    return (
      <Door>
        <Spinner className="size-6 text-bone-dim" />
      </Door>
    )
  }

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

  const refusal = register.error instanceof ApiError ? register.error : undefined
  const refusalText = refusal
    ? ((t.register.errors as Record<string, string | undefined>)[refusal.code] ??
      (refusal.status === 400 ? refusal.message : t.common.error))
    : undefined

  /** What a field says it needs, and what it says is wrong once it may, each by an id the field points at. */
  const words = (key: (typeof FIELDS)[number], hint: string | undefined) => {
    const error = said(key)
    return {
      hint: hint ? <span id={`${key}-hint`}>{hint}</span> : undefined,
      error: error ? <span id={`${key}-error`}>{error}</span> : undefined,
      field: {
        'aria-invalid': error ? true : undefined,
        'aria-describedby': error ? `${key}-error` : hint ? `${key}-hint` : undefined,
        onBlur: () => done.left(key),
        className: WRONG,
      },
    }
  }

  const invite = words(
    'invite',
    codeState === 'checking'
      ? t.register.checking
      : codeState === 'valid' && offer.data
        ? undefined
        : required
          ? t.register.codeHint
          : t.register.codeOptionalHint,
  )
  const name = words('username', t.register.usernameHint)
  const address = words('email', t.register.emailHint)
  const secret = words('password', t.register.passwordHint)

  const codeField = (
    <FormField
      label={required ? t.register.code : t.register.codeOptional}
      htmlFor="invite"
      hint={invite.hint}
      error={invite.error}
    >
      <Input
        id="invite"
        name="invite"
        autoComplete="off"
        spellCheck={false}
        required={required}
        autoFocus={required && !trimmed}
        placeholder="K7QM-2XRP-9DHT-4WCN"
        {...invite.field}
        className={`${WRONG} font-mono tracking-wider uppercase`}
        value={code}
        onChange={(event) => {
          setCode(event.target.value)
          done.typed('invite')
        }}
      />
      {codeState === 'valid' && offer.data ? (
        <p className="flex items-center gap-1.5 text-xs text-status-ok" role="status">
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

      {/* The browser's own checks are not asked for: their bubble speaks the
          browser's language and goes as soon as it came. Each field says what
          is wrong under it, once the reader is done with it, and on sending
          the first wrong one takes the focus — as the browser's would have. */}
      <form
        ref={fields}
        className="space-y-5"
        noValidate
        onSubmit={(event) => {
          event.preventDefault()
          done.sent()
          const first = firstWrong(event.currentTarget)
          if (first) {
            first.focus()
            return
          }
          // A code still being checked is waited for: sent now, the answer
          // would be whatever the server makes of it.
          if (codeState !== 'checking') register.mutate()
        }}
      >
        {required ? codeField : null}

        <FormField label={t.register.username} htmlFor="username" hint={name.hint} error={name.error}>
          <Input
            id="username"
            name="username"
            autoComplete="username"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            enterKeyHint="next"
            required
            autoFocus={!required || Boolean(trimmed)}
            {...name.field}
            value={username}
            onChange={(event) => {
              setUsername(event.target.value)
              done.typed('username')
            }}
          />
        </FormField>

        <FormField label={t.register.displayName} htmlFor="display-name" hint={t.register.displayNameHint}>
          <Input
            id="display-name"
            name="name"
            autoComplete="nickname"
            enterKeyHint="next"
            maxLength={80}
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
          />
        </FormField>

        <FormField label={t.register.email} htmlFor="email" hint={address.hint} error={address.error}>
          <Input
            id="email"
            name="email"
            type="email"
            autoComplete="email"
            enterKeyHint="next"
            {...address.field}
            value={email}
            onChange={(event) => {
              setEmail(event.target.value)
              done.typed('email')
            }}
          />
        </FormField>

        {/* `new-password`, the field's id as its autocomplete: a password
            manager offers one it makes up, and never the one it keeps. */}
        <FormField label={t.register.password} htmlFor="new-password" hint={secret.hint} error={secret.error}>
          <Input
            id="new-password"
            name="password"
            type="password"
            autoComplete="new-password"
            enterKeyHint="done"
            required
            minLength={12}
            {...secret.field}
            value={password}
            onChange={(event) => {
              setPassword(event.target.value)
              done.typed('password')
            }}
          />
        </FormField>

        {required ? null : codeField}

        {refusalText ? (
          <p role="alert" className="flex items-start gap-2 text-sm text-status-error">
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
