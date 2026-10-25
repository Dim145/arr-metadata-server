/**
 * Leaving a page that holds something unsaved: what asks first.
 *
 * The work editor keeps the changes typed into it until they are saved, and
 * says so before anything takes the person elsewhere. A link pressed is
 * caught where it is pressed (`routes/admin/editor/drafts.tsx`); what moves
 * on by itself — the palette, a key, the next work — goes through here, so
 * that it asks too.
 */

/**
 * Takes over a departure when it has something to ask: `true` if it will
 * call `proceed` itself once answered, or never; `false` to let it go.
 */
type Asker = (proceed: () => void) => boolean

let asker: Asker | null = null

/** Ask before leaving, while `next` is set; `null` stops asking. */
export function guardLeaving(next: Asker | null) {
  asker = next
}

/** Go on — at once, or once whatever holds unsaved changes has been answered. */
export function leaving(proceed: () => void) {
  if (asker?.(proceed)) return
  proceed()
}
