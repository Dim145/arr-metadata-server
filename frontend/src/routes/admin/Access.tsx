/**
 * Opening & APIs: the way in, on one page.
 *
 * Three questions, asked in the order a visitor meets them — may they come
 * in, may they sign up, may their software call — each answered by a choice
 * that says underneath what it changes. The sentence at the top is the page
 * read back: what an administrator would otherwise piece together from four
 * panels. Every answer is a setting, written the moment it is chosen; the
 * APIs' numbers are what they answered since the server started.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router'

import {
  Chip,
  Glyph,
  IconButton,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Segmented,
  Skeleton,
  Toggle,
} from '../../components/ui'
import { OidcSettings } from '../../components/account/OidcSettings'
import { ListenersPanel } from '../../components/ListenersPanel'
import { api } from '../../lib/api'
import { cn } from '../../lib/cn'
import { relative } from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import type { AccessReport, AccountRole, ApiName, Registration } from '../../lib/types'

const REGISTRATIONS: Registration[] = ['closed', 'invite', 'approval', 'open']

export function Access() {
  const { t, locale } = useI18n()
  const queryClient = useQueryClient()

  const report = useQuery({
    queryKey: ['admin', 'access'],
    queryFn: () => api.get<AccessReport>('/admin/access'),
    // The numbers move while the page is open; a glance every half-minute is
    // enough to see whether Sonarr still calls.
    refetchInterval: 30_000,
  })

  const write = useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) =>
      api.put('/settings/server/-', { key, value }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['admin', 'access'] })
      void queryClient.invalidateQueries({ queryKey: ['auth', 'options'] })
      void queryClient.invalidateQueries({ queryKey: ['settings'] })
      void queryClient.invalidateQueries({ queryKey: ['me'] })
    },
  })

  const a = t.admin.access

  if (report.isPending) {
    return (
      <div className="mx-auto max-w-5xl space-y-6">
        <Skeleton className="h-28 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (report.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {report.error.message}
      </p>
    )
  }

  const access = report.data
  const set = (key: string, value: string) => write.mutate({ key, value })
  const answering = access.apis.filter((x) => x.enabled).map((x) => a.apis[x.api])
  const nativePolicy = access.apis.find((x) => x.api === 'native')?.policy ?? 'apikey'
  const started = new Date(Date.now() - access.uptimeSeconds * 1000).toISOString()

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.admin.accessPage}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{a.lead}</p>
      </header>

      {access.authDisabled ? (
        <p
          role="alert"
          className="rise mb-6 flex items-start gap-3 rounded-panel border border-vermillion-deep bg-vermillion/[0.06] px-5 py-4 text-sm leading-relaxed text-bone"
        >
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-vermillion" />
          {a.authDisabled}
        </p>
      ) : null}

      {/* The page, read back in a sentence. */}
      <Panel className="rise mb-6 border-l-2 border-l-vermillion" style={{ animationDelay: '40ms' }}>
        <div className="px-6 py-5">
          <Label>{a.summaryLabel}</Label>
          <p className="mt-2 font-display text-xl leading-snug text-bone sm:text-2xl" aria-live="polite">
            {a.summary[access.site]}, {a.summary[access.registration]},{' '}
            {answering.length === access.apis.length ? a.summary.apisAll : a.summary.apisSome(answering)}, {a.summary.keys(access.keysPerUser)}.
          </p>
        </div>
      </Panel>

      <div className="space-y-6">
        {/* 1 · the site */}
        <Question number={1} title={a.q1} delay={80}>
          <Segmented
            label={a.q1}
            value={access.site}
            options={(['private', 'public'] as const).map((value) => ({ value, label: a.site[value] }))}
            onChange={(value) => set('site.access', value)}
            disabled={write.isPending || access.siteLocked}
          />
          <Consequence>{a.siteHint[access.site]}</Consequence>
          {access.siteLocked ? <Caution>{a.locked}</Caution> : null}
          {nativePolicy !== 'apikey' && !access.authDisabled ? <Caution>{a.nativeNote(nativePolicy)}</Caution> : null}
        </Question>

        {/* 2 · signing up */}
        <Question number={2} title={a.q2} note={a.q2Note} delay={120}>
          <Segmented
            label={a.q2}
            value={access.registration}
            options={REGISTRATIONS.map((value) => ({ value, label: a.registration[value] }))}
            onChange={(value) => set('registration.mode', value)}
            disabled={write.isPending}
            // Four choices do not fit a phone's width as a row: a column there.
            className="max-sm:flex max-sm:w-full max-sm:flex-col max-sm:rounded-card"
          />
          <Consequence>{a.registrationHint[access.registration]}</Consequence>

          <div className="mt-5 grid gap-5 border-t border-rule pt-5 sm:grid-cols-[1fr_auto] sm:items-center">
            <div>
              <p className="text-sm font-medium text-bone">{a.newRole}</p>
              <p className="mt-1 text-xs leading-relaxed text-bone-faint">{a.newRoleHint}</p>
            </div>
            <Segmented
              label={a.newRole}
              value={access.registrationRole}
              options={(['member', 'editor'] as AccountRole[]).map((value) => ({
                value,
                label: t.labels.roles[value],
              }))}
              onChange={(value) => set('registration.role', value)}
              // Only an account an administrator approves may be given more
              // than a member's rights.
              disabled={write.isPending || access.registration !== 'approval'}
            />
          </div>

          <ul className="mt-5 space-y-2 border-t border-rule pt-5 text-sm text-bone-dim">
            {access.pending > 0 ? (
              <li className="flex flex-wrap items-center gap-x-3">
                <Lamp tone="brass">{a.pending(access.pending)}</Lamp>
                <Link to="/admin/users" className="inline-flex min-h-11 items-center gap-1 text-vermillion hover:text-vermillion-bright">
                  {a.review}
                  <Glyph name="chevronRight" className="size-3.5" />
                </Link>
              </li>
            ) : null}
            <li className="flex flex-wrap items-center gap-x-3">
              <span>{a.invitations(access.invitations)}</span>
              <Link
                to="/admin/users#invitations"
                className="inline-flex min-h-11 items-center gap-1 text-vermillion hover:text-vermillion-bright"
              >
                {a.manage}
                <Glyph name="chevronRight" className="size-3.5" />
              </Link>
            </li>
          </ul>
        </Question>

        {/* 3 · the APIs */}
        <Question number={3} title={a.q3} note={a.q3Note} delay={160} flush>
          {/* A row per API: five columns from a tablet up; on a phone, the
              name with its switch first — the switch is what one came for —
              then where it answers, then how and how much. */}
          <ul aria-label={a.q3} className="divide-y divide-rule">
            <li
              aria-hidden
              className="hidden grid-cols-[minmax(0,1.1fr)_minmax(0,1.5fr)_10rem_8rem_3.5rem] items-center gap-4 px-5 py-2.5 md:grid"
            >
              <span className="label">{a.surface}</span>
              <span className="label">{a.address}</span>
              <span className="label">{a.policy}</span>
              <span className="label text-right">{a.calls}</span>
              <span className="label text-right">{a.on}</span>
            </li>
            {access.apis.map((x) => (
              <li
                key={x.api}
                className={cn(
                  'grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 px-5 py-3',
                  'md:grid-cols-[minmax(0,1.1fr)_minmax(0,1.5fr)_10rem_8rem_3.5rem]',
                )}
              >
                <div className={cn('min-w-0 max-md:col-start-1 max-md:row-start-1', !x.enabled && 'opacity-60')}>
                  <span className="block font-medium text-bone">{a.apis[x.api]}</span>
                  <span className="block text-xs text-bone-faint">{a.apiNotes[x.api]}</span>
                </div>
                <span
                  className={cn(
                    'min-w-0 font-mono text-xs break-words text-bone-dim max-md:col-span-2 max-md:row-start-2',
                    !x.enabled && 'opacity-60',
                  )}
                >
                  <span className="sr-only">{a.address}: </span>
                  {a.apiPaths[x.api]}
                </span>
                <span className="max-md:col-start-1 max-md:row-start-3">
                  <span className="sr-only">{a.policy}: </span>
                  <PolicyChip policy={x.policy} />
                </span>
                <span className="text-right font-mono text-xs whitespace-nowrap tabular-nums max-md:col-start-2 max-md:row-start-3">
                  <span className="sr-only">{a.calls}: </span>
                  <span className="block text-bone">{a.served(x.served)}</span>
                  <span className={x.refused ? 'block text-brass' : 'block text-bone-faint'}>{a.refused(x.refused)}</span>
                </span>
                <span className="justify-self-end max-md:col-start-2 max-md:row-start-1">
                  <Toggle
                    checked={x.enabled}
                    label={a.toggle(a.apis[x.api as ApiName])}
                    onChange={(next) => set(`api.${x.api}`, String(next))}
                    disabled={write.isPending}
                  />
                </span>
                {x.api === 'tmdb' ? (
                  // Who beyond the editors: the relay spends the operator's
                  // quota, and with sign-ups open a member is anybody.
                  <div className="col-span-full flex items-center justify-between gap-4 rounded-card border border-rule bg-ink px-4 py-1.5">
                    <div className="min-w-0">
                      <span className="block text-sm text-bone">{a.membersRelay}</span>
                      <span className="block text-xs leading-relaxed text-bone-faint">{a.membersRelayHint}</span>
                    </div>
                    <Toggle
                      checked={access.relayForMembers}
                      label={a.membersRelayToggle}
                      onChange={(next) => set('api.tmdbMembers', String(next))}
                      disabled={write.isPending || !x.enabled}
                    />
                  </div>
                ) : null}
              </li>
            ))}
          </ul>
          <div className="space-y-2 border-t border-rule px-5 py-4 text-xs leading-relaxed text-bone-faint">
            <p>{a.offNote}</p>
            <p>
              {a.policyNote} <span className="text-bone-dim">{a.rules(access.allowlistRules)}.</span>{' '}
              <Link to="/admin/clients" className="text-vermillion hover:text-vermillion-bright">
                {t.admin.clients}
              </Link>
            </p>
            <p>{a.uptime(relative(started, locale) ?? '')}</p>
          </div>
        </Question>

        {/* The clients' door: where Sonarr and Radarr come in, in TLS. */}
        <ListenersPanel delay={180} />

        {/* 4 · the identity provider */}
        <OidcSettings number={4} delay={200} />

        {/* Keys per member */}
        <Panel label={a.keysTitle} className="rise" style={{ animationDelay: '200ms' }}>
          <PanelHead title={a.keysTitle} />
          <div className="grid gap-5 p-5 sm:grid-cols-[1fr_auto] sm:items-center">
            <div>
              <p className="text-sm leading-relaxed text-bone-dim">{a.keysHint}</p>
              <p className="mt-2 font-mono text-xs text-bone-faint">{a.keysNow(access.serverKeys, access.personalKeys)}</p>
            </div>
            <div className="flex items-center gap-1 justify-self-start rounded-full border border-rule bg-ink p-1 sm:justify-self-end">
              <IconButton
                glyph="minus"
                label={a.fewer}
                disabled={write.isPending || access.keysPerUser <= 0}
                onClick={() => set('keys.maxPerUser', String(access.keysPerUser - 1))}
              />
              <output
                aria-live="polite"
                aria-label={a.keysTitle}
                className="min-w-10 text-center font-display text-2xl text-bone tabular-nums"
              >
                {access.keysPerUser}
              </output>
              <IconButton
                glyph="plus"
                label={a.more}
                disabled={write.isPending || access.keysPerUser >= 50}
                onClick={() => set('keys.maxPerUser', String(access.keysPerUser + 1))}
              />
            </div>
          </div>
        </Panel>

        {write.isError ? (
          <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {write.error.message}
          </p>
        ) : null}
      </div>
    </div>
  )
}

function Question({
  number,
  title,
  note,
  delay,
  flush = false,
  children,
}: {
  number: number
  title: string
  note?: string
  delay: number
  /** Content that runs to the panel's edges, a table. */
  flush?: boolean
  children: React.ReactNode
}) {
  return (
    <Panel label={title} className="rise" style={{ animationDelay: `${delay}ms` }}>
      <PanelHead
        title={
          <span>
            <span className="text-vermillion">{number}</span> · {title}
          </span>
        }
        action={note ? <span className="hidden text-xs text-bone-faint sm:inline">{note}</span> : undefined}
      />
      <div className={flush ? undefined : 'p-5'}>{children}</div>
    </Panel>
  )
}

/** What stands in the way of the choice above, said under it. */
function Caution({ children }: { children: React.ReactNode }) {
  return (
    <p className="mt-3 flex max-w-prose items-start gap-2 text-sm leading-relaxed text-brass">
      <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
      <span>{children}</span>
    </p>
  )
}

/** What the choice above changes, said under it. */
function Consequence({ children }: { children: React.ReactNode }) {
  return (
    <p className="mt-4 max-w-prose border-l-2 border-rule-bright pl-4 text-sm leading-relaxed text-bone-dim">
      {children}
    </p>
  )
}

function PolicyChip({ policy }: { policy: AccessReport['apis'][number]['policy'] }) {
  const { t } = useI18n()

  return (
    <Chip tone={policy === 'open' ? 'accent' : undefined} className="whitespace-nowrap">
      <Glyph name={policy === 'apikey' ? 'key' : policy === 'allowlist' ? 'shield' : 'alert'} className="size-3" />
      {t.admin.access.policies[policy]}
    </Chip>
  )
}
