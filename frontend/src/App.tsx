import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router'

import { AdminShell } from './components/AdminShell'
import { PublicShell } from './components/PublicShell'
import { ApiError, api } from './lib/api'
import { I18nProvider } from './lib/i18n'
import type { Me } from './lib/types'
import { Browse } from './routes/Browse'
import { Home } from './routes/Home'
import { Login } from './routes/Login'
import { Work } from './routes/Work'
import { Audit } from './routes/admin/Audit'
import { Catalogue } from './routes/admin/Catalogue'
import { Clients } from './routes/admin/Clients'
import { Dashboard } from './routes/admin/Dashboard'
import { Jobs } from './routes/admin/Jobs'
import { Settings } from './routes/admin/Settings'
import { WorkEditor } from './routes/admin/WorkEditor'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
      // A rejected credential will not start working on its own; retrying only
      // delays the redirect to sign-in.
      retry: (failureCount, error) =>
        !(error instanceof ApiError && (error.isUnauthorized || error.status === 403)) &&
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
    <Routes>
      <Route element={<PublicShell me={me.data} />}>
        <Route path="/" element={<Home />} />
        <Route path="/browse" element={<Browse />} />
        <Route path="/work/:id" element={<Work />} />
      </Route>

      <Route path="/login" element={<Login />} />

      {/* The shell is the guard: it asks who is signed in once, and sends
          anyone who may not write to the door before a screen mounts. */}
      <Route path="/admin" element={<AdminShell />}>
        <Route index element={<Dashboard />} />
        <Route path="catalogue" element={<Catalogue />} />
        <Route path="catalogue/:id" element={<WorkEditor />} />
        <Route path="clients" element={<Clients />} />
        <Route path="jobs" element={<Jobs />} />
        <Route path="audit" element={<Audit />} />
        <Route path="settings" element={<Settings />} />
      </Route>

      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  )
}
