import { expect, test } from '@playwright/test'

/**
 * The page as an app and as a link: installable, with a preview of a work
 * or a list in its head for whoever fetches the address without running the
 * app — and the API's answers not repeated to a client that holds them.
 */

const escapeHtml = (text: string) =>
  text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')
const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

test.describe('the page as an app, and as a link', () => {
  test('is installable: a manifest, and every icon it names', async ({ request }) => {
    const manifest = await request.get('/manifest.webmanifest')
    expect(manifest.status()).toBe(200)
    expect(manifest.headers()['content-type']).toMatch(/manifest|json/)
    const body = (await manifest.json()) as { name: string; icons: { src: string; type: string }[] }
    expect(body.name).toBe('Cinémathèque')
    expect(body.icons.length).toBeGreaterThanOrEqual(3)
    for (const icon of body.icons) {
      const response = await request.get(icon.src)
      expect(response.status(), icon.src).toBe(200)
      expect(response.headers()['content-type'], icon.src).toContain(icon.type)
    }
    const html = await (await request.get('/')).text()
    expect(html).toContain('<link rel="manifest" href="/manifest.webmanifest" />')
    expect(html).toContain('<link rel="apple-touch-icon" href="/apple-touch-icon.png" />')
    expect(html).toContain('<meta property="og:site_name" content="Cinémathèque" />')
  })

  test('a work’s address carries its own preview, and a stranger’s the site’s', async ({ request }) => {
    const { items } = (await (await request.get('/api/v1/items?kind=movie&limit=1')).json()) as {
      items: { id: string; title: string; year?: number; overview?: string }[]
    }
    const work = items[0]
    test.skip(!work, 'no film in the catalogue')
    const html = await (await request.get(`/work/${work!.id}`)).text()
    const title = escapeHtml(work!.year ? `${work!.title} (${work!.year})` : work!.title)
    expect(html).toContain(`<meta property="og:title" content="${title}" />`)
    expect(html).toContain(`<title>${title} · Cinémathèque</title>`)
    expect(html).toContain('<meta property="og:type" content="video.movie" />')
    // Once each: the site's own lines gave way.
    expect(html.match(/property="og:title"/g)?.length).toBe(1)
    expect(html.match(/name="description"/g)?.length).toBe(1)
    const opening = work!.overview?.trim().split(/\s+/).slice(0, 3).join(' ')
    if (opening) {
      expect(html).toMatch(new RegExp(`<meta property="og:description" content="${escapeRegExp(escapeHtml(opening))}`))
    }
    expect(html).toContain('<meta property="og:site_name" content="Cinémathèque" />')

    const stranger = await (await request.get('/work/no-such-work')).text()
    expect(stranger).toContain('<meta property="og:title" content="Cinémathèque" />')
    expect(stranger).toContain('<title>Cinémathèque</title>')
  })

  test('an answer is not repeated to a client that holds it', async ({ request }) => {
    // An answer no other test changes meanwhile: which sources are on.
    const first = await request.get('/api/v1/sources')
    expect(first.status()).toBe(200)
    const etag = first.headers()['etag']
    expect(etag).toMatch(/^W\/"[0-9a-f]{32}"$/)
    expect(first.headers()['cache-control']).toBe('private, no-cache')

    const again = await request.get('/api/v1/sources', { headers: { 'if-none-match': etag! } })
    expect(again.status()).toBe(304)
    expect(again.headers()['etag']).toBe(etag)
    expect(again.headers()['content-type']).toBeUndefined()
    expect(await again.text()).toBe('')

    const stale = await request.get('/api/v1/sources', { headers: { 'if-none-match': '"stale"' } })
    expect(stale.status()).toBe(200)

    // A calendar holds still between changes, so a poll of it is a header
    // exchange too.
    const calendar = await request.get('/api/v1/calendar.ics')
    const held = calendar.headers()['etag']
    expect(held).toMatch(/^W\//)
    await new Promise((resolve) => setTimeout(resolve, 1100))
    const polled = await request.get('/api/v1/calendar.ics', { headers: { 'if-none-match': held! } })
    expect(polled.status()).toBe(304)

    // A feed too, under the directive it set for itself.
    const feed = await request.get('/api/v1/feed/added.atom')
    expect(feed.headers()['etag']).toMatch(/^W\//)
    expect(feed.headers()['cache-control']).toBe('private, max-age=900')
  })
})
