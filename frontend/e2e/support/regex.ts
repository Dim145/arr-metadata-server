/**
 * Patterns built from what a test read off the server.
 *
 * A title, a slug, an address: text the suite did not write, matched as
 * written — so every metacharacter is escaped — and compiled in one place,
 * which is the one place to look when a pattern is wrong.
 */

/** The text as a pattern that matches it and nothing else. */
export function escaped(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * A regular expression from a source the test composed: anchors and groups
 * of its own around text it escaped. Nothing a visitor types reaches it.
 */
export function pattern(source: string, flags?: string): RegExp {
  // nosemgrep: javascript.lang.security.audit.detect-non-literal-regexp.detect-non-literal-regexp
  return new RegExp(source, flags)
}
