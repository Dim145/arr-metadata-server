/**
 * Small marks: chips, genre tints, where a value came from and what an API
 * asks for.
 */

import { type ReactNode } from 'react'
import { Link } from 'react-router'
import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import { genreLabel, policyLabel } from '../../lib/labels'

import { Glyph, type GlyphName } from './glyphs'

type ChipTone = 'neutral' | 'manual' | 'provider' | 'accent'

const CHIP_TONE: Record<ChipTone, string> = {
  neutral: 'border-rule-bright text-bone-dim',
  manual: 'border-brass-deep text-brass',
  provider: 'border-slate-deep text-slate',
  accent: 'border-vermillion-deep text-vermillion',
}

/**
 * A state or a source, in a word, in a pill: `neutral`; `manual`, brass, a
 * person's hand; `provider`, slate, a machine's answer; `accent`, what
 * should not pass unseen.
 *
 * States: it is read, not pressed, so it has one — at rest, in its tone's
 * words on its tone's deep edge. Under forced colours the edge is the
 * button colour. Whatever it says of a failure or a wait, it says in words.
 */
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
        // At the twelve-pixel floor: a chip says a state or a source, which
        // is read, not glanced at. Its edge, under forced colours, is the
        // button colour, as a pill's edge is wherever the system draws one.
        'font-mono text-caption font-medium tracking-[0.12em] uppercase',
        'forced-colors:border-[color:ButtonText]',
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
 *
 * States: at rest, its stock's words on a tenth of its stock, edged with a
 * third. As a link: hover, the edge in the words' colour; focus, the ring;
 * active, as hover. It has no disabled, waiting, failed or empty state.
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
export function stockOf(name: string): number {
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
 *
 * States: a `Chip`'s, in two tones — a lock in brass, a cloud in slate.
 */
export function Provenance({ manual, label }: { manual: boolean; label: string }) {
  return (
    <Chip tone={manual ? 'manual' : 'provider'}>
      <Glyph name={manual ? 'lock' : 'cloud'} className="size-3" />
      {label}
    </Chip>
  )
}

/**
 * What an API asks of a caller, in a word: one chip for the dashboard, the
 * configuration and the access page, which had two between them and drew the
 * same policy in two colours, with two glyphs and two sets of words.
 *
 * `open` is the one worth the accent: it means anyone who can reach the port is
 * served, which on a machine with a port forward is the whole internet.
 *
 * States: a `Chip`'s — `open` in the accent with the alert glyph; the
 * allowlist with the globe, `disabled` with the power glyph, a key with the
 * key, all neutral.
 */
export function PolicyChip({ policy }: { policy: string }) {
  const { t } = useI18n()
  const glyph: GlyphName =
    policy === 'open' ? 'alert' : policy === 'allowlist' ? 'globe' : policy === 'disabled' ? 'power' : 'key'

  return (
    <Chip tone={policy === 'open' ? 'accent' : 'neutral'} className="whitespace-nowrap">
      <Glyph name={glyph} className="size-3" />
      {policyLabel(policy, t)}
    </Chip>
  )
}
