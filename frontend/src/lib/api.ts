/**
 * Thin fetch wrapper over the native API.
 *
 * Everything is same-origin in production and proxied in development, so the
 * session cookie rides along on its own. A 401 is surfaced as a typed error so
 * the shell can redirect to sign-in rather than each caller handling it.
 */

import { textLanguage } from './prefs'

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

/**
 * A read in the language the reader chose for the catalogue's texts, where
 * they chose one apart from the interface's: whatever asks for a work, a
 * season, a person, the schedule or a list in the interface's language is
 * answered in that one instead, and the pages need not know. Here rather
 * than in every page, which each ask in their own words.
 *
 * Only the reader's side of the site: the administration asks in the
 * language it is editing, which is not the reader's to change. And only a
 * read that asks for a language at all: one that did not wants none.
 */
export function inTextLanguage(path: string, chosen = textLanguage(), where = currentPath()): string {
  if (!chosen || where.startsWith('/admin')) return path
  const at = path.indexOf('?')
  if (at < 0) return path
  const params = new URLSearchParams(path.slice(at + 1))
  if (!params.has('language')) return path
  params.set('language', chosen)
  return `${path.slice(0, at)}?${params.toString()}`
}

/** Where the page is, which decides whose language a read is asked in. */
function currentPath(): string {
  return typeof window === 'undefined' ? '/' : window.location.pathname
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const reading = !init?.method || init.method === 'GET'
  const response = await fetch(`${BASE}${reading ? inTextLanguage(path) : path}`, {
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

/**
 * A value that came out of the address, as one segment of an API path.
 *
 * Encoded, so a `/`, a `?` or a `#` in it stays inside the segment: the
 * router hands `/work/..%2Fstats` over as `../stats`, and put into a path as
 * it was, that asked the API for `/stats` and showed the answer as a work.
 *
 * A segment of dots alone is refused outright, as the "no such thing" it is.
 * An address cannot hand one over — the browser resolves `..` in the address
 * itself, `%2e` spellings too, before the router sees it — but encoding could
 * not keep one from a value that came by another way: sent, it is resolved
 * the same, and names the directory above.
 */
export function segment(value: string | number): string {
  const text = String(value)
  if (/^(?:\.|%2e)+$/i.test(text)) {
    throw new ApiError(404, 'not_found', 'Not Found')
  }
  return encodeURIComponent(text)
}

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
