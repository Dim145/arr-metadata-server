/**
 * A Sonarr small enough to read, for the tests of the Sonarr & Radarr page.
 *
 * It answers the few calls the server makes of a real one — its status, a
 * series found by its TheTVDB id, the whole library, a command — checks the
 * key on each, and keeps every command it was sent so a test can say what
 * reached it. It listens on a free port of 127.0.0.1: the server reaches its
 * operator's Sonarr on this machine too, as a Sonarr installed beside it is.
 */

import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'

export interface Held {
  /** Sonarr's own id for the series. */
  id: number
  title: string
  tvdbId: number
}

export interface ArrStub {
  url: string
  key: string
  /** Every command received, as its body said it. */
  commands: { name: string; seriesIds?: number[] }[]
  close(): Promise<void>
}

function body(request: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let text = ''
    request.on('data', (chunk) => (text += chunk))
    request.on('end', () => resolve(text))
    request.on('error', reject)
  })
}

function json(response: ServerResponse, status: number, value: unknown) {
  response.writeHead(status, { 'content-type': 'application/json' })
  response.end(JSON.stringify(value))
}

export async function startSonarr(library: Held[]): Promise<ArrStub> {
  const key = `e2e-${Math.random().toString(36).slice(2)}${Math.random().toString(36).slice(2)}`
  const commands: ArrStub['commands'] = []

  const answer = async (request: IncomingMessage, response: ServerResponse) => {
    if (request.headers['x-api-key'] !== key) return json(response, 401, { message: 'Unauthorized' })
    const url = new URL(request.url ?? '/', 'http://stub')

    if (request.method === 'GET' && url.pathname === '/api/v3/system/status') {
      return json(response, 200, { appName: 'Sonarr', instanceName: 'E2E', version: '4.0.15.2941' })
    }
    if (request.method === 'GET' && url.pathname === '/api/v3/series') {
      const tvdb = url.searchParams.get('tvdbId')
      return json(response, 200, tvdb === null ? library : library.filter((held) => String(held.tvdbId) === tvdb))
    }
    if (request.method === 'POST' && url.pathname === '/api/v3/command') {
      const command = JSON.parse(await body(request)) as ArrStub['commands'][number]
      commands.push(command)
      return json(response, 201, { id: commands.length, name: command.name, status: 'queued' })
    }
    return json(response, 404, { message: 'NotFound' })
  }
  // Answered in its own time: the listener's return is waited on by nobody.
  const server = createServer((request, response) => void answer(request, response))

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address() as AddressInfo

  return {
    url: `http://127.0.0.1:${port}`,
    key,
    commands,
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  }
}
