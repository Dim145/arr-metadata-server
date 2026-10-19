/**
 * One work, in full.
 *
 * The page is a catalogue entry: a plate at the top, then the record. Seasons
 * are stacked scrollers rather than tabs, so a whole series is on one page and
 * nothing is hidden behind a state the URL does not carry.
 *
 * On a screen narrower than its two columns the page is long — the record,
 * the cast, the seasons, the pictures, one under the other: eight thousand
 * pixels on a phone — so it says what it holds. A row of its sections stays
 * under the bar at the top; the longest lists are folded, and found all the
 * same by the browser's search; and a bar of its own along the foot of a
 * phone keeps where to watch it, its trailer and its address under the thumb.
 *
 * The plate, the sections and the record are in `work/`.
 */

import { useEffect, useState, useSyncExternalStore } from 'react'
import { useParams } from 'react-router'

import { Trailer } from '../components/Theatre'
import { OnThisPage, Skeleton } from '../components/ui'
import { ApiError } from '../lib/api'
import { cn } from '../lib/cn'
import { useMe, useOrders, useTitle, useWork } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { cast, episodesOf, seasonNumbers } from '../lib/media'
import { remember } from '../lib/recent'
import { pickedRegion, readerRegion } from '../lib/releases'
import { POSTER } from '../lib/transitions'
import { NotFound, Unavailable } from './NotFound'
import { NumberingBanner } from './admin/editor/NumberingBanner'
import { FootActions } from './work/bar'
import { Plate } from './work/Plate'
import { Identifiers, Ratings, Record, Releases, WhereToWatch, useWatch, watchable } from './work/record'
import {
  AlsoKnownAs,
  CastList,
  Collection,
  Gallery,
  OnAir,
  Related,
  Seasons,
  Similar,
  Synopsis,
  hasGallery,
  otherTitles,
  useSimilar,
} from './work/sections'

/** As wide as the page's two columns, where the record stands beside the rest. */
const TWO_COLUMNS = '(min-width: 64rem)'

/** Whether the page has its two columns, and with them no need of an index. */
function useTwoColumns(): boolean {
  return useSyncExternalStore(
    (changed) => {
      const media = window.matchMedia(TWO_COLUMNS)
      media.addEventListener('change', changed)
      return () => media.removeEventListener('change', changed)
    },
    () => window.matchMedia(TWO_COLUMNS).matches,
    () => true,
  )
}

export function Work() {
  const { id = '' } = useParams()
  const work = useWork(id)
  const me = useMe()
  // A series' orders say who numbers it: TMDB, while TheTVDB does not answer
  // for it, is said in a banner. A film has none to ask for.
  const orders = useOrders(work.data?.kind === 'series' ? id : '')
  const { t, locale } = useI18n()
  useTitle(work.data?.title)
  // The country picked to see where the work is watched, kept in this
  // browser: the one its dates and its rating are read for too.
  const [picked, setPicked] = useState<string | undefined>(pickedRegion)
  const region = readerRegion(me.data, locale, picked)
  // Played from the plate on a wide screen, from the bar along the foot of a
  // phone: one player either way.
  const [trailer, setTrailer] = useState(false)
  const columns = useTwoColumns()
  // Asked here as the sections ask them, so the page's index offers a way
  // only to what is there.
  const watch = useWatch(work.data?.id, picked)
  const similar = useSimilar(work.data?.id)

  // Noted as opened, for the shelf of recent works and the palette's list.
  useEffect(() => {
    if (work.data) remember(work.data)
  }, [work.data])

  // What is on screen stays through a refetch that failed; only a read that
  // never came back decides between "gone" and "try again".
  if (!work.data) {
    if (work.isPending) return <WorkSkeleton />
    if (work.error instanceof ApiError && work.error.status === 404) return <NotFound work />
    return <Unavailable onRetry={() => void work.refetch()} />
  }

  const item = work.data
  const credits = cast(item.credits)
  const titles = otherTitles(item)
  const sections = [
    { id: 'overview', label: t.work.overview },
    { id: 'details', label: t.work.details },
    watchable(watch.data) ? { id: 'watch', label: t.work.watch } : undefined,
    credits.length ? { id: 'cast', label: t.work.cast } : undefined,
    item.kind === 'series' && seasonNumbers(item).some((n) => episodesOf(item, n).length)
      ? { id: 'seasons', label: t.work.seasons }
      : undefined,
    hasGallery(item) ? { id: 'gallery', label: t.gallery.title } : undefined,
    similar.data?.items.length ? { id: 'similar', label: t.work.similar } : undefined,
    titles.length || item.keywords.some((k) => k.trim())
      ? { id: 'titles', label: titles.length ? t.work.alsoKnownAs : t.work.keywords }
      : undefined,
  ].filter((section) => section !== undefined)

  return (
    // Keyed by the work, so moving from one to another starts every section
    // afresh: the gallery's tab and the lists opened out were the last one's.
    <article key={item.id} className="pb-8">
      <Plate item={item} region={region} onTrailer={() => setTrailer(true)} />

      {/* Drawn while TMDB numbers the series, and kept once it is given back
          to say so: the banner itself is nothing otherwise. */}
      {orders.data ? (
        <NumberingBanner workId={item.id} orders={orders.data} canWrite={me.data?.canWrite === true} className="mt-8" />
      ) : null}

      {/* One column, one long page: its sections in a row of chips that stays
          under the bar at the top — the administration's own index, drawn
          as it is there, held a bar's height lower. Two columns have no need
          of it, and are not given one to hide. */}
      {columns ? null : (
        <div className="contents [&>nav]:top-[calc(4rem+1px)] [&>nav]:mt-8">
          <OnThisPage entries={sections} label={t.work.onThisPage} />
        </div>
      )}

      {/* The facts sit beside the page on a wide screen, and straight after the
          synopsis on a phone: status, dates, network and identifiers are what
          the page is opened for, and at the foot of a long page they were
          past the cast, the seasons and every picture. */}
      <div
        className={cn(
          'grid gap-x-12 gap-y-14 lg:grid-cols-[minmax(0,1fr)_20rem] lg:grid-rows-[auto_1fr]',
          columns ? 'mt-12' : 'mt-6',
        )}
      >
        <div className="min-w-0 space-y-14 lg:col-start-1 lg:row-start-1">
          <Synopsis item={item} />
          {item.kind === 'series' ? <OnAir item={item} /> : null}
        </div>

        <aside className="space-y-6 lg:col-start-2 lg:row-span-2 lg:row-start-1">
          <Record item={item} region={region} />
          <Releases item={item} region={region} />
          <WhereToWatch item={item} picked={picked} onPick={setPicked} />
          <Ratings item={item} />
          <Identifiers item={item} />
        </aside>

        <div className="min-w-0 space-y-14 lg:col-start-1 lg:row-start-2">
          <CastList credits={credits} />
          {item.kind === 'series' ? <Seasons item={item} /> : null}
          <Gallery item={item} />
          {item.kind === 'movie' && item.collectionTmdbId ? <Collection item={item} /> : null}
          <Related item={item} />
          <Similar item={item} />
          <AlsoKnownAs item={item} />
        </div>
      </div>

      <FootActions item={item} canWatch={watchable(watch.data)} onTrailer={() => setTrailer(true)} />

      {item.trailerYoutubeId ? (
        <Trailer youtubeId={item.trailerYoutubeId} title={item.title} open={trailer} onClose={() => setTrailer(false)} />
      ) : null}
    </article>
  )
}

/* ── Waiting ──────────────────────────────────────────────────────────────── */

function WorkSkeleton() {
  return (
    <div className="pb-8">
      <div className="mx-[calc(50%-50vw)] h-56 w-screen sm:h-72 lg:h-[22rem]">
        <Skeleton className="size-full rounded-none" />
      </div>
      <div className="-mt-24 flex gap-6 sm:-mt-28">
        {/* Where the plate's poster will be, named as it: a card opened before
            the page is read travels here, and the poster then fades in. */}
        <div className="w-32 shrink-0 sm:w-40 lg:w-48" data-travels={POSTER} style={{ viewTransitionName: POSTER }}>
          <Skeleton className="aspect-2/3 w-full rounded-plate" />
        </div>
        <div className="flex-1 space-y-3 pt-24 sm:pt-28">
          <Skeleton className="h-10 w-2/3" />
          <Skeleton className="h-4 w-1/3" />
        </div>
      </div>
      <div className="mt-12 space-y-4">
        <Skeleton className="h-7 w-40" />
        <Skeleton className="h-24 w-full" />
      </div>
    </div>
  )
}
