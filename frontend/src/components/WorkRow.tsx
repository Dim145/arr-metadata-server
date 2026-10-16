/**
 * One work as a row of a list: the catalogue read rather than recognised.
 *
 * The grid is for knowing a film by its poster; the list is for comparing —
 * what it is, when, how it is rated, how it stands, what it is about — down a
 * column, so the poster shrinks to a thumbnail and the words get the room.
 *
 * One link per work, and it is the title: a stretched pseudo-element makes the
 * whole row its target without wrapping a heading, chips and a paragraph in an
 * anchor, which a screen reader would then read out as one very long name.
 */

import { useQueryClient } from '@tanstack/react-query'
import { useRef } from 'react'
import { Link } from 'react-router'

import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { workQuery } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { headlineRating, poster } from '../lib/media'
import { statusLabel } from '../lib/labels'
import { plainClick, useTransitionNavigate } from '../lib/transitions'
import type { MediaItem } from '../lib/types'
import { Artwork, Placeholder, type PosterCardItem } from './media'
import { Genre } from './ui'

/** What a row needs of a work: what a card does, and the words besides. */
export type WorkRowItem = PosterCardItem & Pick<MediaItem, 'genres' | 'status' | 'overview'>

/** The genres a row names; the rest are a click away, on the work's page. */
const GENRES_SHOWN = 3

/**
 * A row, for a `<ul>`.
 *
 * `content-visibility: auto` — a list of a thousand works need not be laid out
 * to be scrolled — which also contains the row's paint, so the ring a keyboard
 * draws round it is drawn inside it.
 */
export function WorkRow({ item, to }: { item: WorkRowItem; to: string }) {
  const { t, lang, locale } = useI18n()
  const client = useQueryClient()
  const go = useTransitionNavigate()
  const plate = useRef<HTMLDivElement>(null)

  const art = poster(item)
  const rating = headlineRating(item.ratings)
  const score = fmt.score(rating?.value, locale)
  const status = statusLabel(item.status, t)
  const overview = item.overview?.trim()
  const opensWork = to === `/work/${item.id}`
  const caption = [item.year, item.kind === 'series' ? t.home.kindSeries : t.home.kindFilm].filter(Boolean).join(' · ')

  // Asked for as soon as a pointer rests on the row or focus reaches it, as a
  // card does: the page is there by the time it is opened.
  const prefetch = () => {
    if (opensWork) void client.prefetchQuery({ ...workQuery(item.id, lang), staleTime: 60_000 })
  }

  return (
    <li className="skip-offscreen [--row:9.5rem]">
      <article
        className={cn(
          'group relative grid grid-cols-[4.5rem_minmax(0,1fr)] items-start gap-x-4 px-3 py-4 sm:grid-cols-[5.5rem_minmax(0,1fr)] sm:px-4',
          'transition-colors duration-150 hover:bg-ink-raised',
          // The ring is the row's, not the title's: the row is what is pressed.
          'has-focus-visible:outline-2 has-focus-visible:-outline-offset-2 has-focus-visible:outline-vermillion',
        )}
        onPointerEnter={(event) => {
          if (event.pointerType === 'mouse') prefetch()
        }}
        onFocus={prefetch}
      >
        <div
          ref={plate}
          className={cn(
            'relative aspect-2/3 overflow-hidden rounded-card border border-rule bg-ink-high',
            'transition-colors duration-150 group-hover:border-vermillion/45',
          )}
        >
          {art ? (
            // Said by the title beside it: the picture adds no name of its own.
            <Artwork
              url={art}
              role="thumb"
              sizes="(min-width: 640px) 88px, 72px"
              alt=""
              className="size-full object-cover"
              fallback={<Placeholder title={item.title} kind={item.kind} className="size-full" />}
            />
          ) : (
            <Placeholder title={item.title} kind={item.kind} className="size-full" />
          )}
        </div>

        <div className="min-w-0">
          <h3 className="font-display text-lg leading-snug font-medium text-bone sm:text-xl">
            <Link
              to={to}
              className="line-clamp-2 transition-colors duration-200 group-hover:text-vermillion after:absolute after:inset-0 focus-visible:outline-none"
              onClick={(event) => {
                if (!plainClick(event)) return
                event.preventDefault()
                go(to, { from: opensWork ? plate.current : null })
              }}
            >
              {item.title}
            </Link>
          </h3>

          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
            <p className="font-mono text-xs tracking-wide text-bone-faint tabular-nums">{caption || '—'}</p>
            {rating?.value && score ? (
              <span
                style={{ color: `var(--color-stock-${rating.value >= 8 ? 2 : rating.value >= 6.5 ? 3 : 6})` }}
                className="font-mono text-xs font-medium tabular-nums"
              >
                <span className="sr-only">{t.a11y.ratingOf(score)}</span>
                <span aria-hidden>{score}</span>
              </span>
            ) : null}
            {status ? <span className="text-xs text-bone-dim">{status}</span> : null}
          </div>

          {item.genres.length ? (
            <ul className="mt-2 flex flex-wrap gap-1.5">
              {item.genres.slice(0, GENRES_SHOWN).map((genre) => (
                <li key={genre}>
                  <Genre name={genre} />
                </li>
              ))}
            </ul>
          ) : null}

          {overview ? (
            <p className="mt-2 line-clamp-2 max-w-prose text-sm leading-relaxed text-bone-dim">{overview}</p>
          ) : null}
        </div>
      </article>
    </li>
  )
}
