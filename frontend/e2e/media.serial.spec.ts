import { deflateSync } from 'node:zlib'

import { expect, test, type Page } from '@playwright/test'

/**
 * The media kept: a picture put on a work by hand is stored and served from
 * this server, a work's pictures are read from the copies kept, and a copy
 * taken away is gone.
 *
 * Needs a store: the suite's server runs with AMS_MEDIA_STORAGE=filesystem.
 * Without one the tests say so and skip. Server-wide, because an upload and
 * a sweep change what other tests read.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

/** A red 600×900 PNG, made here: nothing to fetch. */
function png(width: number, height: number): Buffer {
  const crc = (buf: Buffer) => {
    let c = ~0
    for (const byte of buf) {
      c ^= byte
      for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    }
    return (~c >>> 0)
  }
  const chunk = (type: string, data: Buffer) => {
    const length = Buffer.alloc(4)
    length.writeUInt32BE(data.length)
    const body = Buffer.concat([Buffer.from(type, 'ascii'), data])
    const sum = Buffer.alloc(4)
    sum.writeUInt32BE(crc(body))
    return Buffer.concat([length, body, sum])
  }
  const header = Buffer.alloc(13)
  header.writeUInt32BE(width, 0)
  header.writeUInt32BE(height, 4)
  header[8] = 8
  header[9] = 2
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(width * 3, 0).map((_, i) => (i % 3 === 0 ? 180 : 40))])
  const raw = Buffer.concat(Array.from({ length: height }, () => row))
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

test.describe('the media kept', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('a picture put on a work by hand is kept, served, and taken away again', async ({ page }) => {
    test.slow()
    await signIn(page)

    const status = (await (await page.request.get('/api/v1/media/status')).json()) as { backend: string }
    test.skip(status.backend === 'off', 'no media store is configured on this server')

    const works = (await (await page.request.get('/api/v1/items?kind=series&limit=1')).json()) as {
      items: { id: string }[]
    }
    const id = works.items[0]?.id
    test.skip(!id, 'the catalogue is empty')

    // Uploaded, and pointed at from the work.
    const uploaded = await page.request.post(`/api/v1/items/${id}/media`, {
      multipart: {
        file: { name: 'poster.png', mimeType: 'image/png', buffer: png(600, 900) },
        coverType: 'banner',
      },
    })
    expect(uploaded.status()).toBe(201)
    const { assetId, url, origin } = (await uploaded.json()) as { assetId: string; url: string; origin: string }
    expect(origin).toMatch(/^upload:/)
    expect(url).toMatch(/\/media\/[0-9a-f]{64}\.png$/)

    // Served from here, immutable, with a thumbnail, and by a range.
    const served = await page.request.get(url)
    expect(served.status()).toBe(200)
    expect(served.headers()['content-type']).toBe('image/png')
    expect(served.headers()['cache-control']).toContain('immutable')
    expect((await page.request.get(url.replace(/\.png$/, '-t.jpg'))).status()).toBe(200)
    const part = await page.request.get(url, { headers: { Range: 'bytes=0-7' } })
    expect(part.status()).toBe(206)
    expect((await part.body()).length).toBe(8)

    // On the work, as its own, kept here.
    const { store, media } = (await (await page.request.get(`/api/v1/items/${id}/media`)).json()) as {
      store: boolean
      media: { origin: string; status: string; assetId?: string }[]
    }
    expect(store).toBe(true)
    expect(media.find((m) => m.origin === origin)).toMatchObject({ status: 'stored', assetId })

    await page.goto(`/admin/catalogue/${id}#artwork`)
    const panel = page.locator('#artwork')
    await expect(panel.getByText(/kept here|stockée ici/i).first()).toBeVisible()
    await expect(panel.getByRole('button', { name: /upload a picture|envoyer une image/i })).toBeVisible()

    // Taken away: the file with it.
    expect((await page.request.delete(`/api/v1/items/${id}/media/${assetId}`)).status()).toBe(204)
    expect((await page.request.get(url)).status()).toBe(404)
    const after = (await (await page.request.get(`/api/v1/items/${id}`)).json()) as {
      images: { url: string; isManual: boolean }[]
    }
    expect(after.images.some((image) => image.url === url)).toBe(false)
  })

  test('the media page reads the store, and a stranger reads nothing of it', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/media')
    await expect(page.getByRole('heading', { level: 1, name: /^(media|médias)$/i })).toBeVisible()

    const refused = await page.request.post('/api/v1/items/nothing/media', {
      multipart: { file: { name: 'x.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') } },
    })
    expect([400, 404, 503]).toContain(refused.status())

    const stranger = await page.context().browser()!.newContext()
    expect((await stranger.request.get('/api/v1/media/status')).status()).toBe(401)
    expect((await stranger.request.post('/api/v1/media/retry')).status()).toBe(401)
    // A key nobody has is nothing, whoever asks.
    expect((await stranger.request.get(`/media/${'0'.repeat(64)}.jpg`)).status()).toBe(404)
    expect((await stranger.request.get('/media/not-a-key.jpg')).status()).toBe(404)
    await stranger.close()
  })
})
