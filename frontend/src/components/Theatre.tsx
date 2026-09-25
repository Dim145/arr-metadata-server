/**
 * The room a picture or a film is shown in: dark, and the whole screen.
 *
 * Built on `<dialog>` like the rest of this interface's dialogs, so the
 * platform supplies the focus trap, the Escape key and an inert page behind,
 * and the top layer escapes the shells' `overflow-x-clip`. What differs is the
 * size: this is a projection room, not a form.
 */

import { useEffect, useRef, useState } from 'react'

import { useI18n } from '../lib/i18n'
import { providerName } from '../lib/labels'
import { trailerEmbed, trailerPage } from '../lib/links'
import type { Image } from '../lib/types'
import { ExternalLink } from './elsewhere'
import { Artwork } from './media'
import { IconButton } from './ui'

function Theatre({
  open,
  onClose,
  label,
  onKey,
  children,
}: {
  open: boolean
  onClose: () => void
  label: string
  onKey?: (event: React.KeyboardEvent<HTMLDialogElement>) => void
  children: React.ReactNode
}) {
  const ref = useRef<HTMLDialogElement>(null)

  useEffect(() => {
    const node = ref.current
    if (!node) return
    if (open && !node.open) node.showModal()
    if (!open && node.open) node.close()
  }, [open])

  return (
    <dialog
      ref={ref}
      aria-label={label}
      onClose={onClose}
      onKeyDown={onKey}
      className="m-0 h-dvh max-h-none w-screen max-w-none bg-transparent p-0 text-bone backdrop:bg-ink/95"
    >
      {open ? children : null}
    </dialog>
  )
}

/**
 * A work's artwork, one picture at a time, at the size the provider has it.
 *
 * The arrow keys move through the set, Home and End go to either end, and
 * Escape closes — the keys every image viewer has taught its readers. The
 * buttons do the same for a pointer; nothing depends on a swipe.
 */
export function Lightbox({
  images,
  index,
  onIndex,
  onClose,
  title,
}: {
  images: Image[]
  /** Which image is shown, or `null` when the lightbox is shut. */
  index: number | null
  onIndex: (next: number) => void
  onClose: () => void
  title: string
}) {
  const { t, locale } = useI18n()
  const open = index !== null && images.length > 0
  const at = index ?? 0
  const image = images[Math.min(at, images.length - 1)]

  const go = (step: number) => onIndex((at + step + images.length) % images.length)

  const onKey = (event: React.KeyboardEvent<HTMLDialogElement>) => {
    switch (event.key) {
      case 'ArrowRight':
        event.preventDefault()
        go(1)
        break
      case 'ArrowLeft':
        event.preventDefault()
        go(-1)
        break
      case 'Home':
        event.preventDefault()
        onIndex(0)
        break
      case 'End':
        event.preventDefault()
        onIndex(images.length - 1)
        break
    }
  }

  const position = new Intl.NumberFormat(locale)

  return (
    <Theatre open={open} onClose={onClose} label={t.gallery.viewer(title)} onKey={onKey}>
      {image ? (
        <div className="grid h-full grid-rows-[auto_minmax(0,1fr)_auto]">
          {/* The bar is the dialog's own, drawn on its ink: over the page's
              header, its words and the wordmark's were one tangle. */}
          <header className="flex items-center gap-3 bg-ink px-4 py-2 sm:px-6">
            <p className="font-mono text-xs text-bone-dim tabular-nums" aria-live="polite">
              {t.gallery.position(position.format(at + 1), position.format(images.length))}
            </p>
            {image.source ? (
              <span className="label text-slate">{providerName(image.source)}</span>
            ) : null}
            <span className="flex-1" />
            <ExternalLink href={image.url} className="min-h-11 text-sm text-bone-dim">
              {t.gallery.original}
            </ExternalLink>
            <IconButton glyph="close" label={t.nav.close} onClick={onClose} />
          </header>

          <div className="relative flex min-h-0 items-center justify-center px-2 sm:px-16">
            <Artwork
              key={image.url}
              url={image.url}
              role="full"
              eager
              alt={t.gallery.alt(title, at + 1)}
              className="fade-in max-h-full max-w-full rounded-card object-contain shadow-[var(--shadow-plate)]"
            />

            {images.length > 1 ? (
              <>
                <IconButton
                  glyph="chevronLeft"
                  label={t.gallery.previous}
                  onClick={() => go(-1)}
                  className="absolute top-1/2 left-1 -translate-y-1/2 bg-ink/70 text-bone sm:left-4"
                />
                <IconButton
                  glyph="chevronRight"
                  label={t.gallery.next}
                  onClick={() => go(1)}
                  className="absolute top-1/2 right-1 -translate-y-1/2 bg-ink/70 text-bone sm:right-4"
                />
              </>
            ) : null}
          </div>

          {/* The keys, where there is a keyboard to press them on; a touch
              screen gets the buttons, and a line saying nothing it can use
              would only take room from the picture. */}
          <p className="hidden px-4 py-3 text-center text-xs text-bone-faint pointer-fine:block sm:px-6">
            {t.gallery.keys}
          </p>
        </div>
      ) : null}
    </Theatre>
  )
}

/**
 * A trailer, played where it was asked for.
 *
 * Nothing is fetched from YouTube until the dialog opens: a page that loaded
 * a player on sight would tell YouTube about every visit to every work. The
 * no-cookie domain is the one this server's page policy lets frame, and the
 * frame sends YouTube this server's origin — which its player now refuses to
 * play without — and nothing more.
 */
export function Trailer({
  youtubeId,
  title,
  open,
  onClose,
}: {
  youtubeId: string
  title: string
  open: boolean
  onClose: () => void
}) {
  const { t } = useI18n()

  return (
    <Theatre open={open} onClose={onClose} label={t.trailer.of(title)}>
      <div className="flex h-full flex-col items-center justify-center gap-4 px-4 py-6">
        <div className="flex w-full max-w-5xl items-center justify-between gap-3">
          <p className="min-w-0 truncate font-display text-lg text-bone">{t.trailer.of(title)}</p>
          <div className="flex shrink-0 items-center gap-1">
            <ExternalLink href={trailerPage(youtubeId)} className="min-h-11 px-2 text-sm text-bone-dim">
              {t.trailer.onYoutube}
            </ExternalLink>
            <IconButton glyph="close" label={t.nav.close} onClick={onClose} />
          </div>
        </div>

        <div className="aspect-video w-full max-w-5xl overflow-hidden rounded-panel border border-rule bg-ink">
          <iframe
            src={trailerEmbed(youtubeId)}
            title={t.trailer.of(title)}
            className="size-full"
            allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
            allowFullScreen
            referrerPolicy="strict-origin"
          />
        </div>
      </div>
    </Theatre>
  )
}

/** The lightbox's state, for a page that opens it from several places. */
export function useLightbox() {
  const [index, setIndex] = useState<number | null>(null)
  return { index, open: setIndex, close: () => setIndex(null), move: setIndex }
}
