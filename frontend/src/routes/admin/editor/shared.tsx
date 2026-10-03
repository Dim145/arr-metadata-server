/**
 * What every tab of the work editor is built from: the small marks, the
 * folding "add" form, the one dialog that asks before a row is removed.
 *
 * The tabs are the sections a record divides into, and the address carries
 * which is open: `?tab=seasons`, so that a season's own page, the back button
 * and a bookmark all land on the right one.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useRef, useState, type ReactNode } from 'react'

import { Button, Dialog, Glyph, Spinner, type GlyphName } from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import { useI18n } from '../../../lib/i18n'
import type { MediaItem } from '../../../lib/types'

/** The sections of the editor, in the order the tabs show them. */
export type Tab = 'record' | 'seasons' | 'artwork' | 'people' | 'elsewhere'

export const TABS: Tab[] = ['record', 'seasons', 'artwork', 'people', 'elsewhere']

export function isTab(value: string | null): value is Tab {
  return TABS.includes(value as Tab)
}

/**
 * Which tab an anchor belongs to: the ids the old one-page editor used, and
 * the public pages still link to, each land on the tab that holds them.
 */
export function tabOfAnchor(hash: string): Tab | undefined {
  switch (hash.replace(/^#/, '')) {
    case 'fields':
    case 'sources':
    case 'record':
      return 'record'
    case 'seasons':
    case 'episodes':
    case 'season-fields':
      return 'seasons'
    case 'artwork':
      return 'artwork'
    case 'credits':
    case 'titles':
    case 'translations':
      return 'people'
    case 'identifiers':
    case 'suggestions':
    case 'ratings':
    case 'related':
    case 'orders':
      return 'elsewhere'
    default:
      return undefined
  }
}

/** A figure beside a heading, in the register's own hand. */
export function Count({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn('font-mono text-xs text-bone-faint tabular-nums', className)}>{children}</span>
}

/** How many locks something carries, in brass, with the padlock beside it. */
export function LockBadge({ n, className }: { n: number; className?: string }) {
  const { t } = useI18n()
  if (!n) return null
  return (
    <span className={cn('inline-flex items-center gap-1 font-mono text-[0.6875rem] font-medium text-brass tabular-nums', className)}>
      <Glyph name="lock" className="size-3" />
      {n}
      <span className="sr-only"> {t.admin.editor.lockCount(n)}</span>
    </span>
  )
}

export function Empty({ children, className }: { children: ReactNode; className?: string }) {
  return <p className={cn('px-5 py-4 text-sm text-bone-faint italic', className)}>{children}</p>
}

/**
 * A line of facts in small capitals — the network, the runtime, a count —
 * the way an index card prints what is not prose.
 */
export function Facts({ items, className }: { items: (ReactNode | undefined | null | false)[]; className?: string }) {
  const shown = items.filter(Boolean)
  if (!shown.length) return null
  return (
    <p className={cn('font-mono text-[0.6875rem] leading-relaxed tracking-[0.1em] text-bone-faint uppercase tabular-nums', className)}>
      {shown.map((item, index) => (
        <span key={index}>
          {/* One mark between facts, tied to the fact before it with a
              no-break space: a line then ends on the mark rather than
              beginning with one. */}
          {index > 0 ? (
            <span aria-hidden className="text-bone-faint/60">
              {' ·'}{' '}
            </span>
          ) : null}
          <span className="inline-block">{item}</span>
        </span>
      ))}
    </p>
  )
}

/** Where an image lives, which is as much of its URL as a person reads. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

/**
 * A form that stays folded until it is asked for, so a panel stays legible.
 * Focus comes back to the button that opened it when it folds again.
 */
export function AddForm({
  label,
  glyph = 'plus',
  onSubmit,
  pending,
  error,
  children,
  hint,
  submitLabel,
  className,
}: {
  label: string
  glyph?: GlyphName
  onSubmit: () => void
  pending: boolean
  error?: Error | null
  children: ReactNode
  /** A word beside the button while the form is folded. */
  hint?: ReactNode
  submitLabel?: string
  className?: string
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const trigger = useRef<HTMLButtonElement>(null)

  const close = () => {
    setOpen(false)
    requestAnimationFrame(() => trigger.current?.focus())
  }

  if (!open) {
    return (
      <div className={cn('flex flex-wrap items-center gap-3 border-t border-rule px-5 py-3', className)}>
        <Button ref={trigger} size="sm" onClick={() => setOpen(true)}>
          <Glyph name={glyph} className="size-4" />
          {label}
        </Button>
        {hint}
      </div>
    )
  }

  return (
    <form
      className={cn('flex flex-col gap-4 border-t border-rule bg-ink/40 px-5 py-4', className)}
      onSubmit={(event) => {
        event.preventDefault()
        onSubmit()
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') close()
      }}
    >
      <p className="text-sm font-medium text-bone">{label}</p>
      {children}

      {error ? (
        <p role="alert" className="flex items-center gap-1.5 text-xs text-vermillion">
          <Glyph name="alert" className="size-3.5 shrink-0" />
          {error.message || t.common.actionFailed}
        </p>
      ) : null}

      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="primary" size="sm" disabled={pending}>
          {pending ? <Spinner className="size-4" /> : <Glyph name={glyph} className="size-4" />}
          {submitLabel ?? t.common.add}
        </Button>
        <Button type="button" size="sm" onClick={close}>
          {t.common.cancel}
        </Button>
      </div>
    </form>
  )
}

/** A row an operator has asked to remove: the route to call, and its name. */
export type Target = { path: string; label: string }

/**
 * Asking before a hand-added row goes. One dialog for a whole tab: the rows
 * only name what they are and which route forgets them.
 */
export function useRemove(item: MediaItem, onChanged: () => void) {
  const { t } = useI18n()
  const queryClient = useQueryClient()
  const [removing, setRemoving] = useState<Target | null>(null)

  const remove = useMutation({
    mutationFn: (path: string) => api.delete(`/items/${item.id}/${path}`),
    onSuccess: () => {
      setRemoving(null)
      void queryClient.invalidateQueries({ queryKey: ['item', item.id] })
      onChanged()
    },
  })

  const dialog = (
    <Dialog
      open={removing !== null}
      title={t.admin.editor.children.removeTitle}
      onClose={() => setRemoving(null)}
      footer={
        <>
          <Button onClick={() => setRemoving(null)}>{t.common.cancel}</Button>
          <Button variant="danger" disabled={remove.isPending} onClick={() => removing && remove.mutate(removing.path)}>
            <Glyph name="trash" className="size-4" />
            {t.common.delete}
          </Button>
        </>
      }
    >
      <p className="mb-2 font-mono text-[0.8125rem] text-bone">{removing?.label}</p>
      {t.admin.editor.children.removeBody}
      {remove.isError ? (
        <p role="alert" className="mt-3 text-sm text-vermillion">
          {remove.error.message || t.common.actionFailed}
        </p>
      ) : null}
    </Dialog>
  )

  return { ask: setRemoving, dialog }
}

/** A hand-added row's mark and its way out, at the end of the row. */
export function Yours({ onRemove }: { onRemove?: () => void }) {
  const { t } = useI18n()
  return (
    <span className="flex shrink-0 items-center gap-1">
      <span className="inline-flex items-center gap-1.5 rounded-full border border-brass-deep px-2.5 py-1 font-mono text-[0.6875rem] font-medium tracking-[0.12em] text-brass uppercase">
        <Glyph name="lock" className="size-3" />
        {t.admin.editor.children.yours}
      </span>
      {onRemove ? (
        <button
          type="button"
          onClick={onRemove}
          aria-label={t.common.delete}
          title={t.common.delete}
          className="grid size-11 shrink-0 cursor-pointer place-items-center rounded-card text-bone-faint transition-colors duration-150 hover:bg-ink-high hover:text-vermillion"
        >
          <Glyph name="trash" className="size-4" />
        </button>
      ) : null}
    </span>
  )
}
