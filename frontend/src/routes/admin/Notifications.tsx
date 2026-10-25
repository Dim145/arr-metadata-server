/**
 * Where what happens is told.
 *
 * A destination is an address and the way it is posted to — signed as
 * Standard Webhooks says, a line for Discord or Slack, a notification for
 * ntfy, Gotify or Apprise, or the JSON the single webhook of earlier
 * versions was sent — with the events ticked for it. The page turns on two
 * moments. The one time a signing secret is on screen gets the accent and
 * waits to be dismissed, as a new key does: there is no second look. And
 * the log is where an operator finds out why a channel went quiet, and
 * sends again what it missed.
 *
 * An address is never shown whole, here or anywhere: for most services it
 * carries the token. The list shows its host, and a form left empty keeps
 * it as it is.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useMemo, useState } from 'react'

import { EVERY, EventPicker } from '../../components/EventPicker'
import {
  Button,
  Chip,
  DataTable,
  Dialog,
  EmptyState,
  FormField,
  Glyph,
  IconButton,
  Input,
  Label,
  Lamp,
  Panel,
  PanelHead,
  Select,
  Skeleton,
  Spinner,
  TableOptions,
  TBody,
  Td,
  Th,
  THead,
  Toggle,
  Tr,
  useDataTable,
  type Column,
} from '../../components/ui'
import { ApiError, api, query, segment } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n, type Dict } from '../../lib/i18n'
import type {
  WebhookDeliveries,
  WebhookDelivery,
  WebhookDestination,
  WebhookDestinationInput,
  WebhookEvents,
  WebhookKind,
  WebhookTest,
} from '../../lib/types'

type Words = Dict['admin']['notifications']

/** The destinations, as every part of the page reads them: one query, shared. */
const DESTINATIONS = ['notifications'] as const

/** A page of a log. */
const PAGE = 25

/** The tries a delivery gets, as the server counts them. */
const MOST_ATTEMPTS = 8

/** The kinds that keep a token their service gave. */
const TOKEN_KINDS: WebhookKind[] = ['ntfy', 'gotify']

/** The kinds people read — a chat, a phone — told of failed refreshes together by default. */
const READ_BY_PEOPLE: WebhookKind[] = ['discord', 'slack', 'ntfy', 'gotify', 'apprise']

/**
 * The origin an address is at — its scheme, host and port — or null for what
 * is no address. The address a destination is listed with is masked after
 * its host, which keeps its origin readable.
 */
function originOf(address: string): string | null {
  try {
    const origin = new URL(address.trim()).origin
    return origin === 'null' ? null : origin
  } catch {
    return null
  }
}

export function Notifications() {
  const { t } = useI18n()
  const n = t.admin.notifications
  const queryClient = useQueryClient()

  const destinations = useQuery({
    queryKey: [...DESTINATIONS],
    queryFn: () => api.get<WebhookDestination[]>('/admin/notifications'),
    // The counts move as the worker sends.
    refetchInterval: 30_000,
  })
  const known = useQuery({
    queryKey: ['webhooks', 'events'],
    queryFn: () => api.get<WebhookEvents>('/admin/webhooks/events'),
    // What the server can tell of only changes when the binary does.
    staleTime: 60 * 60_000,
  })

  const [editing, setEditing] = useState<WebhookDestination | 'new' | null>(null)
  const [removing, setRemoving] = useState<WebhookDestination | null>(null)
  const [issued, setIssued] = useState<{ name: string; secret: string } | null>(null)
  const [reading, setReading] = useState<string | null>(null)

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: [...DESTINATIONS] })

  const remove = useMutation({
    mutationFn: (id: string) => api.delete(`/admin/notifications/${segment(id)}`),
    onSuccess: (_, id) => {
      setRemoving(null)
      if (reading === id) setReading(null)
      invalidate()
    },
  })

  const switching = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.patch<WebhookDestination>(`/admin/notifications/${segment(id)}`, { enabled }),
    onSettled: invalidate,
  })

  const saved = (destination: WebhookDestination) => {
    setEditing(null)
    if (destination.secret) setIssued({ name: destination.name, secret: destination.secret })
    invalidate()
  }

  const read = destinations.data?.find((destination) => destination.id === reading)

  return (
    <div className="mx-auto max-w-5xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {t.admin.notificationsNav}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{n.lead}</p>
        <p className="mt-2 max-w-prose text-xs leading-relaxed text-bone-faint">{n.legacy}</p>
      </header>

      {/* Keyed by the secret: the next one made is a panel of its own, not
          this one still marked as copied. */}
      {issued ? <IssuedSecret key={issued.secret} issued={issued} onDismiss={() => setIssued(null)} /> : null}

      <Panel label={n.destinations} className="rise overflow-hidden" style={{ animationDelay: '60ms' }}>
        <PanelHead
          title={n.destinations}
          action={
            <Button size="sm" variant="primary" onClick={() => setEditing('new')} disabled={!known.data}>
              <Glyph name="plus" className="size-4" />
              {n.add}
            </Button>
          }
        />

        {destinations.isPending ? (
          <div className="space-y-2 p-4">
            {Array.from({ length: 2 }, (_, index) => (
              <Skeleton key={index} className="h-16 w-full" />
            ))}
          </div>
        ) : destinations.isError ? (
          <p role="alert" className="flex items-center gap-2 px-5 py-6 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {n.loadFailed}
          </p>
        ) : destinations.data.length === 0 ? (
          <EmptyState title={n.empty} hint={n.emptyHint} />
        ) : (
          <ul className="divide-y divide-rule">
            {destinations.data.map((destination) => (
              <DestinationRow
                key={destination.id}
                destination={destination}
                reading={reading === destination.id}
                onRead={() => setReading((open) => (open === destination.id ? null : destination.id))}
                onEdit={() => setEditing(destination)}
                onRemove={() => setRemoving(destination)}
                onSwitch={(enabled) => switching.mutate({ id: destination.id, enabled })}
                switching={switching.isPending && switching.variables.id === destination.id}
              />
            ))}
          </ul>
        )}
        {known.isError ? (
          <p role="alert" className="flex items-center gap-2 border-t border-rule px-5 py-3 text-xs text-vermillion">
            <Glyph name="alert" className="size-3.5" />
            {n.eventsFailed}
          </p>
        ) : null}
        {switching.isError ? (
          <p role="alert" className="flex items-center gap-2 border-t border-rule px-5 py-3 text-xs text-vermillion">
            <Glyph name="alert" className="size-3.5" />
            {switching.error.message}
          </p>
        ) : null}
      </Panel>

      {read ? <DeliveryLog key={read.id} destination={read} onClose={() => setReading(null)} /> : null}

      {editing && known.data ? (
        <Editor
          key={editing === 'new' ? 'new' : editing.id}
          destination={editing === 'new' ? null : editing}
          known={known.data}
          onClose={() => setEditing(null)}
          onSaved={saved}
        />
      ) : null}

      <Dialog
        open={removing !== null}
        title={n.removeTitle}
        onClose={() => setRemoving(null)}
        footer={
          <>
            <Button onClick={() => setRemoving(null)}>{t.common.cancel}</Button>
            <Button
              variant="danger"
              disabled={remove.isPending}
              onClick={() => removing && remove.mutate(removing.id)}
            >
              {remove.isPending ? <Spinner className="size-4" /> : <Glyph name="trash" className="size-4" />}
              {n.remove}
            </Button>
          </>
        }
      >
        {removing ? n.removeBody(removing.name) : null}
        {remove.isError ? (
          <p role="alert" className="mt-3 text-xs text-vermillion">
            {remove.error.message}
          </p>
        ) : null}
      </Dialog>
    </div>
  )
}

/** What a test came to, in a sentence, and whether it went well. */
function tested(result: WebhookTest, n: Words) {
  if (result.refused === 'not_web') return { fine: false, text: n.notWeb }
  if (result.refused === 'internal') return { fine: false, text: n.internal }
  if (result.refused === 'unsent') return { fine: false, text: n.unsent }
  if (result.status !== undefined) {
    return result.delivered
      ? { fine: true, text: n.delivered(result.status, result.millis) }
      : { fine: false, text: n.refused(result.status, result.millis) }
  }
  return { fine: false, text: n.unreached(result.millis) }
}

/** The events a destination is told of, in a few words. */
function eventsSaid(events: string[], n: Words) {
  return events.includes(EVERY) ? n.everyEvent : n.eventCount(events.length)
}

/**
 * One destination: what it is, where it posts — its host — what it keeps,
 * what its log holds; and what can be done to it, the test first. The test's
 * outcome is said under it, in a line that is always there so that what
 * appears in it is read out.
 */
function DestinationRow({
  destination,
  reading,
  onRead,
  onEdit,
  onRemove,
  onSwitch,
  switching,
}: {
  destination: WebhookDestination
  reading: boolean
  onRead: () => void
  onEdit: () => void
  onRemove: () => void
  onSwitch: (enabled: boolean) => void
  switching: boolean
}) {
  const { t } = useI18n()
  const n = t.admin.notifications
  const test = useMutation({
    mutationFn: () => api.post<WebhookTest>(`/admin/notifications/${segment(destination.id)}/test`),
  })
  const outcome = test.data ? tested(test.data, n) : undefined
  const counts = destination.deliveries
  const keeps = destination.secretSet
    ? destination.kind === 'standard'
      ? n.signed
      : n.tokenSet
    : null

  return (
    <li className={cn('px-5 py-4', destination.enabled ? '' : 'bg-ink/40')} data-destination={destination.id}>
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <div className="min-w-0 flex-1 basis-64">
          <p className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium text-bone">{destination.name}</span>
            <Chip tone={destination.kind === 'standard' ? 'manual' : 'neutral'}>
              {n.kinds[destination.kind] ?? destination.kind}
            </Chip>
            <Lamp tone={destination.enabled ? 'moss' : destination.disabledReason ? 'vermillion' : 'faint'}>
              {destination.enabled ? n.on : n.off}
            </Lamp>
          </p>
          <p className="mt-1 font-mono text-xs break-all text-bone-faint">{destination.address}</p>
          <p className="mt-1 text-xs text-bone-dim">
            {[eventsSaid(destination.events, n), keeps, n.counts(counts.pending, counts.failed, counts.delivered)]
              .filter(Boolean)
              .join(' · ')}
          </p>
          {destination.disabledReason ? (
            <p className="mt-1 text-xs text-vermillion">
              {n.offWhy} <span className="font-mono">{destination.disabledReason}</span>
            </p>
          ) : null}
        </div>

        <div className="flex flex-wrap items-center gap-1">
          <Toggle
            checked={destination.enabled}
            label={n.sendTo(destination.name)}
            onChange={onSwitch}
            disabled={switching}
          />
          <Button
            size="sm"
            onClick={() => test.mutate()}
            disabled={test.isPending}
            aria-describedby={`notification-test-${destination.id}`}
          >
            {test.isPending ? <Spinner className="size-4" /> : <Glyph name="share" className="size-4" />}
            {test.isPending ? n.testing : n.test}
          </Button>
          <Button
            size="sm"
            variant={reading ? 'quiet' : 'ghost'}
            aria-expanded={reading}
            aria-controls="notifications-log"
            onClick={onRead}
          >
            <Glyph name="journal" className="size-4" />
            {n.log.open}
          </Button>
          <IconButton glyph="pencil" label={n.editFor(destination.name)} onClick={onEdit} />
          <IconButton glyph="trash" tone="danger" label={n.removeFor(destination.name)} onClick={onRemove} />
        </div>
      </div>

      <p id={`notification-test-${destination.id}`} className="sr-only">
        {n.testHint}
      </p>
      {/* Always there, so what appears in it is read out. */}
      <p role="status" className={cn('mt-2 text-xs', outcome?.fine ? 'text-moss' : 'text-vermillion')}>
        {outcome ? outcome.text : ''}
        {test.data?.error ? (
          <span className="mt-0.5 block font-mono break-words text-bone-faint">{test.data.error}</span>
        ) : null}
      </p>
      {test.isError ? (
        <p role="alert" className="mt-1 text-xs text-vermillion">
          {test.error.message}
        </p>
      ) : null}
    </li>
  )
}

/**
 * A destination made or changed, in a dialog: its kind first, since the
 * rest follows from it — what the address looks like, whether it keeps a
 * token, whether it is signed. A kept address and a kept token are never
 * filled in: left empty, they stay — the token only while the destination
 * stays the same kind at the same origin, since the server sends nothing it
 * kept for one server to another, and makes a standard destination's
 * receiver at a new origin a secret of its own.
 */
function Editor({
  destination,
  known,
  onClose,
  onSaved,
}: {
  destination: WebhookDestination | null
  known: WebhookEvents
  onClose: () => void
  onSaved: (saved: WebhookDestination) => void
}) {
  const { t } = useI18n()
  const n = t.admin.notifications

  const [kind, setKind] = useState<WebhookKind>(destination?.kind ?? 'standard')
  const [name, setName] = useState(destination?.name ?? '')
  const [address, setAddress] = useState('')
  const [token, setToken] = useState('')
  const [dropToken, setDropToken] = useState(false)
  const [events, setEvents] = useState<string[]>(destination?.events ?? known.defaults)
  const [enabled, setEnabled] = useState(destination?.enabled ?? true)
  // Failed refreshes told together: by default for the kinds people read,
  // following the kind chosen for a new one until it is set by hand.
  const [grouped, setGrouped] = useState(destination?.grouped ?? READ_BY_PEOPLE.includes(kind))
  const [groupedSet, setGroupedSet] = useState(Boolean(destination))
  const [rotating, setRotating] = useState(false)

  const takesToken = TOKEN_KINDS.includes(kind)
  // Another origin — scheme, host or port — is another server: what was
  // kept for the old one does not go to it. An address left empty stays.
  const moved = Boolean(
    destination && address.trim() && originOf(address) !== originOf(destination.address),
  )
  // A token kept goes on being kept while the destination stays the same
  // kind at the same origin; otherwise it is dropped unless typed again.
  const hadToken = Boolean(destination?.secretSet && TOKEN_KINDS.includes(destination.kind))
  const tokenKept = hadToken && destination?.kind === kind && !moved
  const tokenLeft = hadToken && takesToken && !tokenKept
  // Removing is asked of a token kept: with none kept, there is none to remove.
  const dropping = dropToken && tokenKept
  const signedBefore = destination?.kind === 'standard' && destination.secretSet

  const save = useMutation({
    mutationFn: () => {
      const body: WebhookDestinationInput = { name: name.trim(), kind, events, enabled, grouped }
      if (address.trim()) body.address = address.trim()
      if (takesToken) {
        if (dropping) body.token = ''
        else if (token.trim()) body.token = token.trim()
      }
      return destination
        ? api.patch<WebhookDestination>(`/admin/notifications/${segment(destination.id)}`, body)
        : api.post<WebhookDestination>('/admin/notifications', body)
    },
    onSuccess: onSaved,
  })

  const rotate = useMutation({
    mutationFn: (id: string) => api.post<WebhookDestination>(`/admin/notifications/${segment(id)}/secret`),
    onSuccess: onSaved,
  })

  const missing =
    !name.trim() ||
    (!destination && !address.trim()) ||
    (kind === 'gotify' && !token.trim() && !(tokenKept && !dropping))
  const message = save.error instanceof ApiError ? save.error.message : save.error ? n.saveFailed : undefined

  return (
    <Dialog
      open
      size="lg"
      title={destination ? n.editTitle(destination.name) : n.addTitle}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t.common.cancel}</Button>
          <Button type="submit" form="notification-form" variant="primary" disabled={save.isPending || missing}>
            {save.isPending ? <Spinner className="size-4" /> : <Glyph name="check" className="size-4" />}
            {destination ? n.save : n.create}
          </Button>
        </>
      }
    >
      <form
        id="notification-form"
        className="space-y-5"
        onSubmit={(event) => {
          event.preventDefault()
          if (!missing) save.mutate()
        }}
      >
        <div className="grid gap-5 sm:grid-cols-2">
          <FormField label={n.kind} htmlFor="notification-kind" hint={n.kindHints[kind]}>
            <Select
              id="notification-kind"
              value={kind}
              onChange={(event) => {
                const next = event.target.value as WebhookKind
                setKind(next)
                if (!groupedSet) setGrouped(READ_BY_PEOPLE.includes(next))
                if (save.isError) save.reset()
              }}
            >
              {known.kinds.map((option) => (
                <option key={option} value={option}>
                  {n.kinds[option] ?? option}
                </option>
              ))}
            </Select>
          </FormField>

          <FormField label={n.name} htmlFor="notification-name">
            <Input
              id="notification-name"
              required
              maxLength={100}
              autoComplete="off"
              placeholder={n.namePlaceholder}
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </FormField>
        </div>

        <FormField
          label={n.address}
          htmlFor="notification-address"
          hint={destination ? n.addressKeptHint : undefined}
        >
          <Input
            id="notification-address"
            type="url"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            required={!destination}
            placeholder={destination ? n.addressKept(destination.address) : n.placeholders[kind]}
            value={address}
            onChange={(event) => setAddress(event.target.value)}
          />
        </FormField>

        {takesToken ? (
          <FormField
            label={n.token}
            htmlFor="notification-token"
            hint={tokenKept ? n.tokenKept : n.tokenHints[kind]}
          >
            <Input
              id="notification-token"
              type="password"
              autoComplete="new-password"
              spellCheck={false}
              required={kind === 'gotify' && !tokenKept}
              aria-describedby={tokenLeft ? 'notification-token-left' : undefined}
              disabled={dropping}
              value={token}
              onChange={(event) => setToken(event.target.value)}
            />
            {tokenLeft ? (
              <p id="notification-token-left" className="mt-2 flex items-start gap-2 text-xs leading-relaxed text-brass">
                <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
                {n.tokenLeft}
              </p>
            ) : null}
            {tokenKept && kind === 'ntfy' ? (
              <label className="mt-2 flex min-h-11 cursor-pointer items-center gap-3 text-sm text-bone">
                <input
                  type="checkbox"
                  className="size-4 shrink-0 cursor-pointer accent-vermillion"
                  checked={dropToken}
                  onChange={(event) => setDropToken(event.target.checked)}
                />
                {n.tokenRemove}
              </label>
            ) : null}
          </FormField>
        ) : null}

        {kind === 'standard' || signedBefore ? (
          <div className="space-y-2">
            <p className="label">{n.signing}</p>
            <p className="text-xs leading-relaxed text-bone-faint">
              {kind !== 'standard'
                ? n.signingDropped
                : signedBefore
                  ? moved
                    ? n.signingMoved
                    : n.signingKept
                  : n.signingNew}
            </p>
            {destination && signedBefore && kind === 'standard' && !moved ? (
              rotating ? (
                <div className="space-y-2 rounded-card border border-vermillion-deep bg-vermillion/[0.06] p-3">
                  <p className="text-xs leading-relaxed text-bone">{n.rotateAsk}</p>
                  <div className="flex flex-wrap gap-2">
                    <Button size="sm" onClick={() => setRotating(false)}>
                      {t.common.cancel}
                    </Button>
                    <Button
                      size="sm"
                      variant="danger"
                      disabled={rotate.isPending}
                      onClick={() => rotate.mutate(destination.id)}
                    >
                      {rotate.isPending ? <Spinner className="size-4" /> : <Glyph name="key" className="size-4" />}
                      {n.rotateConfirm}
                    </Button>
                  </div>
                  {rotate.isError ? (
                    <p role="alert" className="text-xs text-vermillion">
                      {rotate.error.message}
                    </p>
                  ) : null}
                </div>
              ) : (
                <Button size="sm" onClick={() => setRotating(true)}>
                  <Glyph name="key" className="size-4" />
                  {n.rotate}
                </Button>
              )
            ) : null}
          </div>
        ) : null}

        <div className="flex items-center gap-3">
          <Toggle id="notification-enabled" checked={enabled} label={n.on} onChange={setEnabled} />
          <label htmlFor="notification-enabled" className="text-sm text-bone">
            {n.on}
          </label>
        </div>

        <div className="flex items-start gap-3">
          <Toggle
            id="notification-grouped"
            checked={grouped}
            label={n.grouped}
            onChange={(next) => {
              setGrouped(next)
              setGroupedSet(true)
            }}
          />
          <div className="min-w-0">
            <label htmlFor="notification-grouped" className="block text-sm text-bone">
              {n.grouped}
            </label>
            <p className="mt-0.5 max-w-prose text-xs leading-relaxed text-bone-faint">{n.groupedHint}</p>
          </div>
        </div>

        <EventPicker value={events} known={known} onChange={setEvents} disabled={save.isPending} />

        {message ? (
          <p role="alert" className="flex items-start gap-1.5 text-xs text-vermillion">
            <Glyph name="alert" className="mt-0.5 size-3.5 shrink-0" />
            {message}
          </p>
        ) : null}
      </form>
    </Dialog>
  )
}

/**
 * The one moment a signing secret exists anywhere a person can read it.
 * The server keeps it to sign with and never shows it again, so this waits
 * to be dismissed by somebody who says they have it.
 */
function IssuedSecret({
  issued,
  onDismiss,
}: {
  issued: { name: string; secret: string }
  onDismiss: () => void
}) {
  const { t } = useI18n()
  const s = t.admin.notifications.issued
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(issued.secret)
      setCopied('yes')
    } catch {
      // A browser that refuses the clipboard is not a reason to lose it.
      setCopied('failed')
    }
  }

  return (
    <Panel className="rise mb-6 border-vermillion-deep bg-vermillion/[0.06]">
      <div className="p-5">
        <span className="label flex items-center gap-2 text-vermillion">
          <Glyph name="key" className="size-3.5" />
          {s.title(issued.name)}
        </span>
        <p className="mt-2 mb-4 max-w-prose text-sm leading-relaxed text-bone">{s.onlyOnce}</p>

        <label htmlFor="issued-secret" className="label mb-1.5 block">
          {s.theSecret}
        </label>
        <div className="flex flex-wrap items-center gap-2">
          <input
            id="issued-secret"
            readOnly
            value={issued.secret}
            onFocus={(event) => event.currentTarget.select()}
            className={cn(
              'min-h-11 min-w-0 flex-1 rounded-card border border-vermillion-deep bg-ink px-3',
              'font-mono text-sm text-bone selection:bg-vermillion selection:text-ink',
            )}
          />
          <Button variant="primary" onClick={() => void copy()}>
            <Glyph name={copied === 'yes' ? 'check' : 'copy'} className="size-4" />
            {copied === 'yes' ? s.copied : s.copy}
          </Button>
          <Button onClick={onDismiss}>{s.dismiss}</Button>
        </div>
        {copied === 'failed' ? (
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {s.copyFailed}
          </p>
        ) : null}
        <p className="mt-3 max-w-prose text-xs leading-relaxed text-bone-faint">{s.verify}</p>
      </div>
    </Panel>
  )
}

/** A delivery's standing, as a lamp: the word carries it, the colour helps. */
const LAMPS = { pending: 'brass', delivered: 'moss', failed: 'vermillion' } as const

/**
 * What a destination was told, the newest first: each event, how it came
 * out, its tries and its next one, and a way to tell it again. Asked again
 * every few seconds while something waits, since that is when it moves.
 */
function DeliveryLog({ destination, onClose }: { destination: WebhookDestination; onClose: () => void }) {
  const { t, locale } = useI18n()
  const n = t.admin.notifications
  const l = n.log
  const queryClient = useQueryClient()
  const [page, setPage] = useState(0)

  const log = useQuery({
    queryKey: [...DESTINATIONS, destination.id, 'log', page],
    queryFn: () =>
      api.get<WebhookDeliveries>(
        `/admin/notifications/${segment(destination.id)}/deliveries${query({ limit: PAGE, offset: page * PAGE })}`,
      ),
    refetchInterval: (current) =>
      current.state.data?.deliveries.some((delivery) => delivery.status === 'pending') ? 5_000 : 30_000,
  })

  const replay = useMutation({
    mutationFn: (id: string) =>
      api.post<WebhookDelivery>(
        `/admin/notifications/${segment(destination.id)}/deliveries/${segment(id)}/replay`,
      ),
    onSuccess: () => {
      setPage(0)
      void queryClient.invalidateQueries({ queryKey: [...DESTINATIONS] })
    },
  })

  // A delivery is known by the event it told. The log is read a page at a
  // time, newest first, and no head orders it: a page put in another order
  // here would say nothing of the deliveries on the other pages. One layout
  // for every destination's log.
  const columns: Column[] = useMemo(
    () => [
      { id: 'when', label: l.colWhen },
      { id: 'event', label: l.colEvent, kind: 'primary', always: true },
      { id: 'outcome', label: l.colOutcome },
      { id: 'tries', label: l.colTries, from: 'md' },
      { id: 'replay', label: l.replay, kind: 'actions', always: true, align: 'right' },
    ],
    [l],
  )
  const table = useDataTable({ id: 'notification-log', columns })

  return (
    <Panel
      id="notifications-log"
      label={l.title(destination.name)}
      className="rise mt-6 overflow-clip"
      style={{ animationDelay: '40ms' }}
    >
      <PanelHead
        title={l.title(destination.name)}
        action={
          <span className="flex items-center gap-2">
            {log.isFetching ? <Spinner className="size-3.5 text-bone-faint" /> : null}
            {log.data?.deliveries.length ? <TableOptions table={table} /> : null}
            <IconButton glyph="close" label={l.close} onClick={onClose} />
          </span>
        }
      />
      <p className="border-b border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{l.lead}</p>

      {log.isPending ? (
        <div className="space-y-2 p-4">
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton key={index} className="h-9 w-full" />
          ))}
        </div>
      ) : log.isError ? (
        <p role="alert" className="flex items-center gap-2 px-5 py-6 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {l.loadFailed}
        </p>
      ) : log.data.deliveries.length === 0 ? (
        <EmptyState title={l.empty} hint={l.emptyHint} />
      ) : (
        <DataTable table={table} label={l.title(destination.name)}>
          <THead>
            <Th column="when" />
            <Th column="event" />
            <Th column="outcome" />
            <Th column="tries" />
            <Th column="replay">
              <span className="sr-only">{l.replay}</span>
            </Th>
          </THead>
          <TBody>
            {log.data.deliveries.map((delivery) => (
              <DeliveryRow
                key={delivery.id}
                delivery={delivery}
                locale={locale}
                canReplay={destination.enabled && delivery.status !== 'pending'}
                replaying={replay.isPending && replay.variables === delivery.id}
                onReplay={() => replay.mutate(delivery.id)}
              />
            ))}
          </TBody>
        </DataTable>
      )}

      {replay.isError ? (
        <p role="alert" className="flex items-center gap-2 border-t border-rule px-5 py-3 text-xs text-vermillion">
          <Glyph name="alert" className="size-3.5" />
          {replay.error instanceof ApiError ? replay.error.message : l.replayFailed}
        </p>
      ) : null}

      {log.data && (log.data.total > PAGE || page > 0) ? (
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-rule px-5 py-3">
          <Button size="sm" onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
            <Glyph name="chevronLeft" className="size-4" />
            {l.newer}
          </Button>
          <span className="font-mono text-xs text-bone-faint tabular-nums">
            {l.range(
              log.data.deliveries.length === 0 ? 0 : page * PAGE + 1,
              page * PAGE + log.data.deliveries.length,
              log.data.total,
            )}
          </span>
          <Button size="sm" onClick={() => setPage((p) => p + 1)} disabled={(page + 1) * PAGE >= log.data.total}>
            {l.older}
            <Glyph name="chevronRight" className="size-4" />
          </Button>
        </div>
      ) : null}
    </Panel>
  )
}

function DeliveryRow({
  delivery,
  locale,
  canReplay,
  replaying,
  onReplay,
}: {
  delivery: WebhookDelivery
  locale: string
  canReplay: boolean
  replaying: boolean
  onReplay: () => void
}) {
  const { t } = useI18n()
  const n = t.admin.notifications
  const l = n.log
  const says: Record<string, string | undefined> = t.admin.trail.says
  const named = n.events.fact[delivery.event] ?? says[delivery.event] ?? delivery.event
  const what = [delivery.told?.target, delivery.told?.detail].filter(Boolean).join(' · ')
  const answered = [
    delivery.lastStatus !== undefined ? l.answered(delivery.lastStatus) : '',
    delivery.durationMs !== undefined ? l.took(delivery.durationMs) : '',
  ]
    .filter(Boolean)
    .join(' · ')
  const tries = l.tries(delivery.attempts, MOST_ATTEMPTS)
  const next = delivery.nextAttemptAt ? l.next(fmt.relative(delivery.nextAttemptAt, locale) ?? '') : ''

  return (
    <Tr row={delivery.id} className={delivery.status === 'failed' ? 'bg-vermillion/[0.04]' : ''}>
      <Td column="when" className="whitespace-nowrap">
        <span
          className="font-mono text-xs text-bone-faint tabular-nums"
          title={fmt.dateTime(delivery.createdAt, locale)}
        >
          {fmt.relative(delivery.createdAt, locale)}
        </span>
        {delivery.replayOf ? <span className="block text-[0.6875rem] text-bone-faint">{l.replayed}</span> : null}
      </Td>
      <Td column="event" className="w-full max-w-0">
        <span className="block truncate text-sm text-bone">{named}</span>
        <span className="block font-mono text-[0.6875rem] text-bone-faint">{delivery.event}</span>
        {what ? (
          <span className="block truncate text-xs text-bone-dim" title={what}>
            {what}
          </span>
        ) : null}
      </Td>
      <Td column="outcome" className="whitespace-nowrap">
        <Lamp tone={LAMPS[delivery.status]}>{l.status[delivery.status] ?? delivery.status}</Lamp>
        {answered ? (
          <span className="block font-mono text-[0.6875rem] text-bone-faint tabular-nums">{answered}</span>
        ) : null}
        {delivery.lastError ? (
          <span
            className="block max-w-56 truncate font-mono text-[0.6875rem] text-vermillion"
            title={delivery.lastError}
          >
            {delivery.lastError}
          </span>
        ) : null}
      </Td>
      <Td column="tries" className="whitespace-nowrap">
        <span className="block font-mono text-xs text-bone-dim tabular-nums">{tries}</span>
        {next ? <span className="block text-[0.6875rem] text-bone-faint">{next}</span> : null}
      </Td>
      <Td column="replay">
        <IconButton
          glyph="refresh"
          label={l.replayFor(delivery.event)}
          onClick={onReplay}
          busy={replaying}
          disabled={!canReplay}
        />
      </Td>
    </Tr>
  )
}
