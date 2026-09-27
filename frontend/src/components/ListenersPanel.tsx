/**
 * The clients' door, read back: whether there is one, the names it answers
 * to, the certificate it shows and the authority behind it — with the two
 * files a client needs to trust it, and the way to renew the certificate
 * now. The interface's own door is named in one line, because the two are
 * independent and the page should say so.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'

import { api } from '../lib/api'
import { useI18n } from '../lib/i18n'
import type { Listeners } from '../lib/types'
import { Button, ButtonLink, Chip, Glyph, Lamp, Panel, PanelHead, Skeleton } from './ui'

const COMPOSE = `services:
  sonarr:
    image: lscr.io/linuxserver/sonarr:latest
    extra_hosts:
      - "skyhook.sonarr.tv:<this server's address>"
    volumes:
      - ./arr-metadata-ca.crt:/usr/local/share/ca-certificates/arr-metadata.crt:ro
      - ./trust-ca.sh:/custom-cont-init.d/10-trust-arr-metadata-ca:ro`

export function ListenersPanel({ delay }: { delay: number }) {
  const { t, locale } = useI18n()
  const l = t.admin.access.listeners
  const queryClient = useQueryClient()
  // What the copy button last did: copied, or left the text selected for
  // the reader to copy where the clipboard is out of reach — a plain-HTTP
  // page on a LAN address, which is the setup the docs advise.
  const [copied, setCopied] = useState<'done' | 'manual' | null>(null)
  const [renewed, setRenewed] = useState(false)
  const snippet = useRef<HTMLPreElement>(null)

  const listeners = useQuery({
    queryKey: ['admin', 'tls'],
    queryFn: () => api.get<Listeners>('/admin/tls'),
    refetchInterval: (q) => (q.state.data?.clients?.renewing ? 2_000 : 60_000),
  })

  const renew = useMutation({
    mutationFn: () => api.post('/admin/tls/renew'),
    onSuccess: () => {
      setRenewed(true)
      void queryClient.invalidateQueries({ queryKey: ['admin', 'tls'] })
      void queryClient.invalidateQueries({ queryKey: ['tasks'] })
    },
  })

  // Said once, then gone: the run is in the tasks' history.
  useEffect(() => {
    if (!renewed) return
    const timer = setTimeout(() => setRenewed(false), 6_000)
    return () => clearTimeout(timer)
  }, [renewed])

  useEffect(() => {
    if (copied !== 'done') return
    const timer = setTimeout(() => setCopied(null), 2_000)
    return () => clearTimeout(timer)
  }, [copied])

  const date = (when: string) => new Date(when).toLocaleDateString(locale, { dateStyle: 'medium' })

  const copy = async () => {
    try {
      if (!navigator.clipboard) throw new Error('no clipboard')
      await navigator.clipboard.writeText(COMPOSE)
      setCopied('done')
    } catch {
      // Selected for the reader, who copies it themselves.
      const pre = snippet.current
      const selection = window.getSelection()
      if (pre && selection) {
        selection.removeAllRanges()
        const range = document.createRange()
        range.selectNodeContents(pre)
        selection.addRange(range)
      }
      setCopied('manual')
    }
  }

  const data = listeners.data
  const clients = data?.clients ?? null

  return (
    <Panel label={l.title} className="rise" style={{ animationDelay: `${delay}ms` }}>
      <PanelHead
        title={l.title}
        action={
          clients ? (
            <Lamp tone="moss">{l.bind(clients.bind)}</Lamp>
          ) : data ? (
            <Lamp tone="faint">{t.admin.tasks.off}</Lamp>
          ) : undefined
        }
      />
      <div className="space-y-5 p-5" aria-busy={listeners.isPending}>
        <p className="max-w-prose text-sm leading-relaxed text-bone-dim">{l.lead}</p>

        {listeners.isPending ? <Skeleton className="h-24 w-full" /> : null}

        {listeners.isError ? (
          <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
            <Glyph name="alert" className="size-4" />
            {listeners.error.message}
          </p>
        ) : null}

        {data ? (
          <p className="text-xs leading-relaxed text-bone-faint">{l.web(data.web.bind, data.web.tls)}</p>
        ) : null}

        {data && !clients ? <p className="text-sm leading-relaxed text-bone">{l.off}</p> : null}

        {clients ? (
          <>
            <dl className="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-[auto_minmax(0,1fr)]">
              <dt className="text-bone-faint">{l.mode}</dt>
              <dd className="text-bone">{clients.mode === 'authority' ? l.modeAuthority : l.modeOwn}</dd>

              <dt className="text-bone-faint">{l.names}</dt>
              <dd className="flex flex-wrap gap-1.5">
                {clients.names.map((name) => (
                  <Chip key={name} tone="neutral">
                    <span className="font-mono">{name}</span>
                  </Chip>
                ))}
              </dd>

              {clients.certificate ? (
                <>
                  <dt className="text-bone-faint">{l.certificate}</dt>
                  <dd className="min-w-0 text-bone">
                    <span>{l.validUntil(date(clients.certificate.notAfter))}</span>
                    {clients.certificate.due ? (
                      <Chip tone="accent" className="ml-2">
                        {l.due}
                      </Chip>
                    ) : null}
                    <span className="mt-1 block font-mono text-[0.6875rem] break-all text-bone-faint">
                      {l.fingerprint} {clients.certificate.fingerprint}
                    </span>
                  </dd>
                </>
              ) : null}

              {clients.authority ? (
                <>
                  <dt className="text-bone-faint">{l.authority}</dt>
                  <dd className="min-w-0 text-bone">
                    <span>
                      {clients.authority.subject.replace(/^CN=/, '')} · {l.validUntil(date(clients.authority.notAfter))}
                    </span>
                    <span className="mt-1 block font-mono text-[0.6875rem] break-all text-bone-faint">
                      {l.fingerprint} {clients.authority.fingerprint}
                    </span>
                    <span className="mt-1 block text-xs leading-relaxed text-bone-faint">{l.authorityHint}</span>
                  </dd>
                </>
              ) : null}
            </dl>

            {clients.refused.length > 0 ? (
              <p role="alert" className="flex items-start gap-2 text-sm leading-relaxed text-brass">
                <Glyph name="alert" className="mt-0.5 size-4 shrink-0" />
                {l.refused(clients.refused.join(', '))}
              </p>
            ) : null}

            {clients.authority ? (
              <div className="flex flex-wrap items-center gap-2">
                <ButtonLink href="/ca.crt" download="arr-metadata-ca.crt" variant="primary" size="sm">
                  {l.download}
                </ButtonLink>
                <ButtonLink href="/trust-ca.sh" download="trust-ca.sh" size="sm">
                  {l.script}
                </ButtonLink>
                <Button
                  size="sm"
                  onClick={() => renew.mutate()}
                  disabled={renew.isPending || clients.renewing}
                  aria-busy={renew.isPending || clients.renewing}
                >
                  {clients.renewing ? l.renewing : l.renew}
                </Button>
                <span role="status" className="text-xs text-moss">
                  {renewed ? l.renewed : ''}
                </span>
                {renew.isError ? (
                  <span role="alert" className="text-xs text-vermillion">
                    {renew.error.message}
                  </span>
                ) : null}
              </div>
            ) : null}

            {clients.authority ? (
              <div className="rounded-card border border-rule bg-ink">
                <div className="flex items-center justify-between gap-4 border-b border-rule px-4 py-2">
                  <span className="text-xs font-medium text-bone">{l.howTitle}</span>
                  <Button size="sm" variant="quiet" onClick={() => void copy()}>
                    {copied === 'done' ? l.copied : l.copy}
                  </Button>
                </div>
                <pre
                  ref={snippet}
                  tabIndex={0}
                  role="region"
                  aria-label={l.howTitle}
                  className="overflow-x-auto px-4 py-3 font-mono text-[0.75rem] leading-relaxed text-bone-dim"
                >
                  {COMPOSE}
                </pre>
                <p role="status" className="px-4 text-xs text-bone-faint">
                  {copied === 'done' ? l.copied : copied === 'manual' ? l.copyManual : ''}
                </p>
                <p className="border-t border-rule px-4 py-3 text-xs leading-relaxed text-bone-faint">{l.how}</p>
              </div>
            ) : null}
          </>
        ) : null}
      </div>
    </Panel>
  )
}
