import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

import { scan } from './support/axe'
import { png } from './support/png'
import { escaped, pattern } from './support/regex'

/**
 * The copies of the pictures kept that the pages are given: none until the
 * WebP copies are turned on, the pictures themselves meanwhile; on, the
 * pages ask for them; turned off while some are kept, the Media page asks
 * what becomes of them, and keeps them or deletes them as it is told.
 *
 * Needs a store, as the media spec does: without one the test says so and
 * skips. Server-wide, because the setting is the whole server's and a
 * deletion takes every WebP copy kept. It puts the setting back as it found
 * it, and takes its picture away.
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

interface Status {
  backend: string
  webp: boolean
  purging: boolean
  variants: { webp: number }
}

async function status(request: APIRequestContext): Promise<Status> {
  const answer = await request.get('/api/v1/media/status')
  expect(answer.ok()).toBe(true)
  return (await answer.json()) as Status
}

/** The server's own value of a setting, or nothing where it is not set. */
async function settingNow(request: APIRequestContext, key: string): Promise<string | undefined> {
  const answer = await request.get('/api/v1/settings/server/-')
  expect(answer.ok()).toBe(true)
  return ((await answer.json()) as { key: string; value: string }[]).find((setting) => setting.key === key)?.value
}

/** A setting given a value, or — `null` — not set at all, as it starts. */
async function setting(request: APIRequestContext, key: string, value: string | null) {
  const answer = await request.put('/api/v1/settings/server/-', { data: { key, value } })
  expect(answer.ok(), `${key}=${value}`).toBe(true)
}

/** A film whose poster nobody chose or added by hand: one added now leads it. */
async function plainFilm(request: APIRequestContext): Promise<{ id: string; title: string } | undefined> {
  const list = (await (await request.get('/api/v1/items?kind=movie&limit=40')).json()) as { items: { id: string }[] }
  for (const { id } of list.items) {
    const work = (await (await request.get(`/api/v1/items/${id}`)).json()) as {
      title: string
      primaryImages?: { poster?: string }
      images?: { coverType: string; isManual: boolean }[]
    }
    if (work.primaryImages?.poster) continue
    if ((work.images ?? []).some((image) => image.coverType === 'poster' && image.isManual)) continue
    return { id, title: work.title }
  }
  return undefined
}

test.describe('the copies the pages are given', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('are the pictures themselves until the WebP copies are on, and are kept or deleted when they go off', async ({
    page,
    browser,
  }) => {
    test.slow()
    await signIn(page)
    test.skip((await status(page.request)).backend === 'off', 'no media store is configured on this server')

    const film = await plainFilm(page.request)
    test.skip(!film, 'no film whose poster nobody chose')
    const { id } = film!

    const was = await settingNow(page.request, 'media.webp')
    let assetId: string | undefined
    try {
      await setting(page.request, 'media.webp', 'false')

      // A poster of this run's own, put on the film by hand: it leads it.
      const uploaded = await page.request.post(`/api/v1/items/${id}/media`, {
        multipart: {
          file: { name: 'poster.png', mimeType: 'image/png', buffer: png(600, 900 + (Date.now() % 89)) },
          coverType: 'poster',
        },
      })
      expect(uploaded.status()).toBe(201)
      const sent = (await uploaded.json()) as { assetId: string; url: string }
      assetId = sent.assetId
      const { url } = sent
      expect(url).toMatch(/^\/media\/[0-9a-f]{64}\.png$/)
      const stem = url.replace(/\.png$/, '')
      const copy = pattern(`${escaped(stem)}-\\d+\\.webp$`)

      // Off: a visitor's page draws the picture itself, at no other width.
      const before = await browser.newContext()
      try {
        const them = await before.newPage()
        const asked: string[] = []
        them.on('request', (request) => {
          if (copy.test(new URL(request.url()).pathname)) asked.push(request.url())
        })
        await them.goto(`/work/${id}`)
        const poster = them.locator(`img[src^="${stem}"]`).first()
        await expect(poster).toHaveAttribute('src', url)
        expect(await poster.getAttribute('srcset')).toBeNull()
        await expect.poll(() => poster.evaluate((img) => (img as HTMLImageElement).naturalWidth)).toBe(600)
        expect(asked, 'no WebP copy is asked for while they are off').toEqual([])
      } finally {
        await before.close()
      }

      // On: a page opened now asks for the copies, from its first picture —
      // never for the picture itself — and each is made as it is asked for.
      await setting(page.request, 'media.webp', 'true')
      const after = await browser.newContext()
      try {
        const them = await after.newPage()
        const whole: string[] = []
        them.on('request', (request) => {
          if (new URL(request.url()).pathname === url) whole.push(request.url())
        })
        const made = them.waitForResponse((response) => copy.test(new URL(response.url()).pathname))
        await them.goto(`/work/${id}`)
        const answer = await made
        expect(answer.status()).toBe(200)
        expect(answer.headers()['content-type']).toBe('image/webp')
        const poster = them.locator(`img[src^="${stem}"]`).first()
        await expect(poster).toHaveAttribute('src', `${stem}-480.webp`)
        await expect(poster).toHaveAttribute('srcset', pattern(`${escaped(stem)}-480\\.webp 480w`))
        expect(whole, 'the picture itself is not asked for').toEqual([])
      } finally {
        await after.close()
      }
      await expect.poll(async () => (await status(page.request)).variants.webp).toBeGreaterThan(0)

      // Off again from the Media page, with copies kept: it asks first.
      await page.goto('/admin/media')
      const webp = page.getByRole('switch', { name: /^(webp copies for the pages|copies webp pour les pages)$/i })
      const dialog = page.getByRole('dialog', { name: /turn off the webp copies|désactiver les copies webp/i })
      await expect(webp).toBeChecked()
      await webp.click()
      await expect(dialog).toBeVisible()
      await expect(dialog).toContainText(/webp cop(y|ies) (is|are) kept|copies? webp (est|sont) stockées?/i)
      await scan(page, 'the Media page, asking what becomes of the WebP copies')

      // Cancelled: nothing changes.
      await dialog.getByRole('button', { name: /^(cancel|annuler)$/i }).click()
      await expect(dialog).toBeHidden()
      await expect(webp).toBeChecked()
      expect((await status(page.request)).webp).toBe(true)

      // Turned off, the copies kept.
      const kept = (await status(page.request)).variants.webp
      await webp.click()
      await dialog.getByRole('button', { name: /turn off and keep|désactiver et (la|les) garder/i }).click()
      await expect(dialog).toBeHidden()
      await expect(webp).not.toBeChecked()
      const off = await status(page.request)
      expect(off.webp).toBe(false)
      expect(off.variants.webp).toBe(kept)

      // On again — no question asked — then off, the copies deleted: every
      // one of them, in the background, the picture itself kept.
      await webp.click()
      await expect(webp).toBeChecked()
      expect((await status(page.request)).webp).toBe(true)
      await webp.click()
      await expect(dialog).toBeVisible()
      await dialog.getByRole('button', { name: /turn off and delete|désactiver et (la|les) supprimer/i }).click()
      await expect(dialog).toBeHidden()
      await expect(page.getByRole('status').filter({ hasText: /^(deleting|suppression de) .*webp/i })).toBeVisible()
      await expect(webp).not.toBeChecked()
      await expect
        .poll(
          async () => {
            const now = await status(page.request)
            return !now.purging && now.variants.webp === 0
          },
          { timeout: 30_000 },
        )
        .toBe(true)
      expect((await page.request.get(url)).status()).toBe(200)
      // Asked for anyway, a copy no longer kept is the picture, for now.
      const gone = await page.request.get(`${stem}-480.webp`, { maxRedirects: 0 })
      expect(gone.status()).toBe(302)
      expect(gone.headers()['location']).toMatch(pattern(`${escaped(url)}$`))
    } finally {
      if (assetId) await page.request.delete(`/api/v1/items/${id}/media/${assetId}`)
      await setting(page.request, 'media.webp', was ?? null)
    }
  })
})
