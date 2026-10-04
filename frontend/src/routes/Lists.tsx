/**
 * The catalogue's selections: lists composed by whoever maintains it, shown
 * as the site's own programmes — and the addresses Sonarr and Radarr import
 * them from, since a selection made here is meant to become what those
 * clients add on their own.
 */

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Link, useParams } from 'react-router'

import { PosterCard, PosterGrid } from '../components/media'
import { Button, Chip, EmptyState, Glyph, Label, SectionTitle, Skeleton } from '../components/ui'
import { ApiError, api, query } from '../lib/api'
import { useCuratedLists, useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import type { CollectionPage, Collections, CuratedList, CuratedListPage } from '../lib/types'

export function Lists() {
  const { t } = useI18n()
  useTitle(t.lists.label)

  const lists = useCuratedLists()

  return (
    <div className="pt-10 pb-12">
      <header className="rise mb-8">
        <Label>{t.lists.label}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.lists.title}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.lists.lead}</p>
      </header>

      {lists.isPending ? (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {Array.from({ length: 3 }, (_, index) => (
            <Skeleton key={index} className="h-40 w-full" />
          ))}
        </div>
      ) : lists.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-vermillion">
          <Glyph name="alert" className="size-4" />
          {t.lists.loadFailed}
        </p>
      ) : lists.data.lists.length === 0 ? (
        <EmptyState title={t.lists.empty} hint={t.lists.emptyHint} />
      ) : (
        <ul className="stagger grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {lists.data.lists.map((list) => (
            <li key={list.id} className="min-w-0">
              <ListCard list={list} />
            </li>
          ))}
        </ul>
      )}

      <CollectionsSection />
    </div>
  )
}

/** The film collections the catalogue holds part of, beneath the lists. */
function CollectionsSection() {
  const { t } = useI18n()
  const collections = useQuery({
    queryKey: ['collections'],
    queryFn: () => api.get<Collections>('/collections'),
    staleTime: 10 * 60_000,
    retry: false,
  })
  const cards = collections.data?.collections ?? []
  if (!cards.length) return null

  return (
    <section className="mt-14">
      <SectionTitle>{t.collections.title}</SectionTitle>
      <p className="mb-6 max-w-prose text-sm leading-relaxed text-bone-dim">{t.collections.lead}</p>
      <ul className="stagger grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {cards.map((collection) => (
          <li key={collection.tmdbId} className="min-w-0">
            <Link
              to={`/collections/${collection.tmdbId}`}
              className="plate group flex gap-4 rounded-panel border border-rule bg-ink-raised p-4 transition-colors duration-150 hover:border-rule-bright"
            >
              {collection.poster ? (
                <img
                  src={collection.poster}
                  alt=""
                  width={64}
                  height={96}
                  loading="lazy"
                  className="h-24 w-16 shrink-0 rounded-sm object-cover"
                />
              ) : (
                <span className="h-24 w-16 shrink-0 rounded-sm bg-ink-high" />
              )}
              <span className="min-w-0">
                <span className="block font-display text-lg text-bone transition-colors duration-150 group-hover:text-vermillion">
                  {collection.name ?? t.collections.unnamed(collection.tmdbId)}
                </span>
                <span className="mt-1 block font-mono text-xs text-bone-faint tabular-nums">
                  {t.collections.held(collection.count, 0)}
                </span>
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </section>
  )
}

export function CollectionDetail() {
  const { id = '' } = useParams()
  const { t, lang } = useI18n()

  const page = useQuery({
    queryKey: ['collection-page', id, lang],
    queryFn: () => api.get<CollectionPage>(`/collections/${encodeURIComponent(id)}${query({ language: lang })}`),
    retry: false,
  })
  const name = page.data?.name ?? t.collections.unnamed(Number(id))
  useTitle(page.data ? name : t.collections.one)

  if (page.isPending) {
    return (
      <div className="pt-10 pb-12">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="mt-4 h-10 w-2/3" />
        <PosterGrid className="mt-10 lg:grid-cols-4 xl:grid-cols-5">
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton key={index} className="aspect-[2/3] w-full" />
          ))}
        </PosterGrid>
      </div>
    )
  }
  if (page.isError) {
    const missing = page.error instanceof ApiError && page.error.status === 404
    return (
      <div className="pt-10 pb-12">
        <EmptyState
          title={missing ? t.collections.notFound : t.collections.loadFailed}
          action={
            <Link to="/lists" className="text-sm text-vermillion underline-offset-4 hover:underline">
              {t.lists.label}
            </Link>
          }
        />
      </div>
    )
  }

  const { items, parts } = page.data
  const heldIds = new Set(items.map((item) => item.id))

  return (
    <div className="pt-10 pb-12">
      <header className="rise mb-8">
        <Link
          to="/lists"
          className="-ml-3 inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
        >
          <Glyph name="chevronLeft" className="size-4" />
          {t.lists.label}
        </Link>
        <div className="mt-2">
          <Label>{t.collections.one}</Label>
        </div>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{name}</h1>
        {page.data.overview ? (
          <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{page.data.overview}</p>
        ) : null}
        <p className="mt-2 font-mono text-sm text-bone-faint tabular-nums">
          {t.collections.held(items.length, parts.length)}
        </p>
      </header>

      {items.length ? (
        <PosterGrid className="lg:grid-cols-4 xl:grid-cols-5">
          {items.map((item) => (
            <PosterCard key={item.id} item={item} to={`/work/${item.id}`} />
          ))}
        </PosterGrid>
      ) : (
        <EmptyState title={t.collections.notFound} />
      )}

      {parts.length ? (
        <section className="mt-14">
          <SectionTitle>{t.collections.parts}</SectionTitle>
          <ol className="divide-y divide-rule rounded-panel border border-rule">
            {parts.map((part) => (
              <li key={part.tmdbId} className="flex items-center gap-3 px-4 py-2">
                {part.poster ? (
                  <img src={part.poster} alt="" width={32} height={48} loading="lazy" className="h-12 w-8 shrink-0 rounded-sm object-cover" />
                ) : (
                  <span className="h-12 w-8 shrink-0 rounded-sm bg-ink-high" />
                )}
                <span className="min-w-0 flex-1 text-sm break-words text-bone">
                  {part.held && heldIds.has(part.held) ? (
                    <Link to={`/work/${part.held}`} className="underline-offset-4 hover:underline">
                      {part.title}
                    </Link>
                  ) : (
                    part.title
                  )}
                  {part.year ? <span className="text-bone-faint"> · {part.year}</span> : null}
                </span>
                <Chip tone={part.held ? 'neutral' : 'accent'}>{part.held ? t.collections.inCatalogue : t.collections.missing}</Chip>
              </li>
            ))}
          </ol>
        </section>
      ) : null}
    </div>
  )
}

function ListCard({ list }: { list: CuratedList }) {
  const { t } = useI18n()

  return (
    <Link
      to={`/lists/${list.slug}`}
      className="plate group flex h-full flex-col gap-3 rounded-panel border border-rule bg-ink-raised p-5 transition-colors duration-150 hover:border-rule-bright"
    >
      <div className="flex flex-wrap items-center gap-2">
        <Chip>{t.lists.kind[list.kind]}</Chip>
        {list.isPublic ? null : <Chip tone="accent">{t.lists.private}</Chip>}
      </div>
      <h2 className="font-display text-xl text-bone transition-colors duration-150 group-hover:text-vermillion">
        {list.name}
      </h2>
      {list.description ? (
        <p className="line-clamp-3 text-sm leading-relaxed text-bone-dim">{list.description}</p>
      ) : null}
      <p className="mt-auto font-mono text-xs text-bone-faint tabular-nums">
        {list.mode === 'filter' ? t.lists.byFilter : t.lists.count(list.itemCount)}
      </p>
    </Link>
  )
}

export function ListDetail() {
  const { slug = '' } = useParams()
  const { t, lang } = useI18n()

  const page = useQuery({
    queryKey: ['list', slug, lang],
    queryFn: () =>
      api.get<CuratedListPage>(`/lists/${encodeURIComponent(slug)}${query({ language: lang })}`),
  })
  useTitle(page.data?.list.name ?? t.lists.one)

  if (page.isPending) {
    return (
      <div className="pt-10 pb-12">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="mt-4 h-10 w-2/3" />
        <PosterGrid className="mt-10 lg:grid-cols-4 xl:grid-cols-5">
          {Array.from({ length: 5 }, (_, index) => (
            <Skeleton key={index} className="aspect-[2/3] w-full" />
          ))}
        </PosterGrid>
      </div>
    )
  }

  if (page.isError) {
    const missing = page.error instanceof ApiError && page.error.status === 404
    return (
      <div className="pt-10 pb-12">
        <EmptyState
          title={missing ? t.lists.notFound : t.lists.loadFailed}
          action={
            <Link to="/lists" className="text-sm text-vermillion underline-offset-4 hover:underline">
              {t.lists.label}
            </Link>
          }
        />
      </div>
    )
  }

  const { list, items, total } = page.data

  return (
    <div className="pt-10 pb-12">
      <header className="rise mb-8">
        <Link
          to="/lists"
          className="-ml-3 inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
        >
          <Glyph name="chevronLeft" className="size-4" />
          {t.lists.label}
        </Link>
        <div className="mt-2">
          <Label>{t.lists.one}</Label>
        </div>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{list.name}</h1>
        {list.description ? (
          <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{list.description}</p>
        ) : null}
        <p className="mt-2 font-mono text-sm text-bone-faint tabular-nums">
          {t.lists.count(total)} · {t.lists.kind[list.kind]} ·{' '}
          {list.mode === 'filter' ? t.lists.byFilter : t.lists.byHand}
          {list.isPublic ? '' : ` · ${t.lists.private}`}
        </p>
      </header>

      {items.length === 0 ? (
        <EmptyState title={t.lists.noWorks} />
      ) : (
        <PosterGrid className="lg:grid-cols-4 xl:grid-cols-5">
          {items.map((item) => (
            <PosterCard key={item.id} item={item} to={`/work/${item.id}`} kind={list.kind === 'mixed'} />
          ))}
        </PosterGrid>
      )}

      <section aria-label={t.lists.importTitle} className="mt-14">
        <ImportAddresses list={list} />
      </section>
    </div>
  )
}

/**
 * Where the clients import a list from: one address a shape, each with a
 * button to copy it, since the address is typed into another program.
 */
export function ImportAddresses({ list }: { list: CuratedList }) {
  const { t } = useI18n()
  const origin = typeof window === 'undefined' ? '' : window.location.origin
  const base = `${origin}/api/v1/lists/${list.slug}`
  const shapes: { key: string; label: string; url: string; kinds: CuratedList['kind'][] }[] = [
    { key: 'sonarr', label: t.lists.sonarr, url: `${base}/sonarr.json`, kinds: ['series', 'mixed'] },
    { key: 'radarr', label: t.lists.radarr, url: `${base}/radarr.json`, kinds: ['movie', 'mixed'] },
    { key: 'stevenlu', label: t.lists.stevenlu, url: `${base}/stevenlu.json`, kinds: ['movie', 'mixed'] },
  ]

  return (
    <div className="plate rounded-panel border border-rule bg-ink-raised p-5">
      <h2 className="font-display text-lg text-bone">{t.lists.importTitle}</h2>
      <p className="mt-1 max-w-prose text-sm leading-relaxed text-bone-dim">{t.lists.importHint}</p>
      <ul className="mt-4 space-y-3">
        {shapes
          .filter((shape) => shape.kinds.includes(list.kind))
          .map((shape) => (
            <li key={shape.key} className="min-w-0">
              <p className="text-xs text-bone-dim">{shape.label}</p>
              <div className="mt-1 flex items-center gap-2">
                <code className="min-w-0 flex-1 truncate rounded-card border border-rule bg-ink px-3 py-2 font-mono text-xs text-bone">
                  {shape.url}
                </code>
                <CopyButton text={shape.url} />
              </div>
            </li>
          ))}
      </ul>
    </div>
  )
}

function CopyButton({ text }: { text: string }) {
  const { t } = useI18n()
  const [copied, setCopied] = useState(false)

  return (
    <Button
      size="sm"
      onClick={(event) => {
        const done = () => {
          setCopied(true)
          window.setTimeout(() => setCopied(false), 2000)
        }
        // Off a secure context there is no clipboard to write to; the
        // address is selected instead, a keystroke from copied.
        const fallback = () => {
          const code = event.currentTarget.parentElement?.querySelector('code')
          if (!code) return
          const range = document.createRange()
          range.selectNodeContents(code)
          const selection = window.getSelection()
          selection?.removeAllRanges()
          selection?.addRange(range)
        }
        if (navigator.clipboard?.writeText) {
          navigator.clipboard.writeText(text).then(done).catch(fallback)
        } else {
          fallback()
        }
      }}
      aria-live="polite"
    >
      <Glyph name="copy" className="size-3.5" />
      {copied ? t.lists.copied : t.lists.copy}
    </Button>
  )
}
