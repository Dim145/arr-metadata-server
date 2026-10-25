/**
 * A work's page, handed on.
 *
 * Where the browser has a share sheet — a phone, mostly — it opens, with the
 * title and the address; the server's own preview does the rest wherever the
 * link lands. Elsewhere the address is copied, and the copying is said.
 */

import { useEffect, useRef, useState } from 'react'

import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import type { MediaItem } from '../lib/types'
import { Button, Glyph } from './ui'

/** How long the confirmation stays. */
const SAID_FOR = 3000

/** A share sheet is a finger's tool; a pointer copies, which it can paste. */
function hasSheet(): boolean {
  return typeof navigator.share === 'function' && window.matchMedia('(pointer: coarse)').matches
}

/** What a work's page needs of the work to hand it on. */
type Shared = Pick<MediaItem, 'title' | 'year' | 'kind'>

/** What the last press came to, while it is still said. */
export type Said = 'copied' | 'failed' | null

/**
 * The handing on itself, for each place that offers it — the plate's button
 * on a wider screen, the bar along a phone's foot — so that both hand the
 * page on alike, and each says so in its own place: the press, what it came
 * to, and the words for that, empty while there is nothing to say.
 */
export function useShare(item: Shared): { share: () => void; said: Said; words: string } {
  const { t } = useI18n()
  const [said, setSaid] = useState<Said>(null)
  const timer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(timer.current), [])

  const say = (what: 'copied' | 'failed') => {
    setSaid(what)
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setSaid(null), SAID_FOR)
  }

  const share = async () => {
    // The page, not the place on it: a press on "Watch" or on a section's
    // chip left its anchor in the address, and whoever the link reaches
    // starts at the top.
    const address = new URL(window.location.href)
    address.hash = ''
    const url = address.href
    if (hasSheet()) {
      const text = [item.year, item.kind === 'series' ? t.nav.series : t.nav.films].filter(Boolean).join(' · ')
      try {
        await navigator.share({ title: item.title, text, url })
      } catch {
        // Closed without choosing anything: nothing to say.
      }
      return
    }
    try {
      await navigator.clipboard.writeText(url)
      say('copied')
    } catch {
      say('failed')
    }
  }

  return {
    share: () => void share(),
    said,
    words: said === 'copied' ? t.share.copied : said === 'failed' ? t.share.failed : '',
  }
}

/** The plate's button, with what it came to beside it. */
export function ShareButton({ item }: { item: Shared }) {
  const { t } = useI18n()
  const { share, said, words } = useShare(item)

  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <Button onClick={share}>
        <Glyph name="share" className="size-4" />
        {t.share.button}
      </Button>
      <span role="status" aria-live="polite" className={cn('text-sm', said === 'failed' ? 'text-status-error' : 'text-status-ok')}>
        {words}
      </span>
    </span>
  )
}
