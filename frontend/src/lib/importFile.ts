/**
 * The lines of a batch to import, read from what somebody pasted or dropped.
 *
 * The server reads each line: `tmdb:603`, `tt0133093`, `tvdb:81189`, a
 * work's page at TMDB or IMDb… What is read here is only how a file holds
 * them. Text is a line a work. A CSV names its columns — a TMDB, IMDb or
 * TheTVDB id, the kind, the title, as Radarr's, Sonarr's, IMDb's and Trakt's
 * exports write them — or, with no header this reader knows, has an id in
 * one of its cells. JSON is a list of ids, or of works with their ids, flat
 * (`tmdbId`, `imdb_id`) or under `ids` and under `movie` or `show`, as
 * Trakt nests them.
 */

import type { ImportLine, ImportPreview, MediaKind } from './types'

/** The most lines a batch takes, as the server has it. */
export const MOST_LINES = 2000

/** A column's name, as compared: lower case, without spaces, dashes or underscores. */
function plain(name: string): string {
  return name.toLowerCase().replace(/[\s_\-.]/g, '')
}

const COLUMNS: Record<string, string[]> = {
  tmdb: ['tmdb', 'tmdbid', 'themoviedb', 'themoviedbid'],
  imdb: ['imdb', 'imdbid', 'const', 'tconst'],
  tvdb: ['tvdb', 'tvdbid', 'thetvdb', 'thetvdbid'],
  anilist: ['anilist', 'anilistid'],
  mal: ['mal', 'malid', 'myanimelist', 'myanimelistid'],
  fankai: ['fankai', 'fankaiid'],
  kind: ['kind', 'type', 'titletype', 'mediatype'],
  title: ['title', 'name', 'originaltitle'],
  url: ['url', 'link', 'uri', 'address'],
}

/** What a word for a kind of work stands for, as the exports write it. */
export function kindOf(word: unknown): MediaKind | undefined {
  if (typeof word !== 'string') return undefined
  const text = plain(word)
  if (['movie', 'movies', 'film', 'films', 'feature', 'tvmovie', 'video', 'short'].includes(text)) return 'movie'
  if (['series', 'tv', 'tvseries', 'tvminiseries', 'show', 'shows', 'anime', 'tvshow'].includes(text)) return 'series'
  return undefined
}

/** Whether a cell looks like an id the server reads, as it is. */
function looksLikeId(cell: string): boolean {
  const text = cell.trim()
  return (
    /^tt\d+$/i.test(text) ||
    /^(tmdb|tmdbid|imdb|imdbid|tvdb|tvdbid|anilist|mal|myanimelist|fankai)\s*:\s*\S+$/i.test(text) ||
    /^https?:\/\//i.test(text)
  )
}

/** A line, from a work's ids and its kind: the id the server imports it by first. */
function lineOf(
  ids: { tmdb?: unknown; imdb?: unknown; tvdb?: unknown; anilist?: unknown; mal?: unknown; fankai?: unknown; url?: unknown },
  kind: MediaKind | undefined,
  title: unknown,
): ImportLine | undefined {
  const value = (raw: unknown) => {
    if (typeof raw === 'number' && Number.isFinite(raw) && raw > 0) return String(raw)
    if (typeof raw === 'string' && raw.trim() && raw.trim() !== '0') return raw.trim()
    return undefined
  }
  const [tmdb, imdb, tvdb, anilist, mal, fankai, url] = [
    value(ids.tmdb),
    value(ids.imdb),
    value(ids.tvdb),
    value(ids.anilist),
    value(ids.mal),
    value(ids.fankai),
    value(ids.url),
  ]
  const named = typeof title === 'string' && title.trim() ? { title: title.trim() } : {}
  const withKind = kind ? { kind } : {}
  // A series by TheTVDB, as Sonarr holds it; a film by TMDB, as Radarr does;
  // either by IMDb; the rest by what is left.
  let text: string | undefined
  if (tvdb && kind !== 'movie') text = `tvdb:${tvdb}`
  else if (tmdb && kind) text = `tmdb:${tmdb}`
  else if (imdb) text = imdb.includes(':') || /^tt\d+$/i.test(imdb) ? imdb : `imdb:${imdb}`
  else if (tmdb) text = `tmdb:${tmdb}`
  else if (tvdb) text = `tvdb:${tvdb}`
  else if (anilist) text = `anilist:${anilist}`
  else if (mal) text = `mal:${mal}`
  else if (fankai) text = `fankai:${fankai}`
  else if (url) text = url
  return text ? { text, ...withKind, ...named } : undefined
}

/** Pasted text: a work a line, comments and blank lines left out. */
export function linesOfText(text: string): ImportLine[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith('#'))
    .map((line) => ({ text: line }))
}

/** One row of a CSV, its quoted fields read as such. */
function cells(row: string, separator: string): string[] {
  const out: string[] = []
  let cell = ''
  let quoted = false
  for (let at = 0; at < row.length; at += 1) {
    const char = row[at]
    if (quoted) {
      if (char === '"' && row[at + 1] === '"') {
        cell += '"'
        at += 1
      } else if (char === '"') {
        quoted = false
      } else {
        cell += char
      }
    } else if (char === '"') {
      quoted = true
    } else if (char === separator) {
      out.push(cell)
      cell = ''
    } else {
      cell += char
    }
  }
  out.push(cell)
  return out.map((value) => value.trim())
}

/** The rows of a CSV: a newline inside quotes stays in its cell. */
function rows(text: string): string[] {
  const out: string[] = []
  let row = ''
  let quoted = false
  for (const char of text.replace(/\r\n?/g, '\n')) {
    if (char === '"') quoted = !quoted
    if (char === '\n' && !quoted) {
      out.push(row)
      row = ''
    } else {
      row += char
    }
  }
  out.push(row)
  return out.filter((line) => line.trim())
}

/** A CSV: by its header where it names the columns, else by the id in each row. */
export function linesOfCsv(text: string): ImportLine[] {
  const all = rows(text)
  const first = all[0] ?? ''
  // The separator the header uses most: a comma, a semicolon or a tab.
  const separator = [',', ';', '\t'].reduce((best, candidate) =>
    first.split(candidate).length > first.split(best).length ? candidate : best,
  )
  const header = cells(first, separator).map(plain)
  const column = (name: string) => header.findIndex((cell) => COLUMNS[name]?.includes(cell))
  const at = Object.fromEntries(Object.keys(COLUMNS).map((name) => [name, column(name)])) as Record<string, number>
  const named = ['tmdb', 'imdb', 'tvdb', 'anilist', 'mal', 'fankai', 'url'].some((name) => (at[name] ?? -1) >= 0)

  if (!named) {
    // No header this reader knows: the first cell of each row that looks
    // like an id.
    return all
      .map((row) => cells(row, separator).find(looksLikeId))
      .filter((cell): cell is string => Boolean(cell))
      .map((cell) => ({ text: cell }))
  }

  const out: ImportLine[] = []
  for (const row of all.slice(1)) {
    const values = cells(row, separator)
    const get = (name: string) => {
      const index = at[name] ?? -1
      return index >= 0 ? values[index] : undefined
    }
    const line = lineOf(
      {
        tmdb: get('tmdb'),
        imdb: get('imdb'),
        tvdb: get('tvdb'),
        anilist: get('anilist'),
        mal: get('mal'),
        fankai: get('fankai'),
        url: get('url'),
      },
      kindOf(get('kind')),
      get('title'),
    )
    if (line) out.push(line)
  }
  return out
}

type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

/** A field of an object by any of its spellings. */
function field(object: Record<string, Json>, names: string[]): Json | undefined {
  const keys = Object.keys(object)
  for (const name of names) {
    const key = keys.find((candidate) => plain(candidate) === name)
    if (key !== undefined) return object[key]
  }
  return undefined
}

function isObject(value: Json | undefined): value is Record<string, Json> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** One element of a JSON list: an id, or a work with its ids. */
function lineOfJson(value: Json): ImportLine | undefined {
  if (typeof value === 'string') return value.trim() ? { text: value.trim() } : undefined
  if (!isObject(value)) return undefined
  // Trakt's: the work under `movie` or `show`, its ids under `ids`.
  const nested = field(value, ['movie', 'show'])
  const work = isObject(nested) ? nested : value
  const kindFromNest = isObject(field(value, ['movie'])) ? 'movie' : isObject(field(value, ['show'])) ? 'series' : undefined
  const ids = isObject(field(work, ['ids'])) ? (field(work, ['ids']) as Record<string, Json>) : work
  return lineOf(
    {
      tmdb: field(ids, COLUMNS.tmdb ?? []),
      imdb: field(ids, COLUMNS.imdb ?? []),
      tvdb: field(ids, COLUMNS.tvdb ?? []),
      anilist: field(ids, COLUMNS.anilist ?? []),
      mal: field(ids, COLUMNS.mal ?? []),
      fankai: field(ids, COLUMNS.fankai ?? []),
      url: field(work, COLUMNS.url ?? []),
    },
    kindFromNest ?? kindOf(field(value, COLUMNS.kind ?? []) ?? field(work, COLUMNS.kind ?? [])),
    field(work, COLUMNS.title ?? []),
  )
}

/** A JSON document: a list, or an object holding one under any key. */
export function linesOfJson(document: unknown): ImportLine[] {
  const value = document as Json
  const list = Array.isArray(value)
    ? value
    : isObject(value)
      ? (Object.values(value).find((inside): inside is Json[] => Array.isArray(inside)) ?? [value])
      : []
  return list.map(lineOfJson).filter((line): line is ImportLine => Boolean(line))
}

/**
 * The lines a batch is given once they were set beside the catalogue: all of
 * them but those a provider said it has no work for. Asked again, each would
 * only be not found again, at the provider's cost; the preview said so of
 * them already.
 */
export function worthImporting(lines: ImportLine[], preview: Pick<ImportPreview, 'lines'> | null): ImportLine[] {
  if (!preview) return lines
  const missing = new Set(preview.lines.filter((line) => line.status === 'missing').map((line) => line.line))
  return lines.filter((_, at) => !missing.has(at))
}

/** Whatever was dropped: JSON, a CSV, or text a line a work, by its name and then its first character. */
export function linesOfFile(name: string, text: string): ImportLine[] {
  const body = text.replace(/^\uFEFF/, '')
  const lower = name.toLowerCase()
  const trimmed = body.trimStart()
  if (lower.endsWith('.json') || trimmed.startsWith('[') || trimmed.startsWith('{')) {
    try {
      return linesOfJson(JSON.parse(body))
    } catch {
      // Not JSON after all: read as text.
    }
  }
  if (lower.endsWith('.csv') || lower.endsWith('.tsv')) return linesOfCsv(body)
  return linesOfText(body)
}
