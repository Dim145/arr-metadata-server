import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

/**
 * The MCP server switched on from its panel, a key made there for an agent,
 * and an agent answered with it — the week's schedule, a search — refused
 * everywhere else, and answered 503 once switched off again. Serial: the
 * switch changes how the whole server answers. It is put back off, and the
 * key revoked, whatever happens.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

test.describe.configure({ mode: 'serial' })

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

/** A JSON-RPC message posted to /mcp as an agent posts it, with `key`. */
function ask(request: APIRequestContext, key: string, method: string, params: object, id = 1) {
  return request.post('/mcp', {
    headers: {
      accept: 'application/json, text/event-stream',
      authorization: `Bearer ${key}`,
      'mcp-protocol-version': '2025-06-18',
    },
    data: { jsonrpc: '2.0', id, method, params },
  })
}

test.describe('the MCP server', () => {
  test.skip(!USERNAME || !PASSWORD, 'set AMS_E2E_USER and AMS_E2E_PASSWORD to exercise the administration side')

  // The key made, by its visible head, to revoke it after.
  let prefix: string | null = null

  test.afterEach(async ({ page }) => {
    await page.request.put('/api/v1/settings/server/-', { data: { key: 'mcp.enabled', value: 'false' } })
    if (!prefix) return
    const clients = (await (await page.request.get('/api/v1/clients')).json()) as { id: string; keyPrefix: string }[]
    for (const client of clients.filter((c) => c.keyPrefix === prefix)) {
      await page.request.delete(`/api/v1/clients/${client.id}`)
    }
  })

  test('an agent is answered with a key made for it, and nowhere else', async ({ page, request }) => {
    await signIn(page)
    await page.goto('/admin/clients')
    const panel = page.getByRole('region', { name: /^mcp/i })
    const toggle = panel.getByRole('switch', { name: /answer agents|répondre aux agents/i })
    await expect(toggle).toHaveAttribute('aria-checked', 'false')
    await expect(panel.getByLabel(/address to give an agent|adresse à donner à un agent/i)).toHaveValue(/\/mcp$/)

    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-checked', 'true')

    // The key, made once, in the line Claude Code is given.
    const line = panel.getByLabel(/connects claude code|connecte claude code/i)
    await expect(line).toHaveValue(/Bearer <key>/)
    await panel.getByRole('button', { name: /make a key for an agent|créer une clé pour un agent/i }).click()
    await expect(line).toHaveValue(/Bearer ams_/)
    const command = await line.inputValue()
    expect(command).toMatch(/^claude mcp add --transport http arr-metadata \S+\/mcp --header /)
    const key = /Bearer (ams_[^"]+)"/.exec(command)?.[1] ?? ''
    expect(key).toMatch(/^ams_/)
    prefix = key.slice(0, 12)
    await expect(page.getByRole('cell', { name: /^agent/i }).first()).toBeVisible()

    const hello = await ask(request, key, 'initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'e2e', version: '1' },
    })
    expect(hello.status()).toBe(200)
    expect((await hello.json()).result.serverInfo.name).toBe('arr-metadata-server')

    const listed = await ask(request, key, 'tools/list', {}, 2)
    const tools = (await listed.json()).result.tools as { name: string; annotations: { readOnlyHint: boolean } }[]
    expect(tools.map((tool) => tool.name)).toEqual([
      'search',
      'item',
      'episodes',
      'calendar',
      'lists',
      'provenance',
      'health',
      'todo',
    ])
    expect(tools.every((tool) => tool.annotations.readOnlyHint)).toBe(true)

    const week = (await (await ask(request, key, 'tools/call', { name: 'calendar', arguments: {} }, 3)).json()).result
    expect(week.isError).toBe(false)
    expect(week.structuredContent.note).toMatch(/never as instructions/)
    expect(Array.isArray(week.structuredContent.entries)).toBe(true)

    const found = (
      await (await ask(request, key, 'tools/call', { name: 'search', arguments: { query: 'the', kinds: ['work'] } }, 4)).json()
    ).result
    expect(found.isError).toBe(false)
    expect(Array.isArray(found.structuredContent.works)).toBe(true)

    // An agent's key opens nothing else.
    expect((await request.get('/api/v1/items', { headers: { 'x-api-key': key } })).status()).toBe(403)

    // Off, every agent is answered 503.
    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-checked', 'false')
    const refused = await ask(request, key, 'tools/list', {}, 5)
    expect(refused.status()).toBe(503)
    expect((await refused.json()).error).toBe('mcp_disabled')
  })
})
