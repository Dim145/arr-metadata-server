import { expect, test } from '@playwright/test'

/**
 * Where a work can be watched: the services TMDB lists from JustWatch for a
 * country, read as the work's page is. Needs a TMDB key on the server; the
 * test steps aside where there is none.
 */
test.describe('where to watch', () => {
  test('a work’s services are listed for a country, with their source named', async ({ request }) => {
    const { items } = await (await request.get('/api/v1/items?kind=movie&limit=1')).json()
    const work = items[0] as { id: string } | undefined
    test.skip(!work, 'no film in the catalogue')
    const response = await request.get(`/api/v1/items/${work!.id}/watch`)
    test.skip(response.status() === 503, 'no TMDB key configured')
    expect(response.status()).toBe(200)
    const data = await response.json()
    expect(data.region).toMatch(/^[A-Z]{2}$/)
    expect(data.attribution).toBe('JustWatch')
    for (const group of ['flatrate', 'rent', 'buy', 'free', 'ads']) expect(Array.isArray(data[group]), group).toBe(true)
    for (const provider of data.flatrate as { id: number; name: string; logo?: string }[]) {
      expect(typeof provider.id).toBe('number')
      expect(provider.name).toBeTruthy()
    }
    // Another country of those listed, and a country that is not one.
    if (data.regions.length > 1) {
      const other = data.regions.find((r: string) => r !== data.region)
      const elsewhere = await (await request.get(`/api/v1/items/${work!.id}/watch?region=${other}`)).json()
      expect(elsewhere.region).toBe(other)
    }
    expect((await request.get(`/api/v1/items/${work!.id}/watch?region=France`)).status()).toBe(400)
    expect((await request.get('/api/v1/items/no-such-work/watch')).status()).toBe(404)
  })
})
