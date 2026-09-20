import { useQuery } from '@tanstack/react-query'
import { NavLink, Outlet, useNavigate } from 'react-router'
import { useEffect } from 'react'

import { ApiError, api } from '../lib/api'
import { cn } from '../lib/cn'
import type { Me, Settings } from '../lib/types'
import { Label, Lock, Spinner, Tag } from './ui'

const NAV = [
  { to: '/', label: 'Overview', end: true },
  { to: '/catalogue', label: 'Catalogue', end: false },
  { to: '/clients', label: 'Clients', end: false },
  { to: '/audit', label: 'Audit', end: false },
  { to: '/settings', label: 'Settings', end: false },
]

export function Shell() {
  const navigate = useNavigate()

  const me = useQuery({ queryKey: ['me'], queryFn: () => api.get<Me>('/auth/me') })
  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/settings'),
    enabled: me.isSuccess,
  })

  // Any rejected credential lands here, whichever query hit it first.
  useEffect(() => {
    if (me.error instanceof ApiError && me.error.isUnauthorized) {
      navigate('/login', { replace: true })
    }
  }, [me.error, navigate])

  if (me.isPending) {
    return (
      <div className="grid min-h-dvh place-items-center">
        <Spinner className="h-5 w-5" />
      </div>
    )
  }

  if (me.isError) return null

  return (
    <div className="min-h-dvh lg:grid lg:grid-cols-[232px_1fr]">
      <aside
        className={cn(
          'flex flex-col gap-8 border-b border-line bg-pit px-5 py-6',
          'lg:sticky lg:top-0 lg:h-dvh lg:border-r lg:border-b-0',
        )}
      >
        <div className="reveal">
          <div className="flex items-center gap-2 text-phos">
            <Lock />
            <span className="font-display text-[22px] leading-none tracking-tight text-paper">
              metadata
            </span>
          </div>
          <p className="mt-1.5 font-mono text-[10px] uppercase tracking-[0.2em] text-faint">
            arr server
            {settings.data && <span className="text-line-bright"> · v{settings.data.version}</span>}
          </p>
        </div>

        <nav className="flex flex-row gap-1 lg:flex-col">
          {NAV.map((entry, i) => (
            <NavLink
              key={entry.to}
              to={entry.to}
              end={entry.end}
              style={{ animationDelay: `${60 + i * 45}ms` }}
              className={({ isActive }) =>
                cn(
                  'reveal edge rounded-[2px] px-3 py-2',
                  'font-mono text-[11px] uppercase tracking-[0.14em] transition-colors',
                  isActive
                    ? 'bloom edge-locked text-paper'
                    : 'text-faint hover:text-dim',
                )
              }
            >
              {entry.label}
            </NavLink>
          ))}
        </nav>

        <div className="mt-auto hidden flex-col gap-3 lg:flex">
          {settings.data && <Health settings={settings.data} />}
          <div className="border-t border-line pt-3">
            <Label>Signed in</Label>
            <p className="mt-1 font-mono text-[12px] break-all text-dim">{me.data.identity}</p>
          </div>
          <form
            method="post"
            onSubmit={async (event) => {
              event.preventDefault()
              await api.post('/auth/logout')
              navigate('/login', { replace: true })
            }}
          >
            <button
              type="submit"
              className="font-mono text-[10px] uppercase tracking-[0.14em] text-faint transition-colors hover:text-rust"
            >
              Sign out
            </button>
          </form>
        </div>
      </aside>

      <main className="min-w-0 px-5 py-8 sm:px-8 lg:px-12 lg:py-12">
        <Outlet />
      </main>
    </div>
  )
}

/** The three things an operator actually needs to see at a glance. */
function Health({ settings }: { settings: Settings }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label>Status</Label>
      <div className="flex flex-wrap gap-1">
        <Tag tone="neutral">{settings.database}</Tag>
        <Tag tone={settings.tmdbConfigured ? 'good' : 'bad'}>
          tmdb {settings.tmdbConfigured ? 'ok' : 'off'}
        </Tag>
        <Tag tone={settings.refreshEnabled ? 'auto' : 'neutral'}>
          refresh {settings.refreshEnabled ? 'on' : 'off'}
        </Tag>
        {settings.authDisabled && <Tag tone="bad">auth off</Tag>}
      </div>
    </div>
  )
}
