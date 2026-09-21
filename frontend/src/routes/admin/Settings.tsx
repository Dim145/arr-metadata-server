/**
 * What this server does, and how it was started.
 *
 * Two kinds of fact, and the page is ordered by which one an operator came for.
 * The settings come first and are live: the server keeps them in a table it
 * re-reads on every request, so a change applies to the next one. Below them
 * sit the things that really were read once from the environment — the version,
 * the database, whether a TMDB key exists, what stands at each door — and those
 * stay read-only, because a form that appeared to change them would be lying.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useNavigate } from 'react-router'

import {
  Button,
  ButtonLink,
  Chip,
  Field,
  FormField,
  Glyph,
  Input,
  Label,
  Panel,
  PanelHead,
  Skeleton,
  Spinner,
} from '../../components/ui'
import { ServerSettings } from '../../components/ScopeSettings'
import { ApiError, api } from '../../lib/api'
import { useI18n } from '../../lib/i18n'
import type { ExportSummary, Settings as Config } from '../../lib/types'
import { PolicyChip } from './Dashboard'

export function Settings() {
  const { t } = useI18n()

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Config>('/settings'),
    staleTime: 5 * 60_000,
  })

  if (settings.isPending) {
    return (
      <div className="mx-auto max-w-4xl space-y-6">
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    )
  }

  if (settings.isError) {
    return (
      <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
        <Glyph name="alert" className="size-4" />
        {t.admin.config.loadFailed}
      </p>
    )
  }

  const config = settings.data

  return (
    <div className="mx-auto max-w-4xl">
      <header className="rise mb-8">
        <Label>{t.admin.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {t.admin.settings}
        </h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">
          {t.admin.config.lead}
        </p>
      </header>

      {config.authDisabled ? (
        <p
          role="alert"
          className="rise mb-6 flex items-start gap-3 rounded-panel border border-vermillion-deep bg-vermillion/[0.06] px-5 py-4 text-sm leading-relaxed text-bone"
        >
          <Glyph name="alert" className="mt-0.5 size-4 shrink-0 text-vermillion" />
          <span>
            <strong className="font-medium">{t.admin.config.authOffTitle}</strong>{' '}
            {t.admin.config.authOffBody}
          </span>
        </p>
      ) : null}

      <p className="rise mb-4 max-w-prose text-sm leading-relaxed text-bone-dim">
        {t.settings.lead}
      </p>

      <ServerSettings delay={40} />

      <div className="mt-6 grid gap-6 lg:grid-cols-2">
        <Panel className="rise" style={{ animationDelay: '260ms' }}>
          <PanelHead title={t.admin.config.runtime} />
          <dl className="divide-y divide-rule">
            <Field label={t.admin.config.version}>
              <span className="font-mono text-[0.8125rem] tabular-nums">{config.version}</span>
            </Field>
            <Field label={t.admin.config.database}>
              <span className="font-mono text-[0.8125rem]">{config.database}</span>
            </Field>
            <Field label={t.admin.config.publicUrl}>
              <span className="font-mono text-[0.8125rem] break-all">
                {config.publicUrl ?? '—'}
              </span>
            </Field>
            <Field label={t.admin.config.tmdb}>
              {config.tmdbConfigured ? (
                <Chip>
                  <Glyph name="check" className="size-3" />
                  {t.admin.config.tmdbReady}
                </Chip>
              ) : (
                <Chip tone="accent">
                  <Glyph name="alert" className="size-3" />
                  {t.admin.config.tmdbMissing}
                </Chip>
              )}
            </Field>
            <Field label={t.admin.config.publicBrowse}>
              <State on={config.publicBrowse} />
            </Field>
          </dl>
          <p className="border-t border-rule px-5 py-4 text-xs leading-relaxed text-bone-faint">
            {t.admin.config.runtimeHint}
          </p>
        </Panel>

        <Panel className="rise" style={{ animationDelay: '300ms' }}>
          <PanelHead title={t.admin.config.policy} />
          <dl className="divide-y divide-rule">
            {(
              [
                ['/api/v1/*', config.nativePolicy],
                ['/3/*', config.tmdbPolicy],
                ['/v1/*', config.arrPolicy],
              ] as const
            ).map(([path, policy]) => (
              <div key={path} className="flex items-center justify-between gap-4 px-5 py-2.5">
                <dt className="font-mono text-[0.8125rem] text-bone">{path}</dt>
                <dd>
                  <PolicyChip policy={policy} />
                </dd>
              </div>
            ))}
          </dl>
          <p className="border-t border-rule px-5 py-4 text-xs leading-relaxed text-bone-faint">
            {t.admin.config.policyHint}
          </p>
        </Panel>
      </div>

      <Panel className="rise mt-6" style={{ animationDelay: '340ms' }}>
        <PanelHead
          title={t.admin.config.export}
          action={
            <Link
              to="/admin/jobs"
              className="label -my-2 inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
            >
              {t.admin.jobs}
            </Link>
          }
        />
        <NfoExport />
      </Panel>

      <Panel className="rise mt-6" style={{ animationDelay: '380ms' }}>
        <PanelHead title={t.admin.config.docs} />
        <div className="flex flex-wrap items-center justify-between gap-4 p-5">
          <div className="min-w-0">
            <p className="text-sm text-bone">{t.admin.config.docsTitle}</p>
            <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">
              {t.admin.config.docsBody}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <ButtonLink href="/api/docs" target="_blank" rel="noreferrer" variant="primary">
              <Glyph name="external" className="size-4" />
              {t.admin.config.open}
            </ButtonLink>
            <ButtonLink href="/api/openapi.json" target="_blank" rel="noreferrer">
              openapi.json
            </ButtonLink>
          </div>
        </div>
      </Panel>

      <Panel className="rise mt-6" style={{ animationDelay: '420ms' }}>
        <PanelHead title={t.admin.config.maintenance} />
        <ClearCache />
      </Panel>

      <Panel className="rise mt-6 mb-4" style={{ animationDelay: '460ms' }}>
        <PanelHead title={t.admin.config.password} />
        <ChangePassword />
      </Panel>
    </div>
  )
}

function State({ on }: { on: boolean }) {
  const { t } = useI18n()

  return (
    <Chip tone={on ? 'provider' : 'neutral'}>
      <Glyph name={on ? 'check' : 'close'} className="size-3" />
      {on ? t.admin.config.enabled : t.admin.config.disabled}
    </Chip>
  )
}

/** Write a `.nfo` document for everything, for a library Plex reads. */
function NfoExport() {
  const { t } = useI18n()
  const queryClient = useQueryClient()

  const run = useMutation({
    mutationFn: () => api.post<ExportSummary>('/export/nfo'),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['jobs'] })
    },
  })

  const missingPath =
    run.error instanceof ApiError &&
    (run.error.message.includes('TMDB API key') || run.error.code === 'provider_not_configured')

  return (
    <div className="flex flex-wrap items-center justify-between gap-4 p-5">
      <div className="min-w-0">
        <p className="text-sm text-bone">{t.admin.config.exportTitle}</p>
        <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">
          {t.admin.config.exportBody}
        </p>

        {run.isSuccess ? (
          <p className="mt-2 font-mono text-xs text-moss">
            {t.admin.config.exportDone(run.data.works, run.data.episodes)}
            {run.data.failed > 0 ? ` · ${t.admin.config.exportFailed(run.data.failed)}` : ''}
            <span className="mt-0.5 block break-all text-bone-faint">{run.data.root}</span>
          </p>
        ) : null}

        {run.isError ? (
          <p role="alert" className="mt-2 text-xs text-vermillion">
            {missingPath ? t.admin.config.exportNoPath : run.error.message}
          </p>
        ) : null}
      </div>

      <Button variant="primary" onClick={() => run.mutate()} disabled={run.isPending}>
        {run.isPending ? <Spinner className="size-4" /> : <Glyph name="download" className="size-4" />}
        {run.isPending ? t.admin.config.exportRunning : t.admin.config.exportRun}
      </Button>
    </div>
  )
}

function ClearCache() {
  const { t } = useI18n()
  const clear = useMutation({ mutationFn: () => api.post('/cache/clear') })

  return (
    <div className="flex flex-wrap items-center justify-between gap-4 p-5">
      <div className="min-w-0">
        <p className="text-sm text-bone">{t.admin.config.cacheTitle}</p>
        <p className="mt-1 max-w-prose text-xs leading-relaxed text-bone-faint">
          {t.admin.config.cacheBody}
        </p>
      </div>
      <Button onClick={() => clear.mutate()} disabled={clear.isPending}>
        {clear.isPending ? <Spinner className="size-4" /> : null}
        {clear.isSuccess ? t.admin.config.cacheCleared : t.admin.config.cacheClear}
      </Button>
    </div>
  )
}

/**
 * The password, changed.
 *
 * The server ends every session when it succeeds — they were all authorised
 * under the old password — so this form's success state is the sign-in page.
 * Saying so beforehand is the difference between that and looking like a crash.
 */
function ChangePassword() {
  const { t } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [repeat, setRepeat] = useState('')

  const change = useMutation({
    mutationFn: () =>
      api.post('/auth/password', { currentPassword: current, newPassword: next }),
    onSuccess: () => {
      queryClient.clear()
      navigate('/login', { replace: true })
    },
  })

  const mismatch = repeat !== '' && next !== repeat
  const wrongCurrent = change.error instanceof ApiError && change.error.isUnauthorized

  return (
    <form
      className="grid gap-4 p-5 sm:grid-cols-2"
      onSubmit={(event) => {
        event.preventDefault()
        if (!mismatch) change.mutate()
      }}
    >
      <p className="max-w-prose text-xs leading-relaxed text-bone-faint sm:col-span-2">
        {t.admin.config.passwordTitle}. {t.admin.config.passwordBody}
      </p>

      <div className="sm:col-span-2">
        <FormField
          label={t.admin.config.currentPassword}
          htmlFor="current-password"
          error={
            wrongCurrent
              ? t.admin.config.passwordWrong
              : change.isError
                ? change.error.message
                : undefined
          }
        >
          <Input
            id="current-password"
            type="password"
            autoComplete="current-password"
            required
            value={current}
            onChange={(event) => setCurrent(event.target.value)}
          />
        </FormField>
      </div>

      <FormField
        label={t.admin.config.newPassword}
        htmlFor="new-password"
        hint={t.admin.config.passwordRule}
      >
        <Input
          id="new-password"
          type="password"
          autoComplete="new-password"
          required
          minLength={12}
          value={next}
          onChange={(event) => setNext(event.target.value)}
        />
      </FormField>

      <FormField
        label={t.admin.config.repeatPassword}
        htmlFor="repeat-password"
        error={mismatch ? t.admin.config.passwordMismatch : undefined}
      >
        <Input
          id="repeat-password"
          type="password"
          autoComplete="new-password"
          required
          value={repeat}
          onChange={(event) => setRepeat(event.target.value)}
        />
      </FormField>

      <div className="sm:col-span-2">
        <Button
          type="submit"
          variant="primary"
          disabled={change.isPending || mismatch || !current || next.length < 12}
        >
          {change.isPending ? <Spinner className="size-4" /> : <Glyph name="lock" className="size-4" />}
          {t.admin.config.passwordSubmit}
        </Button>
      </div>
    </form>
  )
}
