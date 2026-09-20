import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router'

import { ApiError } from './lib/api'
import { Shell } from './components/Shell'
import { Audit } from './routes/Audit'
import { Catalogue } from './routes/Catalogue'
import { Clients } from './routes/Clients'
import { Dashboard } from './routes/Dashboard'
import { ItemDetail } from './routes/ItemDetail'
import { Login } from './routes/Login'
import { SettingsPage } from './routes/SettingsPage'

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
      <BrowserRouter>
        <Routes>
          <Route path="/login" element={<Login />} />
          <Route element={<Shell />}>
            <Route path="/" element={<Dashboard />} />
            <Route path="/catalogue" element={<Catalogue />} />
            <Route path="/catalogue/:id" element={<ItemDetail />} />
            <Route path="/clients" element={<Clients />} />
            <Route path="/audit" element={<Audit />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  )
}
