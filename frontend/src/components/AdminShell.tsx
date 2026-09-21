/**
 * The chrome around the administration side.
 *
 * Deliberately a different room from the public catalogue: a standing sidebar
 * on a raised surface instead of a slim bar over the page, and the accent used
 * to mark where you are rather than where you may go. An operator who glances
 * at a screenshot should never have to wonder which side produced it.
 *
 * Below `lg` the sidebar becomes a bottom bar, because a hamburger hides the
 * five places this side actually has, and a thumb reaches the bottom of a phone
 * far more easily than the top.
 */

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { createContext, use, useEffect, useState } from 'react'
import { Link, NavLink, Navigate, Outlet, useLocation, useNavigate } from 'react-router'

import { api } from '../lib/api'
import { cn } from '../lib/cn'
import { useI18n, type Dict, type Lang } from '../lib/i18n'
import type { Me, Settings } from '../lib/types'
import { Glyph, Spinner, type GlyphName } from './ui'

type Entry = {
  to: string
  glyph: GlyphName
  label: (t: Dict) => string
  end?: boolean
  /** Whether it earns one of the five places a thumb can reach. */
  tab?: boolean
}

// The bottom bar holds five. Jobs was already off it and the audit trail now
// joins it: both are read-only histories, consulted rather than operated, and
// the dashboard's figures link to each. Importing takes the place that frees,
// because adding a work is an errand somebody actually runs from a phone.
const NAV: Entry[] = [
  { to: '/admin', glyph: 'gauge', label: (t) => t.admin.dashboard, end: true, tab: true },
  { to: '/admin/catalogue', glyph: 'list', label: (t) => t.admin.catalogue, tab: true },
  { to: '/admin/discover', glyph: 'discover', label: (t) => t.admin.discover, tab: true },
  { to: '/admin/clients', glyph: 'key', label: (t) => t.admin.clients, tab: true },
  { to: '/admin/jobs', glyph: 'clock', label: (t) => t.admin.jobs },
  { to: '/admin/audit', glyph: 'journal', label: (t) => t.admin.audit },
  { to: '/admin/settings', glyph: 'settings', label: (t) => t.admin.settings, tab: true },
]

/**
 * What the narrow top bar says, when a page knows better than its route does.
 *
 * Only the work editor needs this — every other title is the section's own
 * name, which the route already carries.
 */
const TitleContext = createContext<((title: string | null) => void) | null>(null)

export function useAdminTitle(title: string | null) {
  const set = use(TitleContext)

  useEffect(() => {
    set?.(title)
    return () => set?.(null)
  }, [set, title])
}

export function AdminShell() {
  const { t } = useI18n()
  const location = useLocation()
  const [override, setOverride] = useState<string | null>(null)

  const me = useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<Me>('/auth/me'),
    retry: false,
    staleTime: 5 * 60_000,
  })

  if (me.isPending) {
    return (
      <div className="grid min-h-dvh place-items-center">
        <Spinner className="size-6 text-bone-dim" />
        <span className="sr-only">{t.admin.checking}</span>
      </div>
    )
  }

  // Both a rejected credential and a signed-in reader who may not write end up
  // at the door: there is nothing on this side for either of them.
  if (me.isError || !me.data.canWrite) {
    return <Navigate to="/login" replace />
  }

  const current = NAV.find((entry) =>
    entry.end ? location.pathname === entry.to : location.pathname.startsWith(entry.to),
  )

  return (
    // The same containment the public shell uses, for the same reason: a table
    // wide enough to scroll must not take the page sideways with it.
    <div className="grain ambience-quiet min-h-dvh overflow-x-clip lg:grid lg:grid-cols-[15rem_minmax(0,1fr)]">
      <a
        href="#admin-main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-50 focus:rounded-card focus:bg-vermillion focus:px-4 focus:py-2 focus:text-sm focus:font-medium focus:text-ink"
      >
        {t.nav.skipToContent}
      </a>

      <Sidebar identity={me.data.identity} />

      <div className="flex min-w-0 flex-col">
        <TopBar title={override ?? (current ? current.label(t) : t.admin.title)} />

        <main id="admin-main" className="relative z-10 min-w-0 flex-1 px-4 pt-6 pb-28 sm:px-6 lg:px-10 lg:pt-10 lg:pb-16">
          <TitleContext value={setOverride}>
            <Outlet />
          </TitleContext>
        </main>

        <TabBar />
      </div>
    </div>
  )
}

/* ── Wide ─────────────────────────────────────────────────────────────────── */

function Sidebar({ identity }: { identity: string }) {
  const { t } = useI18n()

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/settings'),
    staleTime: 5 * 60_000,
  })

  return (
    <aside className="hidden border-r border-rule bg-ink-raised lg:sticky lg:top-0 lg:flex lg:h-dvh lg:flex-col">
      <div className="border-b border-rule px-5 py-5">
        <Link to="/admin" className="block">
          <span className="font-display text-xl leading-none font-medium tracking-tight text-bone">
            {t.brand.name}
          </span>
          <span className="mt-1.5 flex items-center gap-2">
            <span className="label text-vermillion">{t.admin.title}</span>
            {settings.data ? (
              <span className="font-mono text-[0.625rem] text-bone-faint tabular-nums">
                v{settings.data.version}
              </span>
            ) : null}
          </span>
        </Link>
      </div>

      <nav aria-label={t.admin.sections} className="flex-1 overflow-y-auto py-3">
        {NAV.map((entry) => (
          <NavRow key={entry.to} entry={entry} />
        ))}
      </nav>

      <div className="space-y-3 border-t border-rule px-5 py-4">
        <Link
          to="/"
          className="flex min-h-11 items-center gap-2 text-sm text-bone-dim transition-colors duration-150 hover:text-bone"
        >
          <Glyph name="arrowLeft" className="size-4" />
          {t.admin.backToSite}
        </Link>

        <LanguageToggle />

        <div className="border-t border-rule pt-3">
          <span className="label block">{t.admin.operator}</span>
          <span className="mt-1 block truncate font-mono text-xs text-bone-dim" title={identity}>
            {identity}
          </span>
          <SignOut className="mt-2 flex min-h-11 items-center gap-2 text-sm text-bone-faint transition-colors duration-150 hover:text-vermillion" />
        </div>
      </div>
    </aside>
  )
}

function NavRow({ entry }: { entry: Entry }) {
  const { t } = useI18n()

  return (
    <NavLink
      to={entry.to}
      end={entry.end}
      className={({ isActive }) =>
        cn(
          'relative flex h-[46px] items-center gap-3 px-5 text-sm font-medium',
          'transition-colors duration-150',
          isActive ? 'bg-ink-high text-vermillion' : 'text-bone-dim hover:bg-ink-high hover:text-bone',
        )
      }
    >
      {({ isActive }) => (
        <>
          {/* The mark is a rule at the edge rather than a pill, and it fades
              rather than slides: nothing here should move while you read it. */}
          <span
            aria-hidden
            className={cn(
              'absolute inset-y-0 left-0 w-0.5 bg-vermillion transition-opacity duration-150',
              isActive ? 'opacity-100' : 'opacity-0',
            )}
          />
          <Glyph name={entry.glyph} className="size-4" />
          {entry.label(t)}
        </>
      )}
    </NavLink>
  )
}

/* ── Narrow ───────────────────────────────────────────────────────────────── */

/**
 * Where you are, and the way out of it.
 *
 * The chevron climbs one level rather than replaying history: an operator who
 * arrived at an entry from the audit trail still means "the catalogue" when
 * they leave it, and from the dashboard the level above really is the public
 * catalogue.
 */
function TopBar({ title }: { title: string }) {
  const { t } = useI18n()
  const location = useLocation()

  const up = location.pathname.startsWith('/admin/catalogue/')
    ? '/admin/catalogue'
    : location.pathname === '/admin'
      ? '/'
      : '/admin'

  return (
    <header className="sticky top-0 z-30 flex h-14 items-center gap-1 border-b border-rule bg-ink px-1 lg:hidden">
      <Link
        to={up}
        aria-label={up === '/' ? t.admin.backToSite : t.admin.up}
        className="grid size-11 shrink-0 place-items-center rounded-card text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
      >
        <Glyph name="chevronLeft" className="size-5" />
      </Link>

      <p className="min-w-0 flex-1 truncate font-display text-lg font-medium text-bone">
        {title}
      </p>

      <LanguageToggle compact />
      <SignOut
        iconOnly
        className="grid size-11 shrink-0 place-items-center rounded-card text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-vermillion"
      />
    </header>
  )
}

function TabBar() {
  const { t } = useI18n()

  return (
    <nav
      aria-label={t.admin.sections}
      className="fixed inset-x-0 bottom-0 z-40 grid grid-cols-5 border-t border-rule bg-ink-raised pb-[env(safe-area-inset-bottom)] lg:hidden"
    >
      {NAV.filter((entry) => entry.tab).map((entry) => (
        <NavLink
          key={entry.to}
          to={entry.to}
          end={entry.end}
          className={({ isActive }) =>
            cn(
              'flex min-h-14 flex-col items-center justify-center gap-1 px-1',
              'transition-colors duration-150',
              isActive ? 'text-vermillion' : 'text-bone-faint hover:text-bone',
            )
          }
        >
          <Glyph name={entry.glyph} className="size-5" />
          <span className="line-clamp-2 w-full text-center text-[0.625rem] leading-tight font-medium">
            {entry.label(t)}
          </span>
        </NavLink>
      ))}
    </nav>
  )
}

/* ── Controls shared by both ──────────────────────────────────────────────── */

function SignOut({ className, iconOnly }: { className?: string; iconOnly?: boolean }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  return (
    <button
      type="button"
      aria-label={iconOnly ? t.nav.signOut : undefined}
      className={cn('cursor-pointer', className)}
      onClick={async () => {
        // Whatever the server says, the session is over here: everything cached
        // was answered for somebody who is now gone.
        try {
          await api.post('/auth/logout')
        } finally {
          queryClient.clear()
          navigate('/login', { replace: true })
        }
      }}
    >
      <Glyph name="signOut" className={iconOnly ? 'size-5' : 'size-4'} />
      {iconOnly ? null : t.nav.signOut}
    </button>
  )
}

function LanguageToggle({ compact }: { compact?: boolean }) {
  const { lang, setLang, t } = useI18n()

  return (
    <div
      role="group"
      aria-label={t.nav.language}
      // A segmented control rather than the public bar's inset pills: it has to
      // be hit with a thumb here, and the inset was costing the 44px.
      className={cn(
        'flex h-12 shrink-0 items-center overflow-hidden rounded-card border border-rule',
        compact ? '' : 'w-fit',
      )}
    >
      {(['en', 'fr'] as Lang[]).map((code) => (
        <button
          key={code}
          type="button"
          onClick={() => setLang(code)}
          aria-pressed={lang === code}
          className={cn(
            'h-full min-w-11 cursor-pointer px-2.5 font-mono text-[0.6875rem] font-medium tracking-wider uppercase',
            'transition-colors duration-150',
            lang === code ? 'bg-bone text-ink' : 'text-bone-faint hover:text-bone',
          )}
        >
          {code}
        </button>
      ))}
    </div>
  )
}
