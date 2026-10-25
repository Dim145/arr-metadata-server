import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query'
import { lazy, Suspense } from 'react'
import { BrowserRouter, Navigate, Route, Routes, useLocation, type NavigateOptions, type To } from 'react-router'

import { PageBoundary } from './components/PageBoundary'
import { PublicShell } from './components/PublicShell'
import { Spinner } from './components/ui'
import { ApiError, api } from './lib/api'
import { useAuthOptions } from './lib/hooks'
import { I18nProvider } from './lib/i18n'
import type { Me } from './lib/types'
import { Browse } from './routes/Browse'
import { Home } from './routes/Home'
import { NotFound } from './routes/NotFound'
import { Work } from './routes/Work'

// Everything a visitor never opens is fetched when somebody does. The whole
// administration side used to ship in the one bundle every catalogue reader
// downloaded — half a megabyte, most of it screens behind a sign-in they would
// never pass. The catalogue itself stays eager: it is what the page is for.
const named = <K extends string>(load: () => Promise<Record<K, React.ComponentType>>, name: K) =>
  lazy(() => load().then((module) => ({ default: module[name] })))

// The catalogue's deeper pages: reached from a work, not the way in.
const Season = named(() => import('./routes/Season'), 'Season')
const Episode = named(() => import('./routes/Episode'), 'Episode')
const Person = named(() => import('./routes/Person'), 'Person')
const People = named(() => import('./routes/People'), 'People')
const Calendar = named(() => import('./routes/Calendar'), 'Calendar')
const Seasons = named(() => import('./routes/Seasons'), 'Seasons')
const Lists = named(() => import('./routes/Lists'), 'Lists')
const Figures = named(() => import('./routes/Figures'), 'Figures')
const ListDetail = named(() => import('./routes/Lists'), 'ListDetail')
// The catalogue by a genre, a network, a year or a decade, and two works side by side.
const GenrePage = named(() => import('./routes/Facet'), 'GenrePage')
const NetworkPage = named(() => import('./routes/Facet'), 'NetworkPage')
const YearPage = named(() => import('./routes/Facet'), 'YearPage')
const DecadePage = named(() => import('./routes/Facet'), 'DecadePage')
const Compare = named(() => import('./routes/Compare'), 'Compare')
const CollectionIndex = named(() => import('./routes/Lists'), 'CollectionIndex')
const CollectionDetail = named(() => import('./routes/Lists'), 'CollectionDetail')

const Login = named(() => import('./routes/Login'), 'Login')
const Register = named(() => import('./routes/Register'), 'Register')
const Account = named(() => import('./routes/Account'), 'Account')
const Users = named(() => import('./routes/admin/Users'), 'Users')
const Access = named(() => import('./routes/admin/Access'), 'Access')
const UserDetail = named(() => import('./routes/admin/UserDetail'), 'UserDetail')
const AdminShell = named(() => import('./components/AdminShell'), 'AdminShell')
const Dashboard = named(() => import('./routes/admin/Dashboard'), 'Dashboard')
const Catalogue = named(() => import('./routes/admin/Catalogue'), 'Catalogue')
const AdminPeople = named(() => import('./routes/admin/People'), 'People')
const WorkEditor = named(() => import('./routes/admin/WorkEditor'), 'WorkEditor')
const Discover = named(() => import('./routes/admin/Discover'), 'Discover')
const Clients = named(() => import('./routes/admin/Clients'), 'Clients')
const AdminLists = named(() => import('./routes/admin/Lists'), 'Lists')
const Jobs = named(() => import('./routes/admin/Jobs'), 'Jobs')
const Sources = named(() => import('./routes/admin/Sources'), 'Sources')
const Media = named(() => import('./routes/admin/Media'), 'Media')
const Cache = named(() => import('./routes/admin/Cache'), 'Cache')
const Backups = named(() => import('./routes/admin/Backups'), 'Backups')
const Audit = named(() => import('./routes/admin/Audit'), 'Audit')
const Logs = named(() => import('./routes/admin/Logs'), 'Logs')
const Notifications = named(() => import('./routes/admin/Notifications'), 'Notifications')
const Settings = named(() => import('./routes/admin/Settings'), 'Settings')
const Arr = named(() => import('./routes/admin/Arr'), 'Arr')
const Editorial = named(() => import('./routes/admin/Editorial'), 'Editorial')

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
      // A rejected credential will not start working on its own; retrying only
      // delays the redirect to sign-in.
      // Nor will a 404 start existing: a removed work used to take three tries
      // and their back-off before the page said it was gone.
      retry: (failureCount, error) =>
        !(error instanceof ApiError && [401, 403, 404].includes(error.status)) &&
        failureCount < 2,
    },
  },
})

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <I18nProvider>
        <BrowserRouter>
          {/* Around everything the router draws, so that a shell, the sign-in
              page or a screen's own chunk failing is caught too; each shell
              has one of its own around the page, which is what keeps the bar
              on screen when only a page throws. */}
          <PageBoundary>
            <Router />
          </PageBoundary>
        </BrowserRouter>
      </I18nProvider>
    </QueryClientProvider>
  )
}

function Router() {
  // Who is looking. A visitor gets the catalogue and nothing else; the answer
  // decides whether the bar offers "sign in" or "admin".
  const me = useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<Me>('/auth/me'),
    retry: false,
    staleTime: 5 * 60_000,
  })
  const options = useAuthOptions()

  // A private site sends a stranger to the door before a page asks the server
  // for what it would refuse them. Decided once both answers are in; a site
  // that cannot say (offline, say) is treated as open and lets the pages speak.
  const deciding = options.isPending || (options.data?.site === 'private' && me.isPending)
  const locked =
    options.data?.site === 'private' && me.error instanceof ApiError && me.error.isUnauthorized

  return (
    <Suspense fallback={<Loading />}>
      <Routes>
        <Route element={deciding ? <Loading /> : locked ? <ToSignIn /> : <PublicShell me={me.data} />}>
          <Route path="/" element={<Home />} />
          <Route path="/browse" element={<Browse />} />
          <Route path="/work/:id" element={<Work />} />
          <Route path="/work/:id/season/:season" element={<Season />} />
          <Route path="/work/:id/season/:season/episode/:episode" element={<Episode />} />
          {/* One page for a person, by their record or by TMDB's id, as the
              pages were addressed before there were records. */}
          <Route path="/person/:tmdbId" element={<Person />} />
          <Route path="/people" element={<People />} />
          <Route path="/people/:id" element={<Person />} />
          <Route path="/calendar" element={<Calendar />} />
          <Route path="/lists" element={<Lists />} />
          <Route path="/stats" element={<Figures />} />
          <Route path="/lists/:slug" element={<ListDetail />} />
          <Route path="/collections" element={<CollectionIndex />} />
          <Route path="/collections/:id" element={<CollectionDetail />} />
          <Route path="/seasons" element={<Seasons />} />
          <Route path="/seasons/:year/:season" element={<Seasons />} />
          <Route path="/genre/:slug" element={<GenrePage />} />
          <Route path="/network/:slug" element={<NetworkPage />} />
          <Route path="/year/:slug" element={<YearPage />} />
          <Route path="/decade/:slug" element={<DecadePage />} />
          <Route path="/compare" element={<Compare />} />
          <Route path="/account" element={<Account />} />
          {/* Said, not redirected: see `NotFound`. */}
          <Route path="*" element={<NotFound />} />
        </Route>

        <Route path="/login" element={<Login />} />
        <Route path="/register" element={<Register />} />

        {/* The shell is the guard: it asks who is signed in once, and sends
            anyone who may not write to the door before a screen mounts. */}
        <Route path="/admin" element={<AdminShell />}>
          <Route index element={<Dashboard />} />
          <Route path="catalogue" element={<Catalogue />} />
          <Route path="catalogue/:id" element={<WorkEditor />} />
          <Route path="people" element={<AdminPeople />} />
          <Route path="discover" element={<Discover />} />
          <Route path="clients" element={<Clients />} />
          <Route path="users" element={<Users />} />
          <Route path="users/:id" element={<UserDetail />} />
          <Route path="access" element={<Access />} />
          <Route path="account" element={<Account />} />
          <Route path="lists" element={<AdminLists />} />
          <Route path="jobs" element={<Jobs />} />
          <Route path="sources" element={<Sources />} />
          <Route path="media" element={<Media />} />
          <Route path="cache" element={<Cache />} />
          <Route path="backups" element={<Backups />} />
          <Route path="audit" element={<Audit />} />
          <Route path="logs" element={<Logs />} />
          <Route path="notifications" element={<Notifications />} />
          <Route path="settings" element={<Settings />} />
          <Route path="arr" element={<Arr />} />
          <Route path="editorial" element={<Editorial />} />
          <Route path="*" element={<NotFound admin />} />
        </Route>
      </Routes>
    </Suspense>
  )
}

/** To the sign-in page, and back here once through it. */
function ToSignIn() {
  const location = useLocation()
  const here = `${location.pathname}${location.search}`

  return <Navigate to={here === '/' ? '/login' : `/login?next=${encodeURIComponent(here)}`} replace />
}

/** While a screen that was not in the first bundle arrives. */
function Loading() {
  return (
    <div className="grid min-h-dvh place-items-center">
      <Spinner className="size-6 text-bone-dim" />
    </div>
  )
}

// Under `<BrowserRouter>`, `navigate` returns nothing: the promise its type
// allows for is the data routers', which this interface does not use. Said
// here, as React Router's own documentation has it, so that a page calls it
// without a `void` and the lint still flags a promise that is left floating.
declare module 'react-router' {
  interface NavigateFunction {
    (to: To, options?: NavigateOptions): void
    (delta: number): void
  }
}
