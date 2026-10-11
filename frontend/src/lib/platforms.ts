/**
 * The streaming services whose "air time" is a release day, not a broadcast.
 *
 * A platform puts a season up at midnight UTC — or TheTVDB files it so, having
 * no better figure to give — and the providers hand that on as the moment the
 * episode aired. Read as a clock, it said "02:00 (your time) · 00:00 UTC", an
 * hour nobody was up for and nobody chose. For these services, an episode
 * timed at midnight UTC is "available on the 16th": a day, the one that
 * midnight opens, the same for every reader. A network's episode keeps its
 * clock time, and every other page — the schedule, the feeds — its own.
 *
 * The list is short on purpose: a name here changes how a date is read, so it
 * holds the services known to release this way and nothing guessed.
 */

import { dayDistance } from './format'
import { airsWhen, type AirTime } from './media'

const STREAMING_PLATFORMS = [
  'netflix',
  'prime video',
  'amazon',
  'disney+',
  'apple tv+',
  // Apple TV+ under the name it has gone by since October 2025.
  'apple tv',
  'max',
  'hbo max',
  'paramount+',
  'crunchyroll',
  'hulu',
  'peacock',
]

/**
 * Whether a network is one of the platforms, by name as a provider wrote it:
 * "Amazon Prime Video" is Amazon and Prime Video both, "HBO Max" is Max, and
 * "Maxx" is neither — a name counts whole, not as a fragment of a longer one.
 */
export function isStreamingPlatform(network: string | undefined): boolean {
  if (!network) return false
  const name = ` ${network.trim().toLowerCase().replace(/\s+/g, ' ')} `
  return STREAMING_PLATFORMS.some((platform) => name.includes(` ${platform} `))
}

/**
 * Whether an episode's time is a platform's midnight: a release day rather
 * than a moment it was on. Only for a moment a provider gave — a bare date
 * is already a day — and only where the work's network is a platform.
 */
export function releasedNotAired(network: string | undefined, time: AirTime | undefined): boolean {
  const moment = time?.moment
  if (!moment || !isStreamingPlatform(network)) return false
  return moment.getUTCHours() === 0 && moment.getUTCMinutes() === 0
}

/**
 * The day a platform put an episode up: the UTC day its midnight opens, the
 * same for every reader. A `YYYY-MM-DD` day, which the formatters read in UTC,
 * so a reader west of Greenwich is not shown the evening before it.
 *
 * Not the date filed beside the moment, which is the American evening before:
 * Stranger Things' first season is filed on 15 July 2016 and timed at
 * midnight UTC on the 16th. The page dated it by the moment, the 16th, and
 * still does — for a reader in Los Angeles too, who was shown the 15th at
 * five in the afternoon.
 */
export function releaseDay(time: AirTime): string {
  return time.moment ? time.moment.toISOString().slice(0, 10) : time.day
}

/**
 * How far off an episode is, as its page dates it. A platform's in days from
 * its release day — "tomorrow" — where the hours to its midnight said "in 7
 * hours" on the evening before the day the page names; anything else as
 * `airsWhen` says it.
 */
export function airsWhenOn(network: string | undefined, time: AirTime, locale: string): string {
  return releasedNotAired(network, time) ? dayDistance(releaseDay(time), locale) : airsWhen(time, locale)
}
