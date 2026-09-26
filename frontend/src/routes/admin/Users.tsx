/**
 * The members: every account, what it may do, and when it last came by.
 *
 * A ledger rather than a grid of cards: one line each, the name first, the
 * role in its colour, the status as a lamp with its word. Accounts waiting
 * for approval come first, whatever the filters, since they are the ones an
 * administrator has come here for. Several can be changed at once; each is
 * checked on its own, so the last administrator or your own account in a
 * selection is left as it was and said, rather than failing the rest.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useDeferredValue, useState } from 'react'
import { Link, useNavigate } from 'react-router'

import { Invitations } from '../../components/account/Invitations'
import { Initial, RoleChip, StatusLamp } from '../../components/account/people'
import { SecretReveal } from '../../components/account/SecretReveal'
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
  Segmented,
  Select,
  Skeleton,
  Spinner,
  TableScroll,
  Td,
  Th,
  Tr,
} from '../../components/ui'
import { api, query } from '../../lib/api'
import { dateTime } from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { AccountRole, AccountStatus, ListedUser, Me, User, UsersPage } from '../../lib/types'

type RoleFilter = 'all' | AccountRole
type StatusFilter = 'all' | AccountStatus

const ROLES: AccountRole[] = ['member', 'editor', 'admin']

export function Users() {
  const { t, locale } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const [term, setTerm] = useState('')
  const search = useDeferredValue(term.trim())
  const [role, setRole] = useState<RoleFilter>('all')
  const [status, setStatus] = useState<StatusFilter>('all')
  const [selected, setSelected] = useState<string[]>([])
  const [creating, setCreating] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [refused, setRefused] = useState<{ id: string; reason: string }[]>([])

  const me = useQuery({ queryKey: ['me'], queryFn: () => api.get<Me>('/auth/me'), staleTime: 5 * 60_000 })
  const page = useQuery({
    queryKey: ['users', search, role, status],
    queryFn: () => api.get<UsersPage>(`/users${query({ term: search, role, status, limit: 200 })}`),
    placeholderData: (previous) => previous,
  })

  const bulk = useMutation({
    mutationFn: (body: { action: 'role' | 'status' | 'delete'; value?: string }) =>
      api.post<{ done: string[]; refused: { id: string; reason: string }[] }>('/users/bulk', {
        ids: selected,
        ...body,
      }),
    onSuccess: (outcome) => {
      setRefused(outcome.refused)
      setSelected([])
      setDeleting(false)
      void queryClient.invalidateQueries({ queryKey: ['users'] })
    },
  })

  const users = page.data?.users ?? []
  const counts = page.data?.counts
  const allShown = users.length > 0 && users.every((u) => selected.includes(u.id))
  const toggle = (id: string) =>
    setSelected((was) => (was.includes(id) ? was.filter((x) => x !== id) : [...was, id]))
  const nameOf = (id: string) => users.find((u) => u.id === id)?.username ?? id

  return (
    <div className="mx-auto max-w-6xl">
      <header className="rise mb-8 flex flex-wrap items-end justify-between gap-4">
        <div>
          <Label>{t.admin.title}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.users}</h1>
          <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.admin.people.lead}</p>
        </div>
        <Button variant="primary" onClick={() => setCreating(true)}>
          <Glyph name="plus" className="size-4" />
          {t.admin.people.create}
        </Button>
      </header>

      <div className="rise mb-6 grid gap-3 sm:grid-cols-3" style={{ animationDelay: '40ms' }}>
        <Figure label={t.admin.people.accounts} value={counts?.total} note={counts ? t.admin.people.viaOidc(counts.oidc) : undefined} />
        <Figure
          label={t.admin.people.pending}
          value={counts?.pending}
          note={counts ? t.admin.people.pendingHint(counts.pending) : undefined}
          tone={counts?.pending ? 'brass' : undefined}
        />
        <Figure label={t.admin.people.admins} value={counts?.admins} />
      </div>

      <div className="rise mb-4 flex flex-wrap items-center gap-3" style={{ animationDelay: '80ms' }}>
        <div className="relative min-w-60 flex-1 sm:max-w-sm">
          <Glyph
            name="search"
            className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-bone-faint"
          />
          <Input
            type="search"
            aria-label={t.admin.people.search}
            placeholder={t.admin.people.search}
            value={term}
            onChange={(event) => setTerm(event.target.value)}
            className="pl-9"
          />
        </div>
        <Filter<RoleFilter>
          label={t.admin.people.role}
          value={role}
          onChange={setRole}
          options={[
            { value: 'all', label: t.admin.people.allRoles },
            ...ROLES.map((r) => ({ value: r, label: t.labels.roles[r] })),
          ]}
        />
        <Filter<StatusFilter>
          label={t.admin.people.status}
          value={status}
          onChange={setStatus}
          options={[
            { value: 'all', label: t.admin.people.allStatuses },
            { value: 'active', label: t.labels.statuses.active },
            { value: 'pending', label: t.labels.statuses.pending },
            { value: 'disabled', label: t.labels.statuses.disabled },
          ]}
        />
      </div>

      {selected.length ? (
        <div
          role="region"
          aria-label={t.admin.people.selected(selected.length)}
          className="fade-in mb-4 flex flex-wrap items-center gap-2 rounded-full border border-vermillion-deep bg-vermillion/[0.07] px-4 py-2"
        >
          <span className="mr-1 text-sm font-medium text-bone">{t.admin.people.selected(selected.length)}</span>
          <Select
            aria-label={t.admin.people.setRole}
            value=""
            onChange={(event) => {
              if (event.target.value) bulk.mutate({ action: 'role', value: event.target.value })
            }}
            className="min-h-9 w-auto py-0 text-[0.8125rem]"
          >
            <option value="">{t.admin.people.setRole}</option>
            {ROLES.map((r) => (
              <option key={r} value={r}>
                {t.labels.roles[r]}
              </option>
            ))}
          </Select>
          <Button size="sm" onClick={() => bulk.mutate({ action: 'status', value: 'active' })}>
            {t.admin.people.approve}
          </Button>
          <Button size="sm" onClick={() => bulk.mutate({ action: 'status', value: 'disabled' })}>
            {t.admin.people.disable}
          </Button>
          <Button size="sm" variant="quiet" onClick={() => setSelected([])}>
            {t.admin.people.clear}
          </Button>
          <Button size="sm" variant="danger" className="ml-auto" onClick={() => setDeleting(true)}>
            {t.admin.people.delete}
          </Button>
        </div>
      ) : null}

      {refused.length ? (
        <div role="status" className="fade-in mb-4 rounded-panel border border-brass-deep bg-brass/[0.06] p-4 text-sm">
          <p className="text-bone">{t.admin.people.refused(refused.length)}</p>
          <ul className="mt-1 space-y-0.5 text-bone-dim">
            {refused.map((r) => (
              <li key={r.id}>
                <span className="font-mono text-bone">{nameOf(r.id)}</span> — {r.reason}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <Panel label={t.admin.users} className="rise" style={{ animationDelay: '120ms' }}>
        {page.isPending ? (
          <div className="space-y-2 p-5">
            {Array.from({ length: 4 }, (_, i) => (
              <Skeleton key={i} className="h-12 w-full" />
            ))}
          </div>
        ) : users.length === 0 ? (
          <EmptyState title={t.admin.people.empty} hint={t.admin.people.emptyHint} />
        ) : (
          <TableScroll label={t.admin.users}>
            <table className="w-full min-w-[46rem] border-collapse text-left">
              <thead>
                <tr>
                  <Th className="w-10">
                    <input
                      type="checkbox"
                      aria-label={t.admin.people.selectAll}
                      checked={allShown}
                      onChange={() => setSelected(allShown ? [] : users.map((u) => u.id))}
                      className="size-4 cursor-pointer accent-vermillion"
                    />
                  </Th>
                  <Th>{t.admin.people.person}</Th>
                  <Th>{t.admin.people.role}</Th>
                  <Th>{t.admin.people.status}</Th>
                  <Th>{t.admin.people.signsIn}</Th>
                  <Th>{t.admin.people.lastSeen}</Th>
                  <Th align="right">{t.admin.people.keys}</Th>
                </tr>
              </thead>
              <tbody>
                {users.map((user) => (
                  <Row
                    key={user.id}
                    user={user}
                    you={me.data?.user?.id === user.id}
                    selected={selected.includes(user.id)}
                    onToggle={() => toggle(user.id)}
                    onOpen={() => navigate(`/admin/users/${user.id}`)}
                    locale={locale}
                  />
                ))}
              </tbody>
            </table>
          </TableScroll>
        )}
      </Panel>

      <Invitations />

      <CreateAccount open={creating} onClose={() => setCreating(false)} />

      <Dialog
        open={deleting}
        title={t.admin.people.deleteTitle(selected.length)}
        onClose={() => setDeleting(false)}
        footer={
          <>
            <Button onClick={() => setDeleting(false)}>{t.account.cancel}</Button>
            <Button variant="danger" disabled={bulk.isPending} onClick={() => bulk.mutate({ action: 'delete' })}>
              {bulk.isPending ? <Spinner className="size-4" /> : null}
              {t.admin.people.deleteConfirm}
            </Button>
          </>
        }
      >
        {t.admin.people.deleteBody}
      </Dialog>
    </div>
  )
}

/**
 * One filter: a row of choices where there is room for it, a menu where there
 * is not — four choices in a pill wrap into a shape nobody reads on a phone.
 */
function Filter<T extends string>({
  label,
  value,
  onChange,
  options,
}: {
  label: string
  value: T
  onChange: (next: T) => void
  options: { value: T; label: string }[]
}) {
  return (
    <>
      <Segmented<T> label={label} value={value} onChange={onChange} options={options} className="hidden md:inline-flex" />
      <Select
        aria-label={label}
        value={value}
        onChange={(event) => onChange(event.target.value as T)}
        className="w-auto md:hidden"
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </Select>
    </>
  )
}

function Figure({
  label,
  value,
  note,
  tone,
}: {
  label: string
  value: number | undefined
  note?: string
  tone?: 'brass'
}) {
  const { locale } = useI18n()

  return (
    <div className="rounded-panel border border-rule bg-ink-raised p-4">
      <span className="label">{label}</span>
      <p className="mt-2 flex items-baseline gap-2">
        <span className={tone === 'brass' ? 'font-display text-3xl text-brass' : 'font-display text-3xl text-bone'}>
          {value === undefined ? '—' : value.toLocaleString(locale)}
        </span>
        {note ? <span className="text-sm text-bone-dim">{note}</span> : null}
      </p>
    </div>
  )
}

function Row({
  user,
  you,
  selected,
  onToggle,
  onOpen,
  locale,
}: {
  user: ListedUser
  you: boolean
  selected: boolean
  onToggle: () => void
  onOpen: () => void
  locale: string
}) {
  const { t } = useI18n()
  const name = user.displayName || user.username

  return (
    <Tr className={selected ? 'bg-vermillion/[0.05]' : undefined}>
      <Td>
        <input
          type="checkbox"
          aria-label={t.admin.people.select(name)}
          checked={selected}
          onChange={onToggle}
          className="size-4 cursor-pointer accent-vermillion"
        />
      </Td>
      <Td>
        <div className="flex items-center gap-3">
          <Initial name={name} />
          <div className="min-w-0">
            <Link
              to={`/admin/users/${user.id}`}
              onClick={(event) => {
                event.preventDefault()
                onOpen()
              }}
              className="text-sm font-medium text-bone transition-colors duration-150 hover:text-vermillion"
            >
              {name}
            </Link>
            {you ? <span className="ml-1.5 text-xs text-bone-faint">({t.admin.people.you})</span> : null}
            <p className="truncate font-mono text-xs text-bone-faint">
              {[user.username, user.email].filter(Boolean).join(' · ')}
            </p>
          </div>
        </div>
      </Td>
      <Td>
        <RoleChip role={user.role} />
      </Td>
      <Td>
        <StatusLamp status={user.status} />
      </Td>
      <Td>
        <Chip tone={user.oidcLinked ? 'provider' : 'neutral'}>
          {user.oidcLinked ? t.admin.people.withOidc : t.admin.people.withPassword}
        </Chip>
      </Td>
      <Td className="font-mono text-xs text-bone-dim">
        {user.lastLoginAt ? dateTime(user.lastLoginAt, locale) : t.admin.people.never}
      </Td>
      <Td className="text-right font-mono text-sm tabular-nums">{user.keys}</Td>
    </Tr>
  )
}

/** Opening an account by hand: a password given, or one generated to hand over. */
function CreateAccount({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const [username, setUsername] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [email, setEmail] = useState('')
  const [role, setRole] = useState<AccountRole>('member')
  const [password, setPassword] = useState('')

  const create = useMutation({
    mutationFn: () =>
      api.post<{ user: User; password?: string }>('/users', {
        username,
        displayName,
        email,
        role,
        password: password || undefined,
      }),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['users'] }),
  })

  const close = () => {
    setUsername('')
    setDisplayName('')
    setEmail('')
    setRole('member')
    setPassword('')
    create.reset()
    onClose()
  }

  return (
    <Dialog
      open={open}
      title={t.admin.people.create}
      onClose={close}
      footer={
        create.isSuccess ? (
          <>
            <Link
              to={`/admin/users/${create.data.user.id}`}
              className="inline-flex min-h-11 items-center gap-1.5 px-3 text-sm text-vermillion hover:text-vermillion-bright"
            >
              {create.data.user.username}
              <Glyph name="chevronRight" className="size-3.5" />
            </Link>
            <Button onClick={close}>{t.account.dismiss}</Button>
          </>
        ) : (
          <>
            <Button onClick={close}>{t.account.cancel}</Button>
            <Button
              type="submit"
              form="create-account"
              variant="primary"
              disabled={!username.trim() || create.isPending || (password !== '' && password.length < 12)}
            >
              {create.isPending ? <Spinner className="size-4" /> : null}
              {t.admin.people.create}
            </Button>
          </>
        )
      }
    >
      {create.isSuccess ? (
        <div className="space-y-4">
          <p className="text-bone">{t.admin.people.opened(create.data.user.displayName || create.data.user.username)}</p>
          {create.data.password ? (
            <SecretReveal title={t.admin.people.generated} secret={create.data.password} />
          ) : null}
        </div>
      ) : (
        <form
          id="create-account"
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault()
            create.mutate()
          }}
        >
          <p className="text-xs leading-relaxed text-bone-faint">{t.admin.people.createLead}</p>
          <FormField
            label={t.account.username}
            htmlFor="new-username"
            error={create.isError ? create.error.message : undefined}
          >
            <Input
              id="new-username"
              required
              autoComplete="off"
              maxLength={40}
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              className="font-mono"
            />
          </FormField>
          <FormField label={t.account.displayName} htmlFor="new-display-name">
            <Input id="new-display-name" maxLength={80} value={displayName} onChange={(event) => setDisplayName(event.target.value)} />
          </FormField>
          <FormField label={t.account.email} htmlFor="new-email" hint={t.account.emailHint}>
            <Input id="new-email" type="email" value={email} onChange={(event) => setEmail(event.target.value)} />
          </FormField>
          <div className="space-y-1.5">
            <span className="label block">{t.admin.people.role}</span>
            <Segmented<AccountRole>
              label={t.admin.people.role}
              value={role}
              onChange={setRole}
              options={ROLES.map((r) => ({ value: r, label: t.labels.roles[r] }))}
            />
          </div>
          <FormField label={t.admin.people.passwordOptional} htmlFor="new-password-admin" hint={t.admin.people.passwordGenerate}>
            <Input
              id="new-password-admin"
              type="password"
              autoComplete="new-password"
              minLength={12}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </FormField>
        </form>
      )}
    </Dialog>
  )
}
