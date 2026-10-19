/**
 * The bar along the foot of a phone, on a work's page: what the thumb is
 * there for — where to watch it, its trailer, its address — in the place the
 * site's own bar keeps, which it stands in for on this page alone. TMDB puts
 * "Watch" in a work's header; here it was the last panel of a page eight
 * thousand pixels tall.
 *
 * Sticky rather than fixed: it rides along the foot of the screen while the
 * page goes on, and comes to rest at the page's end, above the footer, which
 * it never covers. A phone's home indicator is given its inset. Cells as the
 * site's bar has them — the glyph above its word — so a thumb finds the same
 * shape in the same place.
 */

import { useFootBar } from '../../components/PublicShell'
import { useShare } from '../../components/Share'
import { Glyph } from '../../components/ui'
import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import type { MediaItem } from '../../lib/types'

const CELL =
  'relative flex min-h-14 min-w-0 cursor-pointer flex-col items-center justify-center gap-1 px-0.5 transition-colors duration-150'
const CAPTION = 'text-center text-caption leading-tight font-medium tracking-tight whitespace-nowrap'

export function FootActions({
  item,
  canWatch,
  onTrailer,
}: {
  item: MediaItem
  /** Whether the panel of services lists any, which "Watch" leads to. */
  canWatch: boolean
  onTrailer: () => void
}) {
  const { t } = useI18n()
  useFootBar()

  return (
    <div
      role="group"
      aria-label={t.work.actions}
      className="sticky bottom-0 z-30 -mx-4 mt-10 border-t border-rule bg-ink-raised pb-[env(safe-area-inset-bottom)] sm:-mx-6 md:hidden"
    >
      <div className="grid auto-cols-fr grid-flow-col">
        {canWatch ? (
          <a href="#watch" className={cn(CELL, 'text-vermillion hover:text-vermillion-bright')}>
            <Glyph name="tv" className="size-5" />
            <span className={CAPTION}>{t.work.watchNow}</span>
          </a>
        ) : null}
        {item.trailerYoutubeId ? (
          <button type="button" onClick={onTrailer} className={cn(CELL, 'text-bone-dim hover:text-bone')}>
            <Glyph name="play" className="size-5" />
            <span className={CAPTION}>{t.trailer.play}</span>
          </button>
        ) : null}
        <ShareCell item={item} />
      </div>
    </div>
  )
}

/**
 * The page's address, handed on as the plate's button hands it on a wider
 * screen (`useShare`): the share sheet where there is one, the address
 * copied where there is not — and the copying said above the bar, where the
 * eye is, and to a screen reader as it happens.
 */
function ShareCell({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const { share, said, words } = useShare(item)

  return (
    <div className="relative flex">
      <button type="button" onClick={share} className={cn(CELL, 'flex-1 text-bone-dim hover:text-bone')}>
        <Glyph name="share" className="size-5" />
        <span className={CAPTION}>{t.share.button}</span>
      </button>
      <span
        role="status"
        aria-live="polite"
        className={cn(
          'pointer-events-none absolute right-2 bottom-full mb-2 rounded-full border bg-ink-raised px-3 py-1 text-caption whitespace-nowrap',
          'transition-opacity duration-150',
          said ? 'opacity-100' : 'opacity-0',
          said === 'failed' ? 'border-vermillion-deep text-vermillion' : 'border-rule text-moss',
        )}
      >
        {words}
      </span>
    </div>
  )
}
