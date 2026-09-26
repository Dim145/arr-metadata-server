/**
 * An identity provider small enough to read, for the sign-in tests.
 *
 * It speaks just enough OpenID Connect for the server to trust it: a
 * discovery document, a key set, an authorization endpoint that signs in
 * whoever the test says without asking, a token endpoint that checks the
 * client's secret and the PKCE verifier and hands back an RS256 ID token
 * with its `at_hash`, and a user-info endpoint. Node's own crypto signs; no
 * library stands between the test and what the server is sent.
 */

import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'

export interface Person {
  sub: string
  preferred_username?: string
  email?: string
  email_verified?: boolean
  name?: string
  groups?: string[]
}

export interface Provider {
  issuer: string
  clientId: string
  clientSecret: string
  /** Who the next sign-in is, or a refusal. */
  signInAs(person: Person | 'denied'): void
  close(): Promise<void>
}

const b64url = (data: Buffer | string) => Buffer.from(data).toString('base64url')

function body(request: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let text = ''
    request.on('data', (chunk) => (text += chunk))
    request.on('end', () => resolve(text))
    request.on('error', reject)
  })
}

function json(response: ServerResponse, status: number, value: unknown) {
  response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' })
  response.end(JSON.stringify(value))
}

export async function startProvider(): Promise<Provider> {
  const clientId = 'cinematheque-e2e'
  const clientSecret = randomBytes(16).toString('hex')
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 })
  const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'e2e-1', alg: 'RS256', use: 'sig' }

  let issuer = ''
  let next: Person | 'denied' = { sub: 'nobody' }
  const codes = new Map<string, { person: Person; nonce: string; challenge: string; redirect: string }>()
  const tokens = new Map<string, Person>()

  const server = createServer(async (request, response) => {
    const url = new URL(request.url ?? '/', issuer)

    if (url.pathname === '/.well-known/openid-configuration') {
      return json(response, 200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        userinfo_endpoint: `${issuer}/userinfo`,
        jwks_uri: `${issuer}/jwks`,
        response_types_supported: ['code'],
        subject_types_supported: ['public'],
        id_token_signing_alg_values_supported: ['RS256'],
        scopes_supported: ['openid', 'profile', 'email', 'groups'],
        token_endpoint_auth_methods_supported: ['client_secret_basic', 'client_secret_post'],
        code_challenge_methods_supported: ['S256'],
      })
    }

    if (url.pathname === '/jwks') return json(response, 200, { keys: [jwk] })

    if (url.pathname === '/authorize') {
      const redirect = url.searchParams.get('redirect_uri') ?? ''
      const back = new URL(redirect)
      back.searchParams.set('state', url.searchParams.get('state') ?? '')
      if (next === 'denied' || url.searchParams.get('client_id') !== clientId) {
        back.searchParams.set('error', 'access_denied')
      } else if (url.searchParams.get('code_challenge_method') !== 'S256') {
        back.searchParams.set('error', 'invalid_request')
      } else {
        const code = randomBytes(16).toString('hex')
        codes.set(code, {
          person: next,
          nonce: url.searchParams.get('nonce') ?? '',
          challenge: url.searchParams.get('code_challenge') ?? '',
          redirect,
        })
        back.searchParams.set('code', code)
      }
      response.writeHead(302, { location: back.toString() })
      return response.end()
    }

    if (url.pathname === '/token' && request.method === 'POST') {
      const form = new URLSearchParams(await body(request))
      const basic = /^Basic (.+)$/.exec(request.headers.authorization ?? '')
      const [id, secret] = basic
        ? Buffer.from(basic[1], 'base64').toString().split(':').map(decodeURIComponent)
        : [form.get('client_id'), form.get('client_secret')]
      if (id !== clientId || secret !== clientSecret) return json(response, 401, { error: 'invalid_client' })

      const grant = codes.get(form.get('code') ?? '')
      codes.delete(form.get('code') ?? '')
      const verifier = form.get('code_verifier') ?? ''
      if (
        !grant ||
        grant.redirect !== form.get('redirect_uri') ||
        b64url(createHash('sha256').update(verifier).digest()) !== grant.challenge
      ) {
        return json(response, 400, { error: 'invalid_grant' })
      }

      const access = randomBytes(24).toString('hex')
      tokens.set(access, grant.person)
      const now = Math.floor(Date.now() / 1000)
      const header = b64url(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid: jwk.kid }))
      const claims = b64url(
        JSON.stringify({
          iss: issuer,
          aud: clientId,
          iat: now,
          exp: now + 300,
          nonce: grant.nonce,
          // The left half of the access token's SHA-256: what binds the two.
          at_hash: b64url(createHash('sha256').update(access).digest().subarray(0, 16)),
          ...grant.person,
        }),
      )
      const signature = b64url(sign('sha256', Buffer.from(`${header}.${claims}`), privateKey))
      return json(response, 200, {
        access_token: access,
        token_type: 'Bearer',
        expires_in: 300,
        id_token: `${header}.${claims}.${signature}`,
      })
    }

    if (url.pathname === '/userinfo') {
      const person = tokens.get((request.headers.authorization ?? '').replace(/^Bearer /, ''))
      return person ? json(response, 200, person) : json(response, 401, { error: 'invalid_token' })
    }

    response.writeHead(404)
    response.end()
  })

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  issuer = `http://127.0.0.1:${(server.address() as AddressInfo).port}`

  return {
    issuer,
    clientId,
    clientSecret,
    signInAs: (person) => {
      next = person
    },
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  }
}
