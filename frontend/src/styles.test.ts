/**
 * The theme's tokens, and the check that holds the interface to them
 * (eslint.config.js, `ams/tokens`): a bracketed value is refused where a token
 * of styles.css or of Tailwind says it, and `--fix` writes the token in —
 * keeping the class's variants, its opacity and its weight.
 */

import { Linter, type ESLint } from 'eslint'
import { describe, expect, it } from 'vitest'

// The configuration is a script of its own, outside the interface's types:
// asked for by a name the compiler does not follow.
const configuration = '../eslint.config.js'
const { ams } = (await import(/* @vite-ignore */ configuration)) as { ams: ESLint.Plugin }

const linter = new Linter({ configType: 'flat' })
const config: Linter.Config[] = [
  {
    files: ['**/*.jsx'],
    plugins: { ams },
    rules: { 'ams/tokens': 'error' },
    languageOptions: { parserOptions: { ecmaFeatures: { jsx: true } } },
  },
]

/** What the check says of a line of a page. */
function said(code: string): string[] {
  return linter.verify(code, config, { filename: 'page.jsx' }).map((message) => message.message)
}

/** The line once `--fix` has written the tokens in. */
function fixed(code: string): string {
  return linter.verifyAndFix(code, config, { filename: 'page.jsx' }).output
}

describe('the tokens check', () => {
  it('refuses a size the type scale names, and says which step', () => {
    expect(said(`const a = 'font-mono text-[0.6875rem] uppercase'`)).toEqual([
      "'text-[0.6875rem]' has a token: 'text-label'. Written as the token, it follows the theme when the theme changes (styles.css, @theme).",
    ])
  })

  it('writes the token in, keeping the variants, the opacity and the weight', () => {
    expect(fixed(`const a = 'sm:hover:text-[0.8125rem] text-bone'`)).toBe(`const a = 'sm:hover:text-body-sm text-bone'`)
    expect(fixed(`const a = 'bg-[#0b0b0c]/50 text-[13px]!'`)).toBe(`const a = 'bg-ink/50 text-body-sm!'`)
    expect(fixed(`const a = 'rounded-[6px] rounded-t-[2px] rounded-[10px]'`)).toBe(`const a = 'rounded-md rounded-t-xs rounded-card'`)
  })

  it('prefers the theme’s own step where Tailwind has one of the same size', () => {
    // Twelve pixels is both `caption` (the size alone) and Tailwind's `xs`
    // (with a line height of its own): the bracketed size set the size alone.
    expect(fixed(`const a = 'text-[0.75rem]'`)).toBe(`const a = 'text-caption'`)
  })

  it('names a colour written out, in either theme, and one asked for by its variable', () => {
    expect(fixed(`const a = 'border-[#f6f3ee] text-[var(--color-bone)] bg-(--color-status-error)'`)).toBe(
      `const a = 'border-ink text-bone bg-status-error'`,
    )
  })

  it('reads a page’s classes wherever they are written', () => {
    expect(fixed('const a = <p className="px-5 text-[0.6875rem]">{b}</p>')).toBe('const a = <p className="px-5 text-label">{b}</p>')
    expect(fixed('const a = `mt-1 ${b} text-[0.8125rem] ${c}`')).toBe('const a = `mt-1 ${b} text-body-sm ${c}`')
  })

  it('lets through what no token says, and what is bracketed on purpose', () => {
    expect(
      said(
        `const a = 'text-[0.625rem] rounded-[3px] tracking-[0.12em] [text-wrap-style:balance] shadow-[var(--shadow-lift)] bg-(--surface-overlay) aria-[current=page]:bg-ink-top items[0]'`,
      ),
    ).toEqual([])
  })
})
