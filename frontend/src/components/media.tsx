/**
 * The two things a catalogue is made of: a poster you can click, and a score.
 */

import { useQueryClient } from '@tanstack/react-query'
import { Children, useRef, useState, type ImgHTMLAttributes, type ReactNode } from 'react'
import { Link } from 'react-router'

import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { workQuery } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { fallBackToOriginal, hasSources, headlineRating, poster, sized, type ImageRole } from '../lib/media'
import { plainClick, useTransitionNavigate } from '../lib/transitions'
import type { MediaItem, MediaKind } from '../lib/types'
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
 *
 * A thumbnail that fails is retried as the original it was made from; should
 * that fail too, the `fallback` takes the image's place — a drawn placeholder
 * rather than the browser's broken-image glyph beside the alternative text.
 */
export function Artwork({
  url,
  role,
  eager = false,
  sizes,
  fallback,
  ...rest
}: {
  url: string
  role: ImageRole
  eager?: boolean
  /** The width it is drawn at, where the role's own does not say. */
  sizes?: string
  /** What stands in once the image and its original have both failed. */
  fallback?: ReactNode
} & Omit<ImgHTMLAttributes<HTMLImageElement>, 'src' | 'srcSet' | 'sizes'>) {
  const image = sized(url, role)
  const retry = image.src === image.original ? undefined : fallBackToOriginal(image.original)
  // Remembered by address, so a card that moves on to another work starts
  // afresh rather than wearing the last one's failure.
  const [failed, setFailed] = useState<string>()

  if (failed === url && fallback) {
    return <>{fallback}</>
  }

  return (
    <img
      {...rest}
      src={image.src}
      srcSet={image.srcSet}
      sizes={image.srcSet ? (sizes ?? image.sizes) : undefined}
      loading={eager ? undefined : 'lazy'}
      decoding="async"
      onError={(event) => {
        if (retry && !event.currentTarget.dataset.fellBack) {
          retry(event)
        } else {
          setFailed(url)
        }
      }}
    />
  )
}

/**
 * What stands where a poster should be and is not: the work's initial in the
 * display face, the glyph of its kind, its title — the spine of a case without
 * its sleeve, on a hatch that says the picture is missing rather than dark.
 *
 * Decorative unless given a `label`, which a plate's poster is: there it is
 * the picture's stand-in and is described as one.
 */
export function Placeholder({
  title,
  kind,
  label,
  className,
}: {
  title: string
  kind: MediaKind
  label?: string
  className?: string
}) {
  const initial = [...title.trim()][0]?.toLocaleUpperCase() ?? '·'

  return (
    <div
      data-placeholder
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={cn(
        'hatch @container grid grid-rows-[auto_minmax(0,1fr)_auto] overflow-hidden bg-ink-high p-[7%] text-bone-faint',
        className,
      )}
    >
      {/* The glyph and the title only where there is room to read them: a
          thumbnail keeps the initial alone. */}
      <Glyph name={kind === 'series' ? 'tv' : 'film'} className="hidden size-4 @min-[6rem]:block" />
      <span aria-hidden className="place-self-center font-display text-[42cqw] leading-none text-bone-dim/45">
        {initial}
      </span>
      <span className="hidden line-clamp-2 text-xs leading-tight text-bone-dim @min-[6rem]:block">{title}</span>
    </div>
  )
}

/* ── Poster ───────────────────────────────────────────────────────────────── */

/** What a card needs of a work: the whole record, or as much as was kept of it. */
export type PosterCardItem = Pick<
  MediaItem,
  'id' | 'title' | 'kind' | 'year' | 'images' | 'primaryImages' | 'ratings' | 'isManual' | 'externalIds'
>

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
  item: PosterCardItem
  to: string
  /** What to say under the title in place of the year. */
  note?: string
  /** Say whether it is a series or a film: for a list that mixes them. */
  kind?: boolean
}) {
  const { t, lang, locale } = useI18n()
  const client = useQueryClient()
  const go = useTransitionNavigate()
  const plate = useRef<HTMLDivElement>(null)
  const caption =
    note ??
    [item.year, kind ? (item.kind === 'series' ? t.home.kindSeries : t.home.kindFilm) : undefined]
      .filter(Boolean)
      .join(' · ')
  const art = poster(item)
  const rating = headlineRating(item.ratings)
  const opensWork = to === `/work/${item.id}`

  // The work is asked for as soon as a pointer rests on its card or focus
  // reaches it: by the time it is opened the page is there, and the poster
  // travels onto its plate rather than onto a skeleton.
  const prefetch = () => {
    if (opensWork) void client.prefetchQuery({ ...workQuery(item.id, lang), staleTime: 60_000 })
  }

  return (
    <Link
      to={to}
      // The note says what the card is for — the season it leads to — so it
      // is read too.
      aria-label={note ? `${t.a11y.openWork(item.title)} · ${note}` : t.a11y.openWork(item.title)}
      className="group block focus-visible:outline-offset-4"
      onPointerEnter={(event) => {
        if (event.pointerType === 'mouse') prefetch()
      }}
      onFocus={prefetch}
      onClick={(event) => {
        if (!plainClick(event)) return
        event.preventDefault()
        go(to, { from: opensWork ? plate.current : null })
      }}
    >
      {/* Colour only. Every mature catalogue tried scaling posters on hover and
          dropped it: twenty cards re-rasterising at once drops frames on a
          phone, and the lift adds nothing a border change does not say. */}
      <div
        ref={plate}
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
            fallback={<Placeholder title={item.title} kind={item.kind} className="size-full" />}
          />
        ) : (
          <Placeholder title={item.title} kind={item.kind} className="size-full" />
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

        {/* A lock means a hand held something against the sources; a work no
            source feeds has nothing to hold against. At the right, clear of
            the title a stand-in writes along its foot. */}
        {item.isManual && hasSources(item) ? (
          <div className="absolute right-2 bottom-2">
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
        <p className="font-mono text-xs tracking-wide text-bone-faint tabular-nums">{caption || '—'}</p>
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
