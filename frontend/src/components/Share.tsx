/**
 * The page's address, handed on.
 *
 * Where the browser has a share sheet — a phone, mostly — it opens, with the
 * title and the address; the server's own preview does the rest wherever the
 * link lands. Elsewhere the address is copied, and the copying is said.
 */

import { useEffect, useRef, useState } from 'react'

import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { Button, Glyph } from './ui'

/** How long the confirmation stays. */
const SAID_FOR = 3000

/** A share sheet is a finger's tool; a pointer copies, which it can paste. */
function hasSheet(): boolean {
  return typeof navigator.share === 'function' && window.matchMedia('(pointer: coarse)').matches
}

export function ShareButton({ title, text }: { title: string; text?: string }) {
  const { t } = useI18n()
  const [said, setSaid] = useState<'copied' | 'failed' | null>(null)
  const timer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(timer.current), [])

  const say = (what: 'copied' | 'failed') => {
    setSaid(what)
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setSaid(null), SAID_FOR)
  }

  const share = async () => {
    const url = window.location.href
    if (hasSheet()) {
      try {
        await navigator.share({ title, text, url })
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

  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <Button onClick={() => void share()}>
        <Glyph name="share" className="size-4" />
        {t.share.button}
      </Button>
      <span role="status" aria-live="polite" className={cn('text-sm', said === 'failed' ? 'text-vermillion' : 'text-moss')}>
        {said === 'copied' ? t.share.copied : said === 'failed' ? t.share.failed : ''}
      </span>
    </span>
  )
}
