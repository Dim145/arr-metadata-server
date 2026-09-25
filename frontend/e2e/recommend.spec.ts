import { expect, test } from '@playwright/test'

/**
 * What goes with a work: the catalogue's own works in the same vein, the
 * collections its films belong to, and TMDB's suggestions — those for the
 * maintainer alone.
 */
test.describe('what goes with a work', () => {
  test('works in the same vein are of the same kind, and never the work itself', async ({ request }) => {
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=1')).json()
    const work = items[0] as { id: string } | undefined
    test.skip(!work, 'no series in the catalogue')
    const response = await request.get(`/api/v1/items/${work!.id}/similar`)
    expect(response.status()).toBe(200)
    const alike = (await response.json()).items as { id: string; kind: string }[]
    expect(alike.length).toBeLessThanOrEqual(12)
    for (const other of alike) {
      expect(other.kind).toBe('series')
      expect(other.id).not.toBe(work!.id)
    }
    expect((await request.get('/api/v1/items/no-such-work/similar')).status()).toBe(404)
  })

  test('TMDB’s suggestions are the maintainer’s alone', async ({ request }) => {
    const { items } = await (await request.get('/api/v1/items?kind=movie&limit=1')).json()
    const work = items[0] as { id: string } | undefined
    test.skip(!work, 'no film in the catalogue')
    expect((await request.get(`/api/v1/items/${work!.id}/suggestions`)).status()).toBe(401)
  })

  test('the collections the films belong to, and one of them', async ({ request }) => {
    const response = await request.get('/api/v1/collections')
    expect(response.status()).toBe(200)
    const { collections } = (await response.json()) as { collections: { tmdbId: number; count: number }[] }
    for (const collection of collections) expect(collection.count).toBeGreaterThan(0)
    // One the catalogue holds nothing of is no collection to a reader.
    expect((await request.get('/api/v1/collections/1')).status()).toBe(404)
    expect((await request.get('/api/v1/collections/ten')).status()).toBe(401)
    test.skip(collections.length === 0, 'no film of the catalogue belongs to a collection')
    const page = await request.get(`/api/v1/collections/${collections[0]!.tmdbId}`)
    expect(page.status()).toBe(200)
    const data = (await page.json()) as { tmdbId: number; items: { id: string }[]; parts: { tmdbId: number; held?: string }[] }
    expect(data.tmdbId).toBe(collections[0]!.tmdbId)
    expect(data.items.length).toBe(collections[0]!.count)
    for (const part of data.parts.filter((p) => p.held)) expect(data.items.some((i) => i.id === part.held)).toBe(true)
  })
})
