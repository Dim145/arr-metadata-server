/**
 * The rest of a long list, folded, and found all the same.
 *
 * Folded by `hidden="until-found"` where the browser has it: out of sight and
 * out of the page's height, yet searched by the browser's find in page, which
 * unfolds it on a match — a reader looking for an actor with ⌘F finds them on
 * the fourth row of a cast nobody opened. A press on the button unfolds it
 * too, and the button goes with it, the focus to the first thing it brought
 * out: a fold that closes again would only hide what was just asked for.
 *
 * Where the browser has it not — Safari, today — a `<details>`, which every
 * browser opens on a press and Chromium's find in page opens too.
 *
 * React writes `hidden` as a flag, which would hide the rest for good and
 * from the search too: the attribute is set here, by hand, on the box.
 */

import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react'

import { Button } from '../../components/ui'
import { cn } from '../../lib/cn'

/** Whether the browser unfolds `hidden="until-found"` on a match. */
const UNTIL_FOUND = typeof HTMLElement !== 'undefined' && 'onbeforematch' in HTMLElement.prototype

const SUMMARY =
  'inline-flex min-h-11 cursor-pointer list-none items-center gap-2 rounded-full border border-rule-bright px-3 ' +
  'text-body-sm font-medium text-bone transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high ' +
  '[&::-webkit-details-marker]:hidden'

export function Fold({
  label,
  children,
  className,
}: {
  /** What the button says: what it unfolds, and how many. */
  label: string
  /** What is folded: nothing laid out on the fold itself, see below. */
  children: ReactNode
  /** Where the button sits. */
  className?: string
}) {
  const id = useId()
  const [open, setOpen] = useState(false)
  const box = useRef<HTMLDivElement>(null)
  // Whether it was the button that unfolded it, which takes the focus along;
  // the browser's find in page puts the reader on the match itself.
  const pressed = useRef(false)

  // The box carries nothing of its own — no padding, no edge, no `display` —
  // as the attribute wants: what it folds is laid out by what is inside it.
  useLayoutEffect(() => {
    const node = box.current
    if (!UNTIL_FOUND || !node) return
    if (open) node.removeAttribute('hidden')
    else node.setAttribute('hidden', 'until-found')
  }, [open])

  // Found by the browser: unfolded as by the button, so the page agrees.
  useEffect(() => {
    const node = box.current
    if (!UNTIL_FOUND || !node) return
    const found = () => setOpen(true)
    node.addEventListener('beforematch', found)
    return () => node.removeEventListener('beforematch', found)
  }, [])

  useEffect(() => {
    if (!open || !pressed.current) return
    pressed.current = false
    const node = box.current
    ;(node?.querySelector<HTMLElement>('a[href], button, [tabindex]') ?? node)?.focus()
  }, [open])

  if (!UNTIL_FOUND) {
    return (
      <details>
        <summary className={cn(SUMMARY, className)}>{label}</summary>
        {children}
      </details>
    )
  }

  return (
    <>
      <div ref={box} id={id} tabIndex={-1} className="outline-none">
        {children}
      </div>
      {open ? null : (
        <div className={className}>
          <Button
            size="sm"
            aria-expanded={false}
            aria-controls={id}
            onClick={() => {
              pressed.current = true
              setOpen(true)
            }}
          >
            {label}
          </Button>
        </div>
      )}
    </>
  )
}
