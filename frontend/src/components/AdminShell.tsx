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
import { useNavigationReset } from '../lib/hooks'
import { cn } from '../lib/cn'
import { LANGS, LANGUAGES, useI18n, type Dict } from '../lib/i18n'
import { ThemeToggle } from './ThemeToggle'
import { describeIdentity } from '../lib/labels'
import type { Me, Settings, UsersPage } from '../lib/types'
import { CommandPalette } from './CommandPalette'
import { Glyph, Spinner, type GlyphName } from './ui'

type Entry = {
  to: string
  glyph: GlyphName
  label: (t: Dict) => string
  end?: boolean
  /** Whether it earns one of the five places a thumb can reach. */
  tab?: boolean
  /** The part of the administration it belongs to, as the sidebar groups it. */
  group: 'catalogue' | 'system' | 'access'
  /** Shown to an administrator only: an editor keeps the catalogue. */
  admin?: boolean
}

// The bottom bar holds five. Jobs was already off it and the audit trail now
// joins it: both are read-only histories, consulted rather than operated, and
// the dashboard's figures link to each. Importing takes the place that frees,
// because adding a work is an errand somebody actually runs from a phone.
const NAV: Entry[] = [
  { to: '/admin', glyph: 'gauge', label: (t) => t.admin.dashboard, end: true, tab: true, group: 'catalogue' },
  { to: '/admin/catalogue', glyph: 'list', label: (t) => t.admin.catalogue, tab: true, group: 'catalogue' },
  { to: '/admin/discover', glyph: 'discover', label: (t) => t.admin.discover, tab: true, group: 'catalogue' },
  { to: '/admin/lists', glyph: 'list', label: (t) => t.admin.lists, group: 'catalogue' },
  { to: '/admin/jobs', glyph: 'clock', label: (t) => t.admin.jobs, group: 'system', admin: true },
  { to: '/admin/sources', glyph: 'cloud', label: (t) => t.admin.sourcesNav, group: 'system' },
  { to: '/admin/media', glyph: 'image', label: (t) => t.admin.mediaNav, group: 'system', admin: true },
  { to: '/admin/cache', glyph: 'database', label: (t) => t.admin.cacheNav, group: 'system', admin: true },
  { to: '/admin/audit', glyph: 'journal', label: (t) => t.admin.audit, group: 'system', admin: true },
  { to: '/admin/users', glyph: 'user', label: (t) => t.admin.users, group: 'access', admin: true },
  { to: '/admin/access', glyph: 'globe', label: (t) => t.admin.accessPage, group: 'access', admin: true },
  { to: '/admin/clients', glyph: 'key', label: (t) => t.admin.clients, tab: true, group: 'access', admin: true },
  { to: '/admin/settings', glyph: 'settings', label: (t) => t.admin.settings, tab: true, group: 'access', admin: true },
]

const GROUPS = ['catalogue', 'system', 'access'] as const

/** The entries this person may open: an editor does not see the keys to the house. */
function allowed(me: Me) {
  return NAV.filter((entry) => !entry.admin || me.isAdmin)
}

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
  useNavigationReset('admin-main')
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

  // A rejected credential goes to the door. A member signed in has nothing on
  // this side either, but a place of their own: their account.
  if (me.isError) {
    return <Navigate to="/login" replace />
  }
  if (!me.data.canWrite) {
    return <Navigate to={me.data.user ? '/account' : '/login'} replace />
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

      <Sidebar me={me.data} />

      <div className="flex min-w-0 flex-col">
        <TopBar title={override ?? (current ? current.label(t) : t.admin.title)} />

        <main
          id="admin-main"
          tabIndex={-1}
          className="relative z-10 min-w-0 flex-1 px-4 pt-6 pb-28 outline-none sm:px-6 lg:px-10 lg:pt-10 lg:pb-16"
        >
          <TitleContext value={setOverride}>
            <Outlet />
            <CommandPalette admin />
          </TitleContext>
        </main>

        <TabBar me={me.data} />
      </div>
    </div>
  )
}

/* ── Wide ─────────────────────────────────────────────────────────────────── */

function Sidebar({ me }: { me: Me }) {
  const { t } = useI18n()
  const identity = me.identity
  const who = me.user
    ? { name: me.user.name, role: t.labels.roles[me.user.role] }
    : describeIdentity(identity, t)
  const entries = allowed(me)

  // The version is read off the settings, which only an administrator may
  // read; an editor's sidebar goes without it rather than logging a refusal.
  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/settings'),
    staleTime: 5 * 60_000,
    enabled: me.isAdmin,
  })

  // Sign-ups waiting for approval, beside Members: the reason an
  // administrator would open that page today.
  const waiting = useQuery({
    queryKey: ['users', 'waiting'],
    queryFn: () => api.get<UsersPage>('/users?status=pending&limit=1'),
    staleTime: 60_000,
    enabled: me.isAdmin,
  })
  const pending = waiting.data?.counts.pending ?? 0

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
              <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                v{settings.data.version}
              </span>
            ) : null}
          </span>
        </Link>
      </div>

      <nav aria-label={t.admin.sections} className="flex-1 overflow-y-auto py-3">
        {GROUPS.map((group) => {
          const inGroup = entries.filter((entry) => entry.group === group)
          if (!inGroup.length) return null
          return (
            <div key={group} className="mb-2">
              <h2 className="label px-5 pt-3 pb-1.5">{t.admin.groups[group]}</h2>
              {inGroup.map((entry) => (
                <NavRow
                  key={entry.to}
                  entry={entry}
                  badge={entry.to === '/admin/users' && pending > 0 ? pending : undefined}
                />
              ))}
            </div>
          )
        })}
      </nav>

      <div className="space-y-3 border-t border-rule px-5 py-4">
        {me.user ? (
          <NavLink
            to="/admin/account"
            className={({ isActive }) =>
              cn(
                'flex min-h-11 items-center gap-2 text-sm transition-colors duration-150',
                isActive ? 'text-vermillion' : 'text-bone-dim hover:text-bone',
              )
            }
          >
            <Glyph name="user" className="size-4" />
            {t.admin.account}
          </NavLink>
        ) : null}
        <Link
          to="/"
          className="flex min-h-11 items-center gap-2 text-sm text-bone-dim transition-colors duration-150 hover:text-bone"
        >
          <Glyph name="arrowLeft" className="size-4" />
          {t.admin.backToSite}
        </Link>

        <div className="flex flex-wrap items-center gap-2">
          <ThemeToggle />
          <LanguageToggle />
        </div>

        <div className="border-t border-rule pt-3">
          <span className="label block">{t.admin.operator}</span>
          <span className="mt-1 flex min-w-0 items-baseline gap-2">
            <span className="truncate text-sm text-bone" title={identity}>
              {who.name}
            </span>
            {who.role ? <span className="shrink-0 text-xs text-bone-faint">{who.role}</span> : null}
          </span>
          <SignOut className="mt-2 flex min-h-11 items-center gap-2 text-sm text-bone-faint transition-colors duration-150 hover:text-vermillion" />
        </div>
      </div>
    </aside>
  )
}

function NavRow({ entry, badge }: { entry: Entry; badge?: number }) {
  const { t } = useI18n()

  return (
    <NavLink
      to={entry.to}
      end={entry.end}
      className={({ isActive }) =>
        cn(
          'relative flex h-[42px] items-center gap-3 px-5 text-sm font-medium',
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
          {badge ? (
            <span className="ml-auto rounded-full border border-brass/60 px-2 py-0.5 font-mono text-[0.6875rem] text-brass tabular-nums">
              <span className="sr-only">{t.admin.people.pending}: </span>
              {badge}
            </span>
          ) : null}
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

      <ThemeToggle /><LanguageToggle compact />
      <SignOut
        iconOnly
        className="grid size-11 shrink-0 place-items-center rounded-card text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-vermillion"
      />
    </header>
  )
}

function TabBar({ me }: { me: Me }) {
  const { t } = useI18n()
  const tabs = allowed(me).filter((entry) => entry.tab)

  return (
    <nav
      aria-label={t.admin.sections}
      className={cn(
        'fixed inset-x-0 bottom-0 z-40 grid border-t border-rule bg-ink-raised pb-[env(safe-area-inset-bottom)] lg:hidden',
        tabs.length === 5 ? 'grid-cols-5' : 'grid-cols-3',
      )}
    >
      {tabs.map((entry) => (
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
          <span className="line-clamp-2 w-full text-center text-[0.6875rem] leading-tight font-medium">
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
        // was answered for somebody who is now gone. A server that cannot be
        // reached is caught rather than left to reject — signing out still
        // works, and an unhandled rejection would be the only trace of it.
        try {
          await api.post('/auth/logout')
        } catch {
          // Nothing to tell the operator: they are being signed out either way.
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
      {LANGS.map((code) => (
        <button
          key={code}
          type="button"
          onClick={() => setLang(code)}
          aria-pressed={lang === code}
          title={LANGUAGES[code].name}
          lang={code}
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
