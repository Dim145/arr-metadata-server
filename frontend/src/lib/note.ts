/**
 * The programmer's note, read.
 *
 * A note is a restricted Markdown: paragraphs, `*emphasis*`, `**strong
 * emphasis**` and `[links](/genre/drama)` to pages of this site, and nothing
 * else. It is taken apart here into pieces a page draws as elements — never as
 * HTML, so nothing a note says can become markup. The rules are the server's
 * (`src/db/repo/editorial.rs`), which checks a note as it is saved; a link
 * that leads anywhere but a page of this site is drawn as its words, in case
 * one got past it.
 */

export type Inline =
  | { kind: 'text'; text: string }
  | { kind: 'em'; children: Inline[] }
  | { kind: 'strong'; children: Inline[] }
  | { kind: 'link'; children: Inline[]; to: string }

/** The longest a note may be, in characters: the server's limit. */
export const MAX_NOTE = 3000

/** The longest a link's address may be. */
const MAX_LINK = 300

/** What a backslash makes a plain character of. */
const ESCAPABLE = new Set(['\\', '*', '[', ']', '(', ')'])

const escaped = (chars: string[], at: number) => chars[at] === '\\' && ESCAPABLE.has(chars[at + 1] ?? '')

const startsWith = (chars: string[], at: number, pattern: string[]) =>
  pattern.every((c, index) => chars[at + index] === c)

/** Where the next unescaped `pattern` starts, from `from` on, or -1. */
function find(chars: string[], from: number, pattern: string[]): number {
  let at = from
  while (at < chars.length) {
    if (escaped(chars, at)) {
      at += 2
      continue
    }
    if (startsWith(chars, at, pattern)) return at
    at += 1
  }
  return -1
}

/**
 * Where the `*` that closes an emphasis opened just before `from` is: the next
 * lone one, after something other than a space. A `**` on the way is strong
 * emphasis inside it, passed over whole.
 */
function closingStar(chars: string[], from: number): number {
  let at = from
  while (at < chars.length) {
    if (escaped(chars, at)) {
      at += 2
    } else if (chars[at] === '*' && chars[at + 1] === '*') {
      at += 2
    } else if (chars[at] === '*' && at > from && !/\s/u.test(chars[at - 1] ?? '')) {
      return at
    } else {
      at += 1
    }
  }
  return -1
}

/**
 * Whether an address leads to a page of this site: a path from its root —
 * `/genre/drama`, `/browse?genre=drama` — with nothing a page's address
 * cannot hold.
 */
export function isInside(to: string): boolean {
  return (
    to.startsWith('/') &&
    !to.startsWith('//') &&
    Array.from(to).length <= MAX_LINK &&
    /^[\x21-\x7e]+$/.test(to) &&
    !/[\\<>"`]/.test(to)
  )
}

/** A link that starts at `at`, a `[`: its text, its address, and where it ends. */
function linkAt(chars: string[], at: number): { text: string[]; to: string; end: number } | undefined {
  const close = find(chars, at + 1, [']'])
  if (close < 0) return undefined
  // Its text is on one line, with no link inside it.
  const text = chars.slice(at + 1, close)
  if (!text.length || text.includes('\n') || find(text, 0, ['[']) >= 0) return undefined
  if (chars[close + 1] !== '(') return undefined
  const end = find(chars, close + 2, [')'])
  if (end < 0) return undefined
  const to = chars.slice(close + 2, end).join('')
  if (!to || /\s/u.test(to)) return undefined
  return { text, to, end }
}

function pieces(chars: string[]): Inline[] {
  const out: Inline[] = []
  let plain = ''
  const flush = () => {
    if (plain) {
      out.push({ kind: 'text', text: plain })
      plain = ''
    }
  }

  let at = 0
  while (at < chars.length) {
    const c = chars[at]
    if (escaped(chars, at)) {
      plain += chars[at + 1]
      at += 2
      continue
    }
    if (startsWith(chars, at, ['*', '*'])) {
      const close = find(chars, at + 2, ['*', '*'])
      if (close > at + 2) {
        flush()
        out.push({ kind: 'strong', children: pieces(chars.slice(at + 2, close)) })
        at = close + 2
      } else {
        plain += '**'
        at += 2
      }
      continue
    }
    if (c === '*' && at + 1 < chars.length && !/\s/u.test(chars[at + 1] ?? '')) {
      const close = closingStar(chars, at + 1)
      if (close >= 0) {
        flush()
        out.push({ kind: 'em', children: pieces(chars.slice(at + 1, close)) })
        at = close + 1
      } else {
        plain += '*'
        at += 1
      }
      continue
    }
    if (c === '[') {
      const link = linkAt(chars, at)
      if (link) {
        flush()
        out.push({ kind: 'link', children: pieces(link.text), to: link.to })
        at = link.end + 1
        continue
      }
    }
    plain += c
    at += 1
  }
  flush()
  return out
}

/**
 * A paragraph's pieces: text, emphasis, strong emphasis and links; an escaped
 * character as itself, and a marker nothing closes as itself too.
 */
export function inline(text: string): Inline[] {
  return pieces(Array.from(text))
}

/** A note's paragraphs: its text cut at each empty line, the lines of one joined by a space. */
export function paragraphs(body: string): string[] {
  const out: string[] = []
  let current: string[] = []
  for (const line of body.split(/\r\n|\r|\n/)) {
    if (!line.trim()) {
      if (current.length) {
        out.push(current.join(' '))
        current = []
      }
    } else {
      current.push(line.trim())
    }
  }
  if (current.length) out.push(current.join(' '))
  return out
}

/** A note as one line of plain text: its markers taken out, a link as its words. */
export function plain(body: string): string {
  const flatten = (list: Inline[]): string =>
    list.map((piece) => (piece.kind === 'text' ? piece.text : flatten(piece.children))).join('')
  return paragraphs(body)
    .map((paragraph) => flatten(inline(paragraph)))
    .join(' ')
    .split(/\s+/u)
    .filter(Boolean)
    .join(' ')
}

/**
 * What the server would refuse in a note, said before it is sent — HTML, a
 * link elsewhere, too many characters — or nothing. The server checks again.
 */
export function problem(body: string): 'html' | 'link' | 'long' | undefined {
  if (Array.from(body).length > MAX_NOTE) return 'long'
  if (/<[A-Za-z/!?]/.test(body)) return 'html'
  const links = (list: Inline[]): string[] =>
    list.flatMap((piece) =>
      piece.kind === 'link' ? [piece.to, ...links(piece.children)] : piece.kind === 'text' ? [] : links(piece.children),
    )
  const outside = paragraphs(body)
    .flatMap((paragraph) => links(inline(paragraph)))
    .some((to) => !isInside(to))
  return outside ? 'link' : undefined
}
