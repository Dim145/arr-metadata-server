import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query'
import { lazy, Suspense } from 'react'
import { BrowserRouter, Route, Routes } from 'react-router'

import { PublicShell } from './components/PublicShell'
import { Spinner } from './components/ui'
import { ApiError, api } from './lib/api'
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
const Calendar = named(() => import('./routes/Calendar'), 'Calendar')

const Login = named(() => import('./routes/Login'), 'Login')
const AdminShell = named(() => import('./components/AdminShell'), 'AdminShell')
const Dashboard = named(() => import('./routes/admin/Dashboard'), 'Dashboard')
const Catalogue = named(() => import('./routes/admin/Catalogue'), 'Catalogue')
const WorkEditor = named(() => import('./routes/admin/WorkEditor'), 'WorkEditor')
const Discover = named(() => import('./routes/admin/Discover'), 'Discover')
const Clients = named(() => import('./routes/admin/Clients'), 'Clients')
const Jobs = named(() => import('./routes/admin/Jobs'), 'Jobs')
const Audit = named(() => import('./routes/admin/Audit'), 'Audit')
const Settings = named(() => import('./routes/admin/Settings'), 'Settings')

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
          <Router />
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

  return (
    <Suspense fallback={<Loading />}>
      <Routes>
        <Route element={<PublicShell me={me.data} />}>
          <Route path="/" element={<Home />} />
          <Route path="/browse" element={<Browse />} />
          <Route path="/work/:id" element={<Work />} />
          <Route path="/work/:id/season/:season" element={<Season />} />
          <Route path="/work/:id/season/:season/episode/:episode" element={<Episode />} />
          <Route path="/person/:tmdbId" element={<Person />} />
          <Route path="/calendar" element={<Calendar />} />
          {/* Said, not redirected: see `NotFound`. */}
          <Route path="*" element={<NotFound />} />
        </Route>

        <Route path="/login" element={<Login />} />

        {/* The shell is the guard: it asks who is signed in once, and sends
            anyone who may not write to the door before a screen mounts. */}
        <Route path="/admin" element={<AdminShell />}>
          <Route index element={<Dashboard />} />
          <Route path="catalogue" element={<Catalogue />} />
          <Route path="catalogue/:id" element={<WorkEditor />} />
          <Route path="discover" element={<Discover />} />
          <Route path="clients" element={<Clients />} />
          <Route path="jobs" element={<Jobs />} />
          <Route path="audit" element={<Audit />} />
          <Route path="settings" element={<Settings />} />
          <Route path="*" element={<NotFound admin />} />
        </Route>
      </Routes>
    </Suspense>
  )
}

/** While a screen that was not in the first bundle arrives. */
function Loading() {
  return (
    <div className="grid min-h-dvh place-items-center">
      <Spinner className="size-6 text-bone-dim" />
    </div>
  )
}
