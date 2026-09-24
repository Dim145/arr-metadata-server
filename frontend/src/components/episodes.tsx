/**
 * What only an episode has: where a special belongs, and what kind of finale
 * it is. Shared by the season sheet and the episode's own page.
 */

import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { episodeCode } from '../lib/media'
import type { Episode } from '../lib/types'

/**
 * Where a special belongs among the regular episodes, as TheTVDB files it.
 * It is what decides where a player slots it in an order-of-broadcast watch.
 */
export function Placement({ episode, className }: { episode: Episode; className?: string }) {
  const { t } = useI18n()

  const text =
    episode.airedBeforeSeasonNumber !== undefined && episode.airedBeforeEpisodeNumber !== undefined
      ? t.episode.airsBefore(episodeCode({ seasonNumber: episode.airedBeforeSeasonNumber, episodeNumber: episode.airedBeforeEpisodeNumber }))
      : episode.airedAfterSeasonNumber !== undefined
        ? t.episode.airsAfter(episode.airedAfterSeasonNumber)
        : undefined

  if (!text) return null

  return <p className={cn('text-xs text-slate', className)}>{text}</p>
}

/** `season`, `series`, `midseason`: the kinds of finale TheTVDB marks. */
export function finaleLabel(kind: string, t: ReturnType<typeof useI18n>['t']): string {
  const known = t.episode.finales as Record<string, string>
  return known[kind] ?? kind
}
