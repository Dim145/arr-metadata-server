// What the compiler does not check: a hook called where React cannot keep its
// order, an effect missing what it reads, a promise nobody waits for, a
// control a screen reader cannot name. Not layout: nothing here formats code.
//
// The type-aware rules read the TypeScript 6 API that typescript-eslint is
// built on — `typescript` in package.json is TypeScript 6's compatibility
// package, `@typescript/typescript6` — while `tsc` is TypeScript 7, which has
// no API yet.

import { readFileSync } from 'node:fs'
import { URL } from 'node:url'

import js from '@eslint/js'
import { defineConfig } from 'eslint/config'
import jsxA11y from 'eslint-plugin-jsx-a11y'
import reactHooks from 'eslint-plugin-react-hooks'
import tseslint from 'typescript-eslint'

// ── A check of this interface's own: a bracketed value where a token says it ──
//
// `text-[0.75rem]` where `text-caption` says it, `rounded-[6px]` where
// `rounded-md` does, a colour written out that the palette names. A page that
// asks for a token follows it when the token changes; one that wrote the value
// down does not, and the design drifts a screen at a time. The tokens are read
// from the theme itself — styles.css, then Tailwind's own steps for sizes and
// corners — so a step added there is held to here at once, and `eslint --fix`
// writes the token in. A value no token has (`text-[0.625rem]`) is let
// through: this refuses a token unused, not a value. Shadows are left alone:
// `shadow-[var(--shadow-lift)]` is how a shadow follows the light theme, which
// the plain `shadow-lift` would not (styles.css says why).

/** A stylesheet of the interface's, without its comments: they talk about tokens, in braces at times. */
const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')

/** Every custom property a stylesheet declares, each with every value it is given, in order. */
function declared(css) {
  const found = new Map()
  for (const [, name, value] of css.matchAll(/(--[\w-]+)\s*:\s*([^;{}]+);/g)) {
    found.set(name, [...(found.get(name) ?? []), value.trim()])
  }
  return found
}

/** What styles.css's `@theme` block holds: the names Tailwind makes utilities of. */
function themeBlock(css) {
  const start = css.search(/@theme\s*\{/)
  if (start < 0) return ''
  let depth = 0
  for (let index = css.indexOf('{', start); index < css.length; index += 1) {
    if (css[index] === '{') depth += 1
    if (css[index] === '}' && --depth === 0) return css.slice(start, index)
  }
  return css.slice(start)
}

/** A length as rem, where it is one this check compares. */
function remOf(value) {
  const match = /^(-?(?:\d+\.?\d*|\.\d+))(px|rem)$/.exec(value.trim())
  if (!match) return undefined
  const number = Number(match[1])
  return match[2] === 'px' ? number / 16 : number
}

/** A colour written as hex, spelt one way: six digits, lower case, no opaque alpha. */
function hexOf(value) {
  const match = /^#([\da-f]{3,4}|[\da-f]{6}|[\da-f]{8})$/i.exec(value.trim())
  if (!match) return undefined
  let digits = match[1].toLowerCase()
  if (digits.length <= 4) digits = [...digits].map((digit) => digit + digit).join('')
  return `#${digits.endsWith('ff') && digits.length === 8 ? digits.slice(0, 6) : digits}`
}

const styles = read('./src/styles.css')
const own = declared(themeBlock(styles))
const tailwind = declared(read('./node_modules/tailwindcss/theme.css'))

/** The tokens, by what they are worth: the theme's own first, so they win a value Tailwind has too. */
const TOKENS = { size: new Map(), sizes: new Set(), radius: new Map(), radii: new Set(), colour: new Map(), colours: new Set() }
for (const theme of [own, tailwind]) {
  for (const [name, [value]] of theme) {
    const [, kind, step] = /^--(text|radius|color)-([\w-]+)$/.exec(name) ?? []
    if (!step || step.includes('--')) continue
    if (kind === 'text') {
      TOKENS.sizes.add(step)
      const rem = remOf(value)
      if (rem !== undefined && !TOKENS.size.has(rem)) TOKENS.size.set(rem, step)
    } else if (kind === 'radius') {
      TOKENS.radii.add(step)
      const rem = remOf(value)
      if (rem !== undefined && !TOKENS.radius.has(rem)) TOKENS.radius.set(rem, step)
    } else {
      TOKENS.colours.add(step)
    }
  }
}
// The palette's colours by value — in either theme, since a light theme's
// colour written out is the same token missed — for the names `@theme` has.
for (const [name, values] of declared(styles)) {
  const step = /^--color-([\w-]+)$/.exec(name)?.[1]
  if (!step || !own.has(name)) continue
  for (const hex of values.map(hexOf)) {
    if (hex && !TOKENS.colour.has(hex)) TOKENS.colour.set(hex, step)
  }
}

const COLOURED = new Set([
  'text', 'bg', 'border', 'border-x', 'border-y', 'border-t', 'border-r', 'border-b', 'border-l', 'border-s',
  'border-e', 'outline', 'ring', 'ring-offset', 'inset-ring', 'fill', 'stroke', 'decoration', 'accent',
  'caret', 'from', 'via', 'to', 'divide',
])
const CORNERED = /^rounded(?:-(?:t|r|b|l|s|e|tl|tr|br|bl|ss|se|es|ee))?$/

/** A class's variants (`sm:hover:`) and the utility after them: split at the last colon outside brackets. */
function variantsOf(word) {
  let depth = 0
  let cut = -1
  for (let index = 0; index < word.length; index += 1) {
    const character = word[index]
    if (character === '[' || character === '(') depth += 1
    else if (character === ']' || character === ')') depth -= 1
    else if (character === ':' && depth === 0) cut = index
  }
  return [word.slice(0, cut + 1), word.slice(cut + 1)]
}

/** The utility a bracketed one should have been written as, if a token says its value. */
export function tokenFor(utility) {
  const match = /^(!?-?)([a-z]+(?:-[a-z]+)*?)-(?:\[([^\]]+)\]|\(([^)]+)\))(\/[^\s!]+)?(!?)$/.exec(utility)
  if (!match) return undefined
  const [, lead, name, bracketed, parenthesised, modifier = '', trail] = match
  const [, hint, value] = /^(?:(length|color|size|number):)?(.+)$/.exec(bracketed ?? `var(${parenthesised})`)
  const variable = /^var\((--[\w-]+)\)$/.exec(value)?.[1]
  const write = (token) => `${lead}${name}-${token}${modifier}${trail}`

  if (name === 'text' && hint !== 'color') {
    const step = variable ? /^--text-([\w-]+)$/.exec(variable)?.[1] : TOKENS.size.get(remOf(value))
    if (step && (TOKENS.sizes.has(step) || !variable)) return write(step)
  }
  if (CORNERED.test(name)) {
    const step = variable ? /^--radius-([\w-]+)$/.exec(variable)?.[1] : TOKENS.radius.get(remOf(value))
    if (step && (TOKENS.radii.has(step) || !variable)) return write(step)
  }
  if (COLOURED.has(name) && hint !== 'length') {
    const step = variable ? /^--color-([\w-]+)$/.exec(variable)?.[1] : TOKENS.colour.get(hexOf(value))
    if (step && (TOKENS.colours.has(step) || !variable)) return write(step)
  }
  return undefined
}

/** The plugin, named for the interface: one rule, `ams/tokens`. */
export const ams = {
  rules: {
    tokens: {
      meta: {
        type: 'suggestion',
        fixable: 'code',
        docs: { description: 'A bracketed value where a token of the theme says it.' },
        messages: {
          token: "'{{found}}' has a token: '{{token}}'. Written as the token, it follows the theme when the theme changes (styles.css, @theme).",
        },
        schema: [],
      },
      create(context) {
        const check = (node, text) => {
          const written = context.sourceCode.getText(node)
          let from = 0
          for (const word of text.split(/\s+/)) {
            if (!word.includes('[') && !word.includes('(')) continue
            const at = written.indexOf(word, from)
            if (at >= 0) from = at + word.length
            const [variants, utility] = variantsOf(word)
            const token = tokenFor(utility)
            if (!token) continue
            context.report({
              node,
              messageId: 'token',
              data: { found: word, token: variants + token },
              // Only where the class is found as written: an escape in the
              // string would shift what is replaced.
              fix: at < 0 ? null : (fixer) => fixer.replaceTextRange([node.range[0] + at, node.range[0] + at + word.length], variants + token),
            })
          }
        }
        return {
          Literal(node) {
            if (typeof node.value === 'string') check(node, node.value)
          },
          TemplateElement(node) {
            check(node, node.value.raw)
          },
        }
      },
    },
  },
}

export default defineConfig(
  // Built, recorded or generated: nobody's to correct by hand.
  { ignores: ['dist/', 'test-results/', 'playwright-report/', 'blob-report/', 'src/lib/api.gen.ts'] },

  js.configs.recommended,
  tseslint.configs.recommendedTypeChecked,
  {
    languageOptions: {
      parserOptions: {
        // Each file with its own project: the interface's tsconfig.json, or
        // the end-to-end suite's in e2e/.
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      // `tsc` refuses an unused name already (noUnusedLocals,
      // noUnusedParameters), by its own conventions: a parameter named `_…`,
      // the siblings of a rest.
      '@typescript-eslint/no-unused-vars': 'off',
    },
  },

  {
    files: ['src/**/*.tsx', 'src/**/*.ts'],
    extends: [reactHooks.configs.flat.recommended, jsxA11y.flatConfigs.recommended],
    rules: {
      // Three of the React Compiler's rules, which hold a component to what
      // the compiler needs to memoize it; this interface is not compiled.
      // What they found was state put back when the address changes, a clock
      // read for "3 days ago", and a callback handed to a render prop.
      'react-hooks/set-state-in-effect': 'off',
      'react-hooks/purity': 'off',
      'react-hooks/refs': 'off',
      // `role` on one of this interface's own components is what a picture is
      // in the layout (`<Artwork role="card">`), not an ARIA role.
      'jsx-a11y/aria-role': ['error', { ignoreNonDOM: true }],
      // Every `autoFocus` here is on a field a click has just opened — an
      // inline editor, a dialog, the search asked for — or on the sign-in and
      // registration forms, which are what those pages are for.
      'jsx-a11y/no-autofocus': 'off',
      // The recommended set, and two things this interface does on purpose:
      // a press on a modal's backdrop lands on the dialog itself, and closes
      // it; Escape, caught by the form around an inline editor, puts the
      // field back.
      'jsx-a11y/no-noninteractive-element-interactions': [
        'error',
        {
          handlers: ['onClick', 'onError', 'onLoad', 'onMouseDown', 'onMouseUp', 'onKeyPress', 'onKeyDown', 'onKeyUp'],
          alert: ['onKeyUp', 'onKeyDown', 'onKeyPress'],
          body: ['onError', 'onLoad'],
          dialog: ['onKeyUp', 'onKeyDown', 'onKeyPress', 'onMouseDown'],
          form: ['onKeyDown'],
          iframe: ['onError', 'onLoad'],
          img: ['onError', 'onLoad'],
        },
      ],
      // A region that scrolls takes the focus, so that a keyboard can scroll it.
      'jsx-a11y/no-noninteractive-tabindex': [
        'error',
        { tags: [], roles: ['tabpanel', 'region'], allowExpressionValues: true },
      ],
    },
  },

  // The interface held to its tokens (above). Its tests write bracketed values
  // on purpose: they check how classes merge.
  {
    files: ['src/**/*.ts', 'src/**/*.tsx'],
    ignores: ['src/**/*.test.ts'],
    plugins: { ams },
    rules: { 'ams/tokens': 'error' },
  },

  {
    files: ['e2e/**/*.ts'],
    rules: {
      // The suite reads the server's JSON as it came, untyped, and asserts on
      // it: that is what it is there to check.
      '@typescript-eslint/no-unsafe-argument': 'off',
      '@typescript-eslint/no-unsafe-assignment': 'off',
      '@typescript-eslint/no-unsafe-call': 'off',
      '@typescript-eslint/no-unsafe-member-access': 'off',
      '@typescript-eslint/no-unsafe-return': 'off',
    },
  },

  // The scripts that are not TypeScript are read without types: this file,
  // and the theme's, a classic script the page runs before its first paint.
  { files: ['**/*.js'], extends: [tseslint.configs.disableTypeChecked] },
  {
    files: ['public/**/*.js'],
    languageOptions: {
      sourceType: 'script',
      globals: { window: 'readonly', document: 'readonly', localStorage: 'readonly' },
    },
  },
)
