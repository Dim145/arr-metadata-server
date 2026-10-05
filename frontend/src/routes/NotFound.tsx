/**
 * An address with nothing behind it.
 *
 * It used to redirect to the front page without a word, so a stale link — an
 * old bookmark, a work since removed, a typo in a URL somebody pasted — landed
 * on the catalogue as if that were what had been asked for. Saying so, and
 * offering the two ways on, is the whole of this page.
 */

import { Link } from 'react-router'

import { EmptyState, Glyph } from '../components/ui'
import { useI18n } from '../lib/i18n'

/**
 * A work that could not be read just now, as opposed to one that is gone.
 *
 * Usually a moment — the server restarting, the network — so the first way
 * out is to ask again. Shared by a work's page and its season and episode
 * pages, which read the same record, and by the boundary that stands in for
 * a page that threw, which words the hint itself and, inside the
 * administration, leads back to its dashboard.
 */
export function Unavailable({
  onRetry,
  hint,
  admin = false,
}: {
  onRetry: () => void
  hint?: string
  admin?: boolean
}) {
  const { t } = useI18n()

  return (
    <div className="pt-16">
      <EmptyState
        title={t.common.error}
        hint={hint ?? t.work.loadFailedHint}
        action={
          <div className="mt-2 flex flex-wrap items-center justify-center gap-2">
            <button
              type="button"
              onClick={onRetry}
              className="inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-card border border-rule-bright px-4 text-sm text-bone transition-colors duration-200 hover:bg-ink-high"
            >
              <Glyph name="refresh" className="size-4" />
              {t.common.retry}
            </button>
            <Link
              to={admin ? '/admin' : '/'}
              className="inline-flex min-h-11 items-center gap-2 rounded-card px-4 text-sm text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-bone"
            >
              <Glyph name="arrowLeft" className="size-4" />
              {admin ? t.admin.dashboard : t.work.back}
            </Link>
          </div>
        }
      />
    </div>
  )
}

export function NotFound({ work = false, admin = false }: { work?: boolean; admin?: boolean }) {
  const { t } = useI18n()

  // Inside the administration side the way back is the dashboard, not the
  // public catalogue an operator did not come from.
  const home = admin ? '/admin' : '/'
  const homeLabel = admin ? t.admin.dashboard : t.notFound.home

  return (
    <section className="rise mx-auto flex max-w-xl flex-col items-start gap-4 py-24">
      <span className="label text-vermillion">404</span>
      <h1 className="font-display text-3xl font-medium text-bone sm:text-4xl">
        {work ? t.notFound.workTitle : t.notFound.title}
      </h1>
      <p className="max-w-prose text-sm leading-relaxed text-bone-dim">
        {work ? t.notFound.workBody : t.notFound.body}
      </p>

      <div className="mt-2 flex flex-wrap items-center gap-2">
        <Link
          to={home}
          className="inline-flex min-h-11 items-center gap-2 rounded-full border border-rule-bright px-5 text-sm font-medium text-bone transition-colors duration-200 hover:border-bone-faint hover:bg-ink-high"
        >
          <Glyph name="arrowLeft" className="size-4" />
          {homeLabel}
        </Link>
        {admin ? null : (
          <Link
            to="/browse"
            className="inline-flex min-h-11 items-center gap-2 rounded-full px-4 text-sm font-medium text-bone-dim transition-colors duration-200 hover:bg-ink-high hover:text-bone"
          >
            {t.notFound.browse}
          </Link>
        )}
      </div>
    </section>
  )
}
