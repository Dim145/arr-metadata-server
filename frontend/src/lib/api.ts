/**
 * Thin fetch wrapper over the native API.
 *
 * Everything is same-origin in production and proxied in development, so the
 * session cookie rides along on its own. A 401 is surfaced as a typed error so
 * the shell can redirect to sign-in rather than each caller handling it.
 */

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }

  get isUnauthorized() {
    return this.status === 401
  }
}

const BASE = '/api/v1'

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${BASE}${path}`, {
    credentials: 'same-origin',
    headers: init?.body ? { 'content-type': 'application/json' } : undefined,
    ...init,
  })

  if (response.status === 204) {
    return undefined as T
  }

  const text = await response.text()
  const body = parse(text)

  if (!response.ok) {
    const detail = body as { error?: string; message?: string } | null
    throw new ApiError(
      response.status,
      detail?.error ?? 'unknown',
      detail?.message ?? response.statusText,
    )
  }

  if (text && body === null) {
    throw new ApiError(response.status, 'unreadable', response.statusText)
  }

  return body as T
}

/**
 * Read a body that is supposed to be JSON, without trusting that it is.
 *
 * Not everything that answers is this server: a reverse proxy in front of it
 * returns an HTML error page, and a captive portal returns a login form. Letting
 * `JSON.parse` throw there would hand the caller a `SyntaxError` carrying no
 * status, so a 401 behind a proxy would be retried as if it were a network
 * blip and reported as "something went wrong" rather than "sign in again".
 */
function parse(text: string): unknown {
  if (!text) {
    return null
  }

  try {
    return JSON.parse(text) as unknown
  } catch {
    return null
  }
}

const json = (body: unknown): RequestInit => ({ body: JSON.stringify(body) })

export const api = {
  get: <T,>(path: string) => request<T>(path),
  post: <T,>(path: string, body?: unknown) =>
    request<T>(path, { method: 'POST', ...(body === undefined ? {} : json(body)) }),
  put: <T,>(path: string, body: unknown) => request<T>(path, { method: 'PUT', ...json(body) }),
  patch: <T,>(path: string, body: unknown) =>
    request<T>(path, { method: 'PATCH', ...json(body) }),
  delete: <T,>(path: string) => request<T>(path, { method: 'DELETE' }),
  /** A form with a file in it: the browser sets the boundary itself. */
  upload: <T,>(path: string, form: FormData) => request<T>(path, { method: 'POST', body: form, headers: {} }),
}

/** Build a query string, dropping empty values so the URL stays readable. */
export function query(params: Record<string, string | number | boolean | undefined>) {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== '' && value !== false) {
      search.set(key, String(value))
    }
  }
  const encoded = search.toString()
  return encoded ? `?${encoded}` : ''
}
