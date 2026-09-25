/**
 * Small marks: chips, genre tints and where a value came from.
 */

import { type ReactNode } from 'react'
import { Link } from 'react-router'
import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import { genreLabel } from '../../lib/labels'

import { Glyph } from './glyphs'

type ChipTone = 'neutral' | 'manual' | 'provider' | 'accent'

const CHIP_TONE: Record<ChipTone, string> = {
  neutral: 'border-rule-bright text-bone-dim',
  manual: 'border-brass-deep text-brass',
  provider: 'border-slate-deep text-slate',
  accent: 'border-vermillion-deep text-vermillion',
}

export function Chip({
  children,
  tone = 'neutral',
  className,
}: {
  children: ReactNode
  tone?: ChipTone
  className?: string
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1',
        'font-mono text-[0.6875rem] font-medium tracking-[0.12em] uppercase',
        CHIP_TONE[tone],
        className,
      )}
    >
      {children}
    </span>
  )
}

/**
 * A genre, in its own colour.
 *
 * The tint is a stable function of the name, so Drama is the same green on
 * every page and in every language of the interface — and a crime thriller does
 * not look like a comedy. It carries no meaning beyond identity, which is why
 * the name is always written out rather than the colour standing for it.
 */
export function Genre({
  name,
  to,
  className,
}: {
  name: string
  /** Where the genre leads — the catalogue narrowed to it — when it is a link. */
  to?: string
  className?: string
}) {
  const { lang } = useI18n()
  // The tint follows the stored name, so a genre keeps its colour whichever
  // language it is read in.
  const stock = stockOf(name)

  const style = {
    color: `var(--color-stock-${stock})`,
    borderColor: `color-mix(in srgb, var(--color-stock-${stock}) 35%, transparent)`,
    backgroundColor: `color-mix(in srgb, var(--color-stock-${stock}) 10%, transparent)`,
  }
  const classes = cn(
    'inline-flex items-center rounded-full border px-2.5 py-1 text-xs font-medium',
    'transition-colors duration-150',
    to && 'hover:border-current focus-visible:outline-offset-2',
    className,
  )

  return to ? (
    <Link to={to} style={style} className={classes}>
      {genreLabel(name, lang)}
    </Link>
  ) : (
    <span style={style} className={classes}>
      {genreLabel(name, lang)}
    </span>
  )
}

/** One of eight film-stock tints, the same one every time for a given name. */
function stockOf(name: string): number {
  let hash = 0
  for (let index = 0; index < name.length; index += 1) {
    hash = (hash * 31 + name.charCodeAt(index)) | 0
  }

  return (Math.abs(hash) % 8) + 1
}

/**
 * Who decided this value.
 *
 * Carries a glyph and a word as well as its colour: someone who cannot tell
 * brass from slate still has to be able to tell a person's edit from a
 * provider's answer, since that distinction is the point of this server.
 */
export function Provenance({ manual, label }: { manual: boolean; label: string }) {
  return (
    <Chip tone={manual ? 'manual' : 'provider'}>
      <Glyph name={manual ? 'lock' : 'cloud'} className="size-3" />
      {label}
    </Chip>
  )
}
