/**
 * How a person's role and status read, wherever they appear.
 *
 * Vermillion for an administrator, who holds the keys; brass for an editor,
 * whose hand is on the catalogue — the colour a hand-made change wears
 * everywhere else; slate for a member, like everything that is only read.
 */

import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import type { AccountRole, AccountStatus } from '../../lib/types'
import { Lamp } from '../ui/choice'

const ROLE_TONE: Record<AccountRole, string> = {
  admin: 'border-vermillion-deep bg-vermillion/10 text-vermillion',
  editor: 'border-brass-deep bg-brass/10 text-brass',
  member: 'border-slate-deep text-slate',
}

export function RoleChip({ role, className }: { role: AccountRole; className?: string }) {
  const { t } = useI18n()

  return (
    <span
      className={cn(
        'inline-flex items-center rounded-full border px-2.5 py-1 font-mono text-[0.6875rem] font-medium tracking-[0.12em] uppercase',
        ROLE_TONE[role],
        className,
      )}
    >
      {t.labels.roles[role]}
    </span>
  )
}

export function StatusLamp({ status }: { status: AccountStatus }) {
  const { t } = useI18n()
  const tone = status === 'active' ? 'moss' : status === 'pending' ? 'brass' : 'faint'

  return <Lamp tone={tone}>{t.labels.statuses[status]}</Lamp>
}

/** A person's initial, in the display face, where a portrait would be. */
export function Initial({ name, className }: { name: string; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        'grid size-9 shrink-0 place-items-center rounded-full border border-rule-bright bg-ink-top font-display text-base text-bone',
        className,
      )}
    >
      {name.trim().slice(0, 1).toUpperCase() || '·'}
    </span>
  )
}
