// What the compiler does not check: a hook called where React cannot keep its
// order, an effect missing what it reads, a promise nobody waits for, a
// control a screen reader cannot name. Not layout: nothing here formats code.
//
// The type-aware rules read the TypeScript 6 API that typescript-eslint is
// built on — `typescript` in package.json is TypeScript 6's compatibility
// package, `@typescript/typescript6` — while `tsc` is TypeScript 7, which has
// no API yet.

import js from '@eslint/js'
import { defineConfig } from 'eslint/config'
import jsxA11y from 'eslint-plugin-jsx-a11y'
import reactHooks from 'eslint-plugin-react-hooks'
import tseslint from 'typescript-eslint'

export default defineConfig(
  // Built, recorded or generated: nobody's to correct by hand.
  { ignores: ['dist/', 'test-results/', 'playwright-report/', 'blob-report/', 'src/lib/api.gen.d.ts'] },

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
