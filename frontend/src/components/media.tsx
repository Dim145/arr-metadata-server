/**
 * The two things a catalogue is made of: a poster you can click, and a score.
 */

import { useQueryClient } from '@tanstack/react-query'
import { Children, useRef, useState, type CSSProperties, type ImgHTMLAttributes, type ReactNode } from 'react'
import { Link } from 'react-router'

import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { workQuery } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { hasSources, headlineRating, lookOf, poster, sized, useWebpOffered, type ImageRole, type Picture } from '../lib/media'
import { forAdults, unveil, useAdultPostersShown, useUnveiled } from '../lib/prefs'
import { hasAlpha, thumbHashToDataURL } from '../lib/thumbhash'
import { plainClick, useTransitionNavigate } from '../lib/transitions'
import type { MediaItem, MediaKind } from '../lib/types'
import { Chip, Glyph, Shelf, stockOf } from './ui'

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
      {/* The arc is the score drawn, which forced colours would erase: its
          stroke is a gradient they discard. So the gauge keeps its own
          colours there, redrawn in the system's — the track in its grey, the
          length in its highlight — and the number beside it is left to the
          palette like any other text. */}
      <svg viewBox="0 0 36 36" className="size-full -rotate-90 forced-colors:forced-color-adjust-none">
        <defs>
          <linearGradient id="score-arc" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0%" stopColor="var(--color-vermillion)" />
            <stop offset="100%" stopColor="var(--color-brass)" />
          </linearGradient>
        </defs>
        <circle
          cx="18"
          cy="18"
          r={radius}
          fill="none"
          stroke="var(--color-rule)"
          strokeWidth="2.4"
          className="forced-colors:stroke-[color:GrayText]"
        />
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
          className="forced-colors:stroke-[color:Highlight]"
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span className={cn('font-display leading-none text-bone', text)}>
          {fmt.score(value, locale)}
        </span>
        {/* At the twelve-pixel floor, and short — "35K" — so that it still
            fits inside the ring a two-million count would have overrun. */}
        {size === 'lg' && votes ? (
          <span className="mt-1 font-mono text-caption leading-none tracking-wider text-bone-faint">
            {new Intl.NumberFormat(locale, { notation: 'compact', maximumFractionDigits: 1 }).format(votes)}
          </span>
        ) : null}
      </div>
    </div>
  )
}

/* ── Artwork ──────────────────────────────────────────────────────────────── */

/**
 * The shape each kind of picture comes in, so that its box has its height
 * before the picture is there and nothing under it moves when it arrives: a
 * poster's 2:3, a still's 16:9. `auto` first, so the picture's own proportions
 * take over once it has loaded; a caller's own `aspect-` wins over these. A
 * logo and the lightbox's picture have no shape to promise.
 */
const SHAPE: Partial<Record<ImageRole, string>> = {
  card: 'aspect-[auto_2/3]',
  thumb: 'aspect-[auto_2/3]',
  poster: 'aspect-[auto_2/3]',
  headshot: 'aspect-[auto_2/3]',
  portrait: 'aspect-[auto_2/3]',
  backdrop: 'aspect-[auto_16/9]',
  still: 'aspect-[auto_16/9]',
  frame: 'aspect-[auto_16/9]',
}

/**
 * The sketch a picture kept is drawn as while it loads, as a `data:`
 * address, and whether the picture is transparent somewhere: worked out
 * once for each, whatever draws it.
 */
const SKETCHES = new Map<string, { url?: string; alpha: boolean }>()

function sketchOf(thumbhash: string): { url?: string; alpha: boolean } {
  let sketch = SKETCHES.get(thumbhash)
  if (!sketch) {
    if (SKETCHES.size > 2000) SKETCHES.clear()
    let alpha = false
    try {
      alpha = hasAlpha(Uint8Array.from(atob(thumbhash), (c) => c.charCodeAt(0)))
    } catch {
      // Not base64: no sketch, as below.
    }
    sketch = { url: alpha ? undefined : thumbHashToDataURL(thumbhash), alpha }
    SKETCHES.set(thumbhash, sketch)
  }
  return sketch
}

/**
 * What lies under a picture kept until it arrives: the colour most of it
 * is, at once, and its blurred sketch over that, filling the box as the
 * picture will. Nothing for one that is transparent somewhere — a logo
 * shows what is behind it, and a sketch would stay there — nor for the
 * lightbox's, which is drawn on its own dark.
 */
export function placeholderStyle(picture: Picture | undefined, role: ImageRole): CSSProperties | undefined {
  if (!picture?.color || role === 'logo' || role === 'full') return undefined
  const sketch = picture.thumbhash ? sketchOf(picture.thumbhash) : undefined
  if (sketch?.alpha) return undefined
  return {
    backgroundColor: picture.color,
    ...(sketch?.url ? { backgroundImage: `url("${sketch.url}")`, backgroundSize: 'cover', backgroundPosition: 'center' } : {}),
  }
}

/**
 * An image, fetched at the size it is drawn at.
 *
 * A picture this server keeps comes at the widths of its role, as WebP
 * while the server makes those copies, and whole otherwise — either way
 * under a `<picture>` offering AVIF first where the night made those — with
 * its proportions given to the box and, until it arrives, its colour and
 * its sketch under it (see `placeholderStyle`). Whether the WebP copies are
 * on is the server's to say, and every picture is drawn again when it
 * changes (see `useWebpOffered`). What the server says of a picture comes
 * with the work it is in, and is found by its address; a caller that holds
 * it may pass it as `picture`.
 *
 * Lazy unless told otherwise, because most of these are below the fold. A hero
 * backdrop is the exception and says so with `eager` — with `fetchPriority`
 * high, where it is the page's largest paint. Decoded off the main thread
 * either way, so a grid of them arriving does not hold up a click.
 *
 * A copy that fails is retried as the original it was made from; should
 * that fail too, the `fallback` takes the image's place — a drawn placeholder
 * rather than the browser's broken-image glyph beside the alternative text.
 */
export function Artwork({
  url,
  role,
  alt,
  eager = false,
  sizes,
  fallback,
  picture,
  className,
  style,
  onLoad,
  ...rest
}: {
  url: string
  role: ImageRole
  /** What the picture shows, for whoever cannot see it; empty for one that only decorates. */
  alt: string
  eager?: boolean
  /** The width it is drawn at, where the role's own does not say. */
  sizes?: string
  /** What stands in once the image and its original have both failed. */
  fallback?: ReactNode
  /** What the server says of the picture, where the caller holds it. */
  picture?: Picture
} & Omit<ImgHTMLAttributes<HTMLImageElement>, 'src' | 'srcSet' | 'sizes' | 'alt'>) {
  const look = picture ?? lookOf(url)
  const webp = useWebpOffered()
  const image = sized(url, role, look, webp)
  // Each remembered by address, so a card that moves on to another work
  // starts afresh rather than wearing the last one's failure, or its load.
  const [retried, setRetried] = useState<string>()
  const [failed, setFailed] = useState<string>()
  const [loaded, setLoaded] = useState<string>()

  if (failed === url && fallback) {
    return <>{fallback}</>
  }

  // The original alone: once a copy has failed, and where nothing else is
  // offered — an address with no copies, or a picture kept whose WebP
  // copies are off and none of whose AVIF copies reaches as wide.
  const original = retried === url || (image.src === image.original && !image.avif)
  const src = original ? image.original : image.src
  const under = loaded === url ? undefined : placeholderStyle(look, role)
  const img = (
    <img
      {...rest}
      className={cn(SHAPE[role], className)}
      style={under ? { ...under, ...style } : style}
      alt={alt}
      src={src}
      srcSet={original ? undefined : image.srcSet}
      sizes={!original && image.srcSet ? (sizes ?? image.sizes) : undefined}
      // The size is the `src`'s, which an original drawn in a copy's place
      // need not share.
      width={src === image.src ? image.width : undefined}
      height={src === image.src ? image.height : undefined}
      loading={eager ? undefined : 'lazy'}
      decoding="async"
      onLoad={(event) => {
        setLoaded(url)
        onLoad?.(event)
      }}
      onError={() => {
        if (!original) {
          setRetried(url)
        } else {
          setFailed(url)
        }
      }}
    />
  )

  if (image.avif && !original) {
    // The box is the `<img>`'s: the `<picture>` around it lays out as
    // nothing, so a caller's sizes and shapes hold as they did.
    return (
      <picture className="contents">
        <source type="image/avif" srcSet={image.avif} sizes={sizes ?? image.sizes} />
        {img}
      </picture>
    )
  }
  return img
}

/**
 * What stands where a poster should be and is not: the work's initial in the
 * display face, the glyph of its kind, its title — the spine of a case without
 * its sleeve, on a hatch that says the picture is missing rather than dark.
 *
 * Decorative unless given a `label`, which a plate's poster is: there it is
 * the picture's stand-in and is described as one.
 *
 * It is a picture, drawn, so forced colours leave it as they leave a poster:
 * its colours are a set that reads against itself, bone on ink, whatever the
 * palette around it.
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
        'forced-colors:forced-color-adjust-none',
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

/**
 * What a card needs of a work: the whole record, or as much as was kept of
 * it — and, where the record is at hand, what says it is for adults, and the
 * genres its plate is tinted by while the poster loads.
 */
export type PosterCardItem = Pick<
  MediaItem,
  'id' | 'title' | 'kind' | 'year' | 'images' | 'primaryImages' | 'ratings' | 'isManual' | 'externalIds'
> &
  Partial<Pick<MediaItem, 'isAdult' | 'contentRating' | 'genres' | 'pictures'>>

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
  const veiled = useVeiled(item)
  // What the plate shows while its poster is on the way: the colour most of
  // the poster is, where this server keeps it and said so — its sketch lies
  // on the picture itself — else the tint of the work's first genre over
  // the ink, the one colour a card earns from its record, or the ink alone
  // for a work filed under none. The poster covers it once it has loaded.
  const look = art ? (item.pictures?.[art] ?? lookOf(art)) : undefined
  const genre = item.genres?.[0]
  const waiting = look?.color
    ? { backgroundColor: look.color }
    : genre
      ? { backgroundColor: `color-mix(in srgb, var(--color-stock-${stockOf(genre)}) 14%, var(--color-ink-high))` }
      : undefined

  // The work is asked for as soon as a pointer rests on its card or focus
  // reaches it: by the time it is opened the page is there, and the poster
  // travels onto its plate rather than onto a skeleton.
  const prefetch = () => {
    if (opensWork) void client.prefetchQuery({ ...workQuery(item.id, lang), staleTime: 60_000 })
  }

  return (
    <Link
      to={to}
      // Named by what it shows — the poster's description, then the
      // rating, the title, the year or the note saying what the card is for
      // — so that whoever speaks to the page can say what they see (WCAG
      // 2.5.3).
      className="group block focus-visible:outline-offset-4"
      onPointerEnter={(event) => {
        if (event.pointerType === 'mouse') prefetch()
      }}
      onFocus={prefetch}
      onClick={(event) => {
        // A veiled poster is uncovered by the press that lands on it; the
        // title under it, and Enter, still open the work.
        if (veiled && art && plate.current?.contains(event.target as Node)) {
          event.preventDefault()
          unveil(item.id)
          return
        }
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
        style={waiting}
        className={cn(
          'relative aspect-2/3 overflow-hidden rounded-panel border border-rule bg-ink-high',
          'shadow-[var(--shadow-lift)] transition-colors duration-150',
          'group-hover:border-vermillion/45',
          // Under forced colours the card's edge is the text's colour rather
          // than the link's: the outline of a thing rather than a word to
          // follow, or a grid of them reads as one lattice of link colour.
          'forced-colors:border-[color:CanvasText]',
        )}
      >
        {art ? (
          <Artwork
            url={art}
            role="card"
            picture={look}
            alt={t.a11y.poster(item.title)}
            className={cn('size-full object-cover', veiled && VEILED)}
            fallback={<Placeholder title={item.title} kind={item.kind} className="size-full" />}
          />
        ) : (
          <Placeholder title={item.title} kind={item.kind} className="size-full" />
        )}
        {veiled && art ? <Veil id={item.id} /> : null}

        {/* A scrim only where the badges sit, so the artwork is not dimmed. */}
        {rating?.value ? (
          <div className="absolute top-0 right-0 left-0 flex justify-end bg-gradient-to-b from-ink/80 to-transparent p-2">
            <span
              style={{
                color: `var(--color-stock-${rating.value >= 8 ? 2 : rating.value >= 6.5 ? 3 : 6})`,
              }}
              className="rounded-full bg-ink/85 px-2 py-0.5 font-mono text-caption font-medium"
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
 * one row's worth, three rows deep; a shelf is a row, and it says where it
 * goes on — the season shelf's fades and arrows — where a clipped card alone
 * did not when the cut fell between two. A grid has no arrows: it fits.
 */
export function PosterShelf({ children, label }: { children: React.ReactNode; label?: string }) {
  return (
    <Shelf
      label={label}
      onKeyDown={walkCards}
      // Room above and to the left for a card's focus ring, which the
      // scroller would otherwise cut off.
      className="-mt-2 -mr-4 -ml-2 sm:m-0"
      listClassName={cn(
        'stagger gap-4 pt-2 pr-4 pb-2 pl-2',
        'sm:grid sm:grid-cols-3 sm:gap-x-4 sm:gap-y-7 sm:overflow-visible sm:p-0',
        'md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6',
      )}
    >
      {Children.map(children, (child) => (
        <li className="w-[42%] shrink-0 snap-start sm:w-auto">{child}</li>
      ))}
    </Shelf>
  )
}

/**
 * The grid posters live in. One definition, so every page spaces them alike —
 * and lays out only the rows near the screen (`.poster-grid`, styles.css),
 * and walks between its cards with the arrow keys (`walkCards`).
 *
 * A page whose grid is its results can name it (`id`), so that a link before
 * its filters — "to the results" — lands on it, and can give it a way out
 * before its first card (`skip`) — "to the filters", or past the results to
 * what follows them — for a keyboard that would otherwise tab through every
 * card to get there. Both links stay out of sight until they have the focus.
 * While the page fetches what the grid will hold, it says so (`busy`).
 */
export function PosterGrid({
  children,
  className,
  id,
  skip,
  busy,
}: {
  children: React.ReactNode
  className?: string
  /** The grid's name in the address, for a link elsewhere to lead to it. */
  id?: string
  /** A link that goes round the grid: where to, and its words. */
  skip?: { to: string; label: string }
  /** Its content is on its way (see the top of ui/feedback.tsx). */
  busy?: boolean
}) {
  return (
    <>
      {skip ? (
        <a href={`#${skip.to}`} className={SKIP_LINK}>
          {skip.label}
        </a>
      ) : null}
      {/* eslint-disable-next-line jsx-a11y/no-static-element-interactions -- the cards take the focus, one at a time; the grid hears their arrow keys */}
      <div
        id={id}
        // Focusable from a link that leads to it, and not otherwise a stop.
        tabIndex={id ? -1 : undefined}
        aria-busy={busy || undefined}
        onKeyDown={walkCards}
        className={cn(
          'poster-grid stagger grid grid-cols-2 gap-x-4 gap-y-7 outline-none',
          'sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6',
          className,
        )}
      >
        {children}
      </div>
    </>
  )
}

/** A link that shows itself only when it has the focus: the shells' "skip to content", in place. */
const SKIP_LINK =
  'sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-50 focus:rounded-card ' +
  'focus:bg-vermillion focus:px-4 focus:py-2 focus:text-sm focus:font-medium focus:text-ink'

/* ── Keyboard ─────────────────────────────────────────────────────────────── */

/** What the arrow keys move along, by axis and sense. */
const ARROWS: Record<string, { x: -1 | 0 | 1; y: -1 | 0 | 1 }> = {
  ArrowLeft: { x: -1, y: 0 },
  ArrowRight: { x: 1, y: 0 },
  ArrowUp: { x: 0, y: -1 },
  ArrowDown: { x: 0, y: 1 },
}

/** What takes the focus in a card: the card itself where it is the link, else the first control it holds. */
const FOCUSABLE = 'a[href], button:not(:disabled), [tabindex]:not([tabindex="-1"])'

/**
 * The arrow keys, between the cards of a grid or a shelf.
 *
 * Every card stays a stop of its own for Tab: a catalogue read card by card
 * is what most keyboards do, and the single stop of an ARIA grid would hide
 * forty links behind one. The arrows are a quicker way across, to the card
 * beside, above or below the one in focus — found from where the cards are
 * drawn, not from their order in the page, which the columns rearrange at
 * every width. Where there is no card that way, the key is left to the page,
 * which scrolls as it would have; with a modifier held, or from inside a
 * field, it is left alone.
 */
export function walkCards(event: React.KeyboardEvent<HTMLElement>) {
  const way = ARROWS[event.key]
  if (!way || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || event.defaultPrevented) return
  const from = event.target
  if (!(from instanceof HTMLElement) || from.matches(':read-write, select, input, [role="slider"]')) return

  const cards = [...event.currentTarget.children]
    .map((item) => (item.matches(FOCUSABLE) ? item : item.querySelector(FOCUSABLE)))
    .filter((card): card is HTMLElement => card instanceof HTMLElement && card.getClientRects().length > 0)
  if (!cards.includes(from)) return

  const to = nearest(from, cards, way)
  if (!to) return
  event.preventDefault()
  to.focus({ preventScroll: true })
  to.scrollIntoView({ block: 'nearest', inline: 'nearest' })
}

/**
 * The card a step `way` from `from`, by the boxes on screen.
 *
 * A card is that way when its near edge is past the far edge of the one in
 * focus. Of those, the ones in its row (or column) come first, the nearest
 * of them. A card out of line is taken only when none is in line — a step
 * down from the end of a full row onto a shorter last one — and then the one
 * nearest the line, its drift from it weighed double against its distance.
 */
function nearest(from: HTMLElement, cards: HTMLElement[], way: { x: number; y: number }): HTMLElement | undefined {
  const a = from.getBoundingClientRect()
  // Sub-pixel layouts put neighbours a fraction over each other's edge.
  const slack = 1
  let best: { card: HTMLElement; aligned: boolean; score: number; drift: number } | undefined

  for (const card of cards) {
    if (card === from) continue
    const b = card.getBoundingClientRect()
    const gap = way.x > 0 ? b.left - a.right : way.x < 0 ? a.left - b.right : way.y > 0 ? b.top - a.bottom : a.top - b.bottom
    if (gap < -slack) continue

    const overlap = way.x
      ? Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top)
      : Math.min(a.right, b.right) - Math.max(a.left, b.left)
    const drift = way.x
      ? Math.abs(a.top + a.height / 2 - (b.top + b.height / 2))
      : Math.abs(a.left + a.width / 2 - (b.left + b.width / 2))
    const aligned = overlap > 0
    const score = aligned ? gap : gap + 2 * drift

    const better =
      !best ||
      (aligned && !best.aligned) ||
      (aligned === best.aligned && (score < best.score || (score === best.score && drift < best.drift)))
    if (better) best = { card, aligned, score, drift }
  }

  return best?.card
}

/* ── For adults ───────────────────────────────────────────────────────────── */

/**
 * How a veiled picture is drawn: blurred past recognition, a little larger
 * than its frame so the blur's soft edge falls outside it, and dimmed under
 * the words laid over it.
 */
export const VEILED = 'scale-110 blur-xl brightness-50'

/**
 * Whether a work's pictures are veiled for this reader: a work for adults
 * (see `forAdults`), a reader who has not asked to see their posters, and a
 * poster not uncovered in this visit.
 */
export function useVeiled(work: { id: string; isAdult?: boolean | null; contentRating?: string | null }): boolean {
  const shown = useAdultPostersShown()
  const unveiled = useUnveiled(work.id)
  return !shown && !unveiled && forAdults(work)
}

/**
 * What a veiled picture says over itself: that the work is for adults, and
 * that a press shows it — the poster, or the picture where it is of another
 * kind (an episode's still, a backdrop). On a plate it is the button that does; on a card,
 * which is a link from edge to edge, it is words, and the card hears the
 * press (see `PosterCard`) — a button inside a link is no button at all. A
 * frame that leads elsewhere when pressed — an episode's, in a strip — says
 * only what it is: its work's page is where it is shown.
 */
export function Veil({
  id,
  button = false,
  shows = 'poster',
}: {
  id: string
  button?: boolean
  shows?: 'poster' | 'picture' | false
}) {
  const { t } = useI18n()
  const shape = 'absolute inset-0 grid place-items-center p-3 text-center'
  // On a ground of their own: the picture under them is dimmed in either
  // theme, and the words are the theme's.
  const words = (
    <span className="grid justify-items-center gap-1 rounded-card bg-ink/85 px-3 py-2 shadow-[var(--shadow-lift)]">
      <span className="label text-bone">{t.reader.forAdults}</span>
      {shows ? (
        <span className="text-caption text-bone-dim">{shows === 'poster' ? t.reader.show : t.reader.showPicture}</span>
      ) : null}
    </span>
  )

  return button ? (
    <button type="button" onClick={() => unveil(id)} className={cn(shape, 'cursor-pointer')}>
      {words}
    </button>
  ) : (
    <span aria-hidden className={shape}>
      {words}
    </span>
  )
}
