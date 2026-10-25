import { describe, expect, it } from 'vitest'

import { decadePath, facetApiPath, genrePath, networkPath, slugOf, yearPath } from './facets'

describe('the pages of the catalogue', () => {
  it('address a genre and a network by the slug the server makes of their names', () => {
    expect(genrePath('Drama')).toBe('/genre/drama')
    expect(genrePath('Science Fiction')).toBe('/genre/science-fiction')
    // Its French name leads to the same page, which the server answers under its slug.
    expect(genrePath('Drame')).toBe('/genre/drame')
    expect(genrePath('Comédie')).toBe('/genre/comedie')
    expect(networkPath('Canal+')).toBe('/network/canal')
    expect(networkPath(' BBC One ')).toBe('/network/bbc-one')
  })

  it('hand a name the server folds otherwise over as it is', () => {
    expect(slugOf('フジテレビ')).toBe(encodeURIComponent('フジテレビ'))
    expect(slugOf('Cœur')).toBe(encodeURIComponent('Cœur'))
  })

  it('address a year by itself and a decade by its first year', () => {
    expect(yearPath(1999)).toBe('/year/1999')
    expect(decadePath(1997)).toBe('/decade/1990')
    expect(decadePath(2000)).toBe('/decade/2000')
  })

  it('read a page from the API by its plural', () => {
    expect(facetApiPath('genre', 'drama')).toBe('/genres/drama')
    expect(facetApiPath('decade', '1990')).toBe('/decades/1990')
    expect(facetApiPath('network', 'a/b')).toBe('/networks/a%2Fb')
  })
})
