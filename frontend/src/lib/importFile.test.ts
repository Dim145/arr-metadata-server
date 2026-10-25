import { describe, expect, it } from 'vitest'

import { kindOf, linesOfCsv, linesOfFile, linesOfJson, linesOfText, worthImporting } from './importFile'

describe('the lines of a batch', () => {
  it('reads pasted text a work a line, without its comments', () => {
    expect(linesOfText('# my films\ntmdb:603\n\n  tt0133093  \nhttps://www.themoviedb.org/tv/1396\n')).toEqual([
      { text: 'tmdb:603' },
      { text: 'tt0133093' },
      { text: 'https://www.themoviedb.org/tv/1396' },
    ])
  })

  it('reads a CSV by the columns its header names', () => {
    // As IMDb exports a list: its own id, and the kind of title.
    const imdb = 'Position,Const,Created,Title,Title Type\n1,tt0133093,2020-01-01,"The Matrix, Reloaded?",Movie\n2,tt0903747,2020-01-02,Breaking Bad,TV Series\n'
    expect(linesOfCsv(imdb)).toEqual([
      { text: 'tt0133093', kind: 'movie', title: 'The Matrix, Reloaded?' },
      { text: 'tt0903747', kind: 'series', title: 'Breaking Bad' },
    ])
    // A film by TMDB when the kind is known; a series by TheTVDB.
    const mixed = 'title;tmdb_id;tvdb_id;type\nHeat;949;;movie\nThe Wire;1438;79126;show\nUnknown;603;;\n'
    expect(linesOfCsv(mixed)).toEqual([
      { text: 'tmdb:949', kind: 'movie', title: 'Heat' },
      { text: 'tvdb:79126', kind: 'series', title: 'The Wire' },
      // A TMDB id of no kind: the server asks which.
      { text: 'tmdb:603', title: 'Unknown' },
    ])
  })

  it('reads a CSV with no header it knows by the id in each row', () => {
    expect(linesOfCsv('a,tt0133093,x\nb,c,https://www.imdb.com/title/tt0903747/\nnothing,here,at all\n')).toEqual([
      { text: 'tt0133093' },
      { text: 'https://www.imdb.com/title/tt0903747/' },
    ])
  })

  it('reads JSON as Radarr, Sonarr and Trakt export it', () => {
    // Radarr's films, Sonarr's series.
    expect(
      linesOfJson([
        { title: 'Heat', tmdbId: 949, imdbId: 'tt0113277' },
        { title: 'The Wire', tvdbId: 79126 },
      ]),
    ).toEqual([
      { text: 'tt0113277', title: 'Heat' },
      { text: 'tvdb:79126', title: 'The Wire' },
    ])
    // Trakt's: the work nested, its ids under `ids`.
    expect(
      linesOfJson([
        { type: 'movie', movie: { title: 'Heat', ids: { tmdb: 949, imdb: 'tt0113277' } } },
        { type: 'show', show: { title: 'The Wire', ids: { tvdb: 79126, tmdb: 1438 } } },
      ]),
    ).toEqual([
      { text: 'tmdb:949', kind: 'movie', title: 'Heat' },
      { text: 'tvdb:79126', kind: 'series', title: 'The Wire' },
    ])
    // A list of ids, or a list under a key.
    expect(linesOfJson(['tmdb:603', ' ', 'tt0133093'])).toEqual([{ text: 'tmdb:603' }, { text: 'tt0133093' }])
    expect(linesOfJson({ items: ['tt0133093'] })).toEqual([{ text: 'tt0133093' }])
  })

  it('reads a file by its name, then by what it starts with', () => {
    expect(linesOfFile('list.json', '["tt0133093"]')).toEqual([{ text: 'tt0133093' }])
    expect(linesOfFile('list.txt', '\uFEFF[ "tt0133093" ]')).toEqual([{ text: 'tt0133093' }])
    expect(linesOfFile('export.csv', 'tmdb,type\n949,movie\n')).toEqual([{ text: 'tmdb:949', kind: 'movie' }])
    expect(linesOfFile('ids.txt', 'tmdb:949  # Heat (1995)\n')).toEqual([{ text: 'tmdb:949  # Heat (1995)' }])
    // Not JSON after all.
    expect(linesOfFile('odd.json', '[not json\ntt1')).toEqual([{ text: '[not json' }, { text: 'tt1' }])
  })

  it('leaves out of the batch the lines no provider has a work for, and nothing else', () => {
    const lines = [{ text: 'tmdb:603' }, { text: '# a comment' }, { text: 'tmdb:1' }, { text: 'tt0133093' }, { text: 'tmdb:603' }]
    // The comment was passed over, so the preview's lines say where each was.
    const preview = {
      lines: [
        { line: 0, text: 'tmdb:603', status: 'candidate' as const, checked: true },
        { line: 2, text: 'tmdb:1', status: 'missing' as const, checked: true },
        { line: 3, text: 'tt0133093', status: 'held' as const, checked: false },
        { line: 4, text: 'tmdb:603', status: 'repeated' as const, checked: false },
      ],
    }
    expect(worthImporting(lines, preview)).toEqual([{ text: 'tmdb:603' }, { text: '# a comment' }, { text: 'tt0133093' }, { text: 'tmdb:603' }])
    // Before any preview, every line.
    expect(worthImporting(lines, null)).toEqual(lines)
  })

  it('knows the words for a kind of work', () => {
    expect(kindOf('Movie')).toBe('movie')
    expect(kindOf('TV Mini Series')).toBe('series')
    expect(kindOf('tvSeries')).toBe('series')
    expect(kindOf('podcast')).toBeUndefined()
    expect(kindOf(3)).toBeUndefined()
  })
})
