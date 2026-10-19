/**
 * A count on the installed app's icon: the episodes airing today.
 *
 * Only where the catalogue runs as an installed app (`display-mode:
 * standalone`) and the system badges an app's icon (`navigator.setAppBadge`:
 * Chromium's installed apps, Safari's added to the Dock or the Home Screen).
 * The count goes on the icon when the app goes behind another and comes off
 * when it is brought back: it is there for a reader who is not looking at the
 * schedule, and one who has the app in front of them has the schedule for
 * that. No notification, no permission asked, nothing sent anywhere: the
 * number is the one the schedule itself would show for today.
 */

import { api, query } from './api'
import { localDay } from './format'
import { airTime } from './media'
import type { Calendar } from './types'

/**
 * Today's episodes, counted as the schedule counts them: by the reader's own
 * date, the episodes with no time of their own included — which are stored
 * at midnight UTC, some hours off the reader's midnight, so the window asked
 * for reaches both (the schedule's `windowOf`, for one day).
 */
async function airingToday(now: Date): Promise<number> {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1)
  const utcMidnight = (day: Date) => Date.UTC(day.getFullYear(), day.getMonth(), day.getDate())
  const from = new Date(Math.min(today.getTime(), utcMidnight(today))).toISOString()
  const to = new Date(Math.max(tomorrow.getTime(), utcMidnight(tomorrow))).toISOString()

  const listing = await api.get<Calendar>(`/calendar${query({ from, to })}`)
  const day = localDay(now)
  return listing.entries.filter((entry) => entry.kind === 'episode' && airTime(entry.episode)?.day === day).length
}

/**
 * Keeps the icon's count from now on: cleared as the app opens, put back as
 * it goes behind another, cleared again as it returns.
 *
 * The count is asked for while the app is in front — a page going behind
 * another may be frozen before an answer could arrive — and only a count of
 * today is ever shown: one learned yesterday says nothing tonight.
 */
export function keepBadge(): void {
  if (typeof navigator === 'undefined' || !('setAppBadge' in navigator)) return
  if (!window.matchMedia('(display-mode: standalone)').matches) return

  let known: { day: string; count: number } | undefined

  const learn = () => {
    const now = new Date()
    airingToday(now).then(
      (count) => {
        known = { day: localDay(now), count }
      },
      // Signed out of a private site, or offline: no count, no badge.
      () => {
        known = undefined
      },
    )
  }
  const clear = () => {
    navigator.clearAppBadge().catch(() => {})
  }
  const show = () => {
    if (known && known.day === localDay(new Date()) && known.count > 0) {
      navigator.setAppBadge(known.count).catch(() => {})
    } else {
      clear()
    }
  }

  clear()
  learn()
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') {
      show()
    } else {
      clear()
      learn()
    }
  })
}
