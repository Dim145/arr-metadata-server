/**
 * When a field of the sign-in and sign-up forms says it is wrong.
 *
 * Once the reader has typed in it and left it, or has sent the form — never
 * while they are still typing, nor on a field they only passed through: an
 * error on a field nobody has touched yet is a page scolding its visitor.
 * It is the moment `:user-invalid` keeps, which the fields are drawn by; kept
 * here too, so that `aria-invalid` and the words under the field come at the
 * same moment as the red edge, and a screen reader hears what the eye sees.
 *
 * What is wrong is the field's own word for it as well: each message is set
 * as the field's custom validity, so the browser's idea of a valid form is
 * the page's, and the first field it holds wrong is the one given the focus.
 */

import { useEffect, useState, type RefObject } from 'react'

/** The edge a field wears once it says it is wrong — by the browser's account or the page's. */
export const WRONG = 'user-invalid:border-status-error aria-invalid:border-status-error'

export function useDone<K extends string>(keys: readonly K[]) {
  const [typed, setTyped] = useState<ReadonlySet<K>>(() => new Set())
  const [done, setDone] = useState<ReadonlySet<K>>(() => new Set())

  return {
    /** Typed in. */
    typed: (key: K) => setTyped((was) => (was.has(key) ? was : new Set(was).add(key))),
    /** Left: done with, if it was typed in. */
    left: (key: K) => {
      if (typed.has(key)) setDone((was) => (was.has(key) ? was : new Set(was).add(key)))
    },
    /** The form sent: every field is done with. */
    sent: () => setDone(new Set(keys)),
    /** Whether the field may say it is wrong. */
    done: (key: K) => done.has(key),
  }
}

/**
 * The fields' custom validity, by their names in the form, kept to what the
 * page says of each — after every drawing, which costs a few assignments, so
 * that a form drawn late, once what the page waited for has come, is kept too.
 */
export function useValidity(form: RefObject<HTMLFormElement | null>, wrong: Record<string, string | undefined>) {
  useEffect(() => {
    for (const [name, message] of Object.entries(wrong)) {
      const field = form.current?.elements.namedItem(name)
      if (field instanceof HTMLInputElement) field.setCustomValidity(message ?? '')
    }
  })
}

/** The first field of a form the browser holds wrong, which the focus is given on sending it. */
export function firstWrong(form: HTMLFormElement): HTMLElement | undefined {
  return [...form.elements].find(
    (element): element is HTMLInputElement => element instanceof HTMLInputElement && !element.validity.valid,
  )
}
