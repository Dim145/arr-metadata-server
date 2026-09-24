/**
 * The two things a catalogue is made of: a poster you can click, and a score.
 */

import { Children, type ImgHTMLAttributes } from 'react'
import { Link } from 'react-router'

import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import { fallBackToOriginal, headlineRating, poster, sized, type ImageRole } from '../lib/media'
import type { MediaItem } from '../lib/types'
import { Chip, Glyph } from './ui'

/* ── Score ────────────────────────────────────────────────────────────────── */

/**
 * A score as a gauge.
 *
 * The arc is an SVG stroke with a dash offset — one path, no filter, no blur —
 * so a grid of these costs the compositor nothing. The number is set in the
 * display face because on a film page a rating is a headline, not a data point.
 */
export function Score({
  value,
  votes,
  source,
  size = 'md',
  className,
}: {
  value: number
  votes?: number
  /** Who gave it, by name, for the reader who cannot see the caption beside it. */
  source?: string
  size?: 'sm' | 'md' | 'lg'
  className?: string
}) {
  const { locale, t } = useI18n()

  const fraction = Math.max(0, Math.min(value / 10, 1))
  const radius = 15.5
  const circumference = 2 * Math.PI * radius

  const box = { sm: 'size-10', md: 'size-14', lg: 'size-20' }[size]
  const text = { sm: 'text-xs', md: 'text-base', lg: 'text-2xl' }[size]

  return (
    <div
      className={cn('relative shrink-0', box, className)}
      role="img"
      aria-label={
        source
          ? t.a11y.ratingOn(fmt.score(value, locale) ?? '', source)
          : t.a11y.ratingOf(fmt.score(value, locale) ?? '')
      }
    >
      <svg viewBox="0 0 36 36" className="size-full -rotate-90">
        <defs>
          <linearGradient id="score-arc" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0%" stopColor="var(--color-vermillion)" />
            <stop offset="100%" stopColor="var(--color-brass)" />
          </linearGradient>
        </defs>
        <circle cx="18" cy="18" r={radius} fill="none" stroke="var(--color-rule)" strokeWidth="2.4" />
        <circle
          cx="18"
          cy="18"
          r={radius}
          fill="none"
          stroke="url(#score-arc)"
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - fraction)}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span className={cn('font-display leading-none text-bone', text)}>
          {fmt.score(value, locale)}
        </span>
        {size === 'lg' && votes ? (
          <span className="mt-1 font-mono text-[0.5rem] tracking-wider text-bone-faint">
            {fmt.count(votes, locale)}
          </span>
        ) : null}
      </div>
    </div>
  )
}

/* ── Artwork ──────────────────────────────────────────────────────────────── */

/**
 * An image from a provider, fetched at the size it is drawn at.
 *
 * Lazy unless told otherwise, because most of these are below the fold. A hero
 * backdrop is the exception and says so with `eager`.
 */
export function Artwork({
  url,
  role,
  eager = false,
  sizes,
  ...rest
}: {
  url: string
  role: ImageRole
  eager?: boolean
  /** The width it is drawn at, where the role's own does not say. */
  sizes?: string
} & Omit<ImgHTMLAttributes<HTMLImageElement>, 'src' | 'srcSet' | 'sizes'>) {
  const image = sized(url, role)

  return (
    <img
      {...rest}
      src={image.src}
      srcSet={image.srcSet}
      sizes={image.srcSet ? (sizes ?? image.sizes) : undefined}
      loading={eager ? undefined : 'lazy'}
      decoding="async"
      onError={image.src === image.original ? undefined : fallBackToOriginal(image.original)}
    />
  )
}

/* ── Poster ───────────────────────────────────────────────────────────────── */

/**
 * One work in a grid.
 *
 * The whole card is the link, so the target is the poster rather than the words
 * under it. The year sits under the title rather than over the artwork: text
 * laid on a poster is a pattern every mature catalogue has tried and dropped.
 */
export function PosterCard({
  item,
  to,
  note,
  kind = false,
}: {
  item: MediaItem
  to: string
  /** What to say under the title in place of the year. */
  note?: string
  /** Say whether it is a series or a film: for a list that mixes them. */
  kind?: boolean
}) {
  const { t, locale } = useI18n()
  const caption =
    note ??
    [item.year, kind ? (item.kind === 'series' ? t.home.kindSeries : t.home.kindFilm) : undefined]
      .filter(Boolean)
      .join(' · ')
  const art = poster(item)
  const rating = headlineRating(item.ratings)

  return (
    <Link
      to={to}
      // The note says what the card is for — the season it leads to — so it
      // is read too.
      aria-label={note ? `${t.a11y.openWork(item.title)} · ${note}` : t.a11y.openWork(item.title)}
      className="group block focus-visible:outline-offset-4"
    >
      {/* Colour only. Every mature catalogue tried scaling posters on hover and
          dropped it: twenty cards re-rasterising at once drops frames on a
          phone, and the lift adds nothing a border change does not say. */}
      <div
        className={cn(
          'relative aspect-2/3 overflow-hidden rounded-panel border border-rule bg-ink-high',
          'shadow-[var(--shadow-lift)] transition-colors duration-150',
          'group-hover:border-vermillion/45',
        )}
      >
        {art ? (
          <Artwork
            url={art}
            role="card"
            alt={t.a11y.poster(item.title)}
            className="size-full object-cover"
          />
        ) : (
          <div className="flex size-full items-center justify-center">
            <Glyph name={item.kind === 'series' ? 'tv' : 'film'} className="size-7 text-bone-faint" />
          </div>
        )}

        {/* A scrim only where the badges sit, so the artwork is not dimmed. */}
        {rating?.value ? (
          <div className="absolute top-0 right-0 left-0 flex justify-end bg-gradient-to-b from-ink/80 to-transparent p-2">
            <span
              style={{
                color: `var(--color-stock-${rating.value >= 8 ? 2 : rating.value >= 6.5 ? 3 : 6})`,
              }}
              className="rounded-full bg-ink/85 px-2 py-0.5 font-mono text-[0.6875rem] font-medium"
            >
              {fmt.score(rating.value, locale)}
            </span>
          </div>
        ) : null}

        {item.isManual ? (
          <div className="absolute bottom-2 left-2">
            <Chip tone="manual">
              <Glyph name="lock" className="size-3" />
            </Chip>
          </div>
        ) : null}
      </div>

      <div className="mt-2.5 space-y-0.5">
        <h3 className="line-clamp-2 text-sm leading-snug font-medium text-bone transition-colors duration-200 group-hover:text-vermillion">
          {item.title}
        </h3>
        <p className="font-mono text-[0.6875rem] tracking-wide text-bone-faint tabular-nums">
          {caption || '—'}
        </p>
      </div>
    </Link>
  )
}

/**
 * A row of posters: a shelf that scrolls sideways on a phone, the grid
 * anywhere wider. On the front page two columns of twelve were six screens of
 * one row's worth, three rows deep; a shelf is a row, and the card clipped at
 * the edge says there is more.
 */
export function PosterShelf({ children, label }: { children: React.ReactNode; label?: string }) {
  return (
    <ul
      aria-label={label || undefined}
      className={cn(
        // Room above and to the left for a card's focus ring, which the
        // scroller would otherwise cut off.
        'stagger -mt-2 -mr-4 -ml-2 flex snap-x snap-mandatory scroll-pl-2 gap-4 overflow-x-auto pt-2 pr-4 pb-2 pl-2',
        'sm:m-0 sm:grid sm:grid-cols-3 sm:gap-x-4 sm:gap-y-7 sm:overflow-visible sm:p-0',
        'md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6',
      )}
    >
      {Children.map(children, (child) => (
        <li className="w-[42%] shrink-0 snap-start sm:w-auto">{child}</li>
      ))}
    </ul>
  )
}

/** The grid posters live in. One definition, so every page spaces them alike. */
export function PosterGrid({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        'stagger grid grid-cols-2 gap-x-4 gap-y-7',
        'sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6',
        className,
      )}
    >
      {children}
    </div>
  )
}
