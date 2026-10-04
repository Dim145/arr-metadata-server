/**
 * Every picture the work carries, laid out as a light table rather than a
 * list of addresses: a poster is recognised by looking at it, never by its
 * URL. Grouped by kind, and beside each the marks that matter here — the one
 * chosen to lead with, the ones a person added, and what this server keeps a
 * copy of.
 *
 * The work's own pictures only: a season's are decided where the season is,
 * on its own panel, and a line here says how many there are and leads there.
 *
 * A manual image survives every refresh; a provider's cannot be removed here
 * at all, because it would simply come back and read as the delete having
 * failed. So the remove control exists only where it means something.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useRef, useState } from 'react'
import { Link } from 'react-router'

import { Artwork as Picture } from '../../../components/media'
import { Button, Chip, FormField, Glyph, IconButton, Input, Panel, PanelHead, Select } from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import { useI18n } from '../../../lib/i18n'
import { providerName } from '../../../lib/labels'
import { seasonImages } from '../../../lib/media'
import type { Image, MediaItem, Uploaded, WorkMedia, WorkMedium } from '../../../lib/types'

import { ImagePicker } from './ImagePicker'
import { Count, Empty, hostOf, useRemove } from './shared'

/** The kinds in the order the table shows them; anything else follows. */
const KINDS = ['poster', 'fanart', 'landscape', 'banner', 'clearlogo', 'clearart', 'characterart']

/** The kinds offered when adding one by hand. */
const ADDABLE = ['poster', 'fanart', 'banner', 'clearlogo', 'landscape', 'clearart'] as const

/** The lock a work-level poster or background is chosen with; none for the rest. */
type ChoiceField = 'primaryPoster' | 'primaryFanart'
function choiceFor(image: Image): ChoiceField | null {
  if (image.seasonNumber !== undefined && image.seasonNumber !== null) return null
  return image.coverType === 'poster' ? 'primaryPoster' : image.coverType === 'fanart' ? 'primaryFanart' : null
}

const isWide = (kind: string) => kind !== 'poster'
const isDrawn = (kind: string) => kind === 'clearlogo' || kind === 'clearart' || kind === 'characterart'

export function ArtworkTab({ work, onChanged }: { work: MediaItem; onChanged: () => void }) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const a = t.admin.editor.artworkTab
  const p = t.admin.editor.picker
  const queryClient = useQueryClient()
  const [kind, setKind] = useState<string>('all')
  // The kinds laid out whole in the table of everything: a dozen of each
  // are drawn, and a work with sixty posters asks before drawing the rest.
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const { ask, dialog } = useRemove(work, onChanged)

  // What is kept of each address the work points at: shown beside the
  // picture, and the way to forget a copy.
  const media = useQuery({
    queryKey: ['item', work.id, 'media'],
    queryFn: () => api.get<WorkMedia>(`/items/${work.id}/media`),
    refetchInterval: (q) => (q.state.data?.media.some((m) => m.status === 'pending') ? 5_000 : false),
  })
  const keptOf = (url: string) => media.data?.media.find((m) => m.url === url || m.origin === url)
  const storeOn = media.data?.store ?? false

  const done = () => {
    void queryClient.invalidateQueries({ queryKey: ['item', work.id, 'media'] })
    onChanged()
  }
  const forget = useMutation({
    mutationFn: (assetId: string) => api.delete(`/items/${work.id}/media/${assetId}`),
    onSuccess: done,
  })
  // The poster and the background to lead with: a lock like any edit, set
  // by the star and taken off by it again. None is chosen by default.
  const chosen = work.primaryImages ?? {}
  const choose = useMutation({
    mutationFn: ({ field, url }: { field: ChoiceField; url: string | null }) =>
      url === null
        ? api.delete(`/items/${work.id}/overrides/item/${field}`)
        : api.put(`/items/${work.id}/overrides`, { scope: 'item', field, value: url }),
    onSuccess: done,
  })

  // Adding one by hand: a kind, maybe a language, and the picture from a
  // file or an address — the one picker every picture is chosen with.
  const [adding, setAdding] = useState(false)
  const [addKind, setAddKind] = useState<string>('poster')
  const [addLanguage, setAddLanguage] = useState('')

  const own = (work.images ?? []).filter((image) => image.seasonNumber === undefined || image.seasonNumber === null)
  // The seasons' pictures are counted, not shown: their panels are where they are decided.
  const seasonal = (work.seasons ?? []).reduce((n, season) => n + seasonImages(work, season.seasonNumber).length, 0)
  const kinds = [...KINDS.filter((k) => own.some((image) => image.coverType === k)), ...[...new Set(own.map((image) => image.coverType))].filter((k) => !KINDS.includes(k))]
  const kindLabel = (value: string) => (t.gallery.kind as Record<string, string>)[value] ?? value
  const coverLabel = (value: string) =>
    ({ poster: c.poster, fanart: c.fanart, banner: c.banner, clearlogo: c.clearlogo, landscape: c.landscape, clearart: c.clearart } as Record<string, string>)[value] ??
    value

  // Under a heading that already names the kind, the caption says only
  // where the picture is from.
  const card = (image: Image) => {
    const kept = keptOf(image.url)
    const field = choiceFor(image)
    const primary = field !== null && (field === 'primaryPoster' ? chosen.poster : chosen.fanart) === image.id
    // Chosen, and no longer listed by its sources: kept for the choice,
    // with no row of its own to delete.
    const keptForChoice = image.id.startsWith('chosen-')
    return (
      <ArtCard
        key={image.id}
        image={image}
        kept={kept}
        primary={primary}
        caption={
          keptForChoice
            ? c.primaryKept
            : kept?.origin.startsWith('upload:')
              ? c.uploadedBy(kept.uploadedBy ?? '')
              : image.source && image.source !== 'manual' && image.source !== 'unknown'
                ? `${providerName(image.source)} · ${hostOf(image.url)}`
                : hostOf(image.url)
        }
        star={
          field ? (
            <IconButton
              glyph="star"
              pressed={primary}
              label={primary ? c.primaryUnset : c.primarySet}
              busy={choose.isPending && choose.variables?.field === field}
              onClick={() => choose.mutate({ field, url: primary ? null : image.url })}
            />
          ) : null
        }
        onForget={kept?.status === 'stored' && kept.assetId && !kept.origin.startsWith('upload:') ? () => forget.mutate(kept.assetId!) : undefined}
        forgetting={forget.isPending && forget.variables === kept?.assetId}
        onRemove={image.isManual && !keptForChoice ? () => ask({ path: `images/${image.id}`, label: image.url }) : undefined}
      />
    )
  }

  const shownKinds = kind === 'all' ? kinds : kinds.filter((k) => k === kind)

  return (
    <section id="artwork" aria-label={c.artwork} className="scroll-mt-28 lg:scroll-mt-16">
      {own.length ? (
        <div className="mb-3 flex flex-wrap items-center gap-2" role="group" aria-label={t.gallery.kinds}>
          <KindChip on={kind === 'all'} onClick={() => setKind('all')} label={a.all} n={own.length} />
          {kinds.map((k) => (
            <KindChip key={k} on={kind === k} onClick={() => setKind(k)} label={kindLabel(k)} n={own.filter((image) => image.coverType === k).length} />
          ))}
        </div>
      ) : null}
      <p className="flex items-start gap-1.5 text-xs leading-relaxed text-bone-faint">
        <Glyph name="star" className="mt-0.5 size-3.5 shrink-0 text-brass" />
        <span>{c.primaryHint}</span>
      </p>
      {forget.isError || choose.isError ? (
        <p role="alert" className="mt-3 text-xs text-vermillion">
          {(forget.error ?? choose.error)?.message || t.common.actionFailed}
        </p>
      ) : null}

      {own.length === 0 ? (
        <Panel className="mt-5">
          <Empty>{a.none}</Empty>
        </Panel>
      ) : null}

      {shownKinds.map((k) => {
        const images = own.filter((image) => image.coverType === k)
        const whole = kind === k || expanded.has(k) || images.length <= 12
        const shown = whole ? images : images.slice(0, 12)
        return (
          <section key={k} aria-label={kindLabel(k)} className="mt-6">
            <h3 className="mb-3 flex items-baseline gap-2 font-display text-lg font-medium text-bone">
              {kindLabel(k)}
              <Count>{images.length}</Count>
            </h3>
            {images.length ? (
              <>
                <ul className={cn('grid items-start gap-3', isWide(k) ? 'grid-cols-1 sm:grid-cols-2 lg:grid-cols-3' : 'grid-cols-2 sm:grid-cols-3 md:grid-cols-4 xl:grid-cols-6')}>
                  {shown.map((image) => card(image))}
                </ul>
                {whole ? null : (
                  <div className="mt-3 flex justify-center">
                    <Button size="sm" onClick={() => setExpanded((held) => new Set(held).add(k))}>
                      {t.gallery.showAll(images.length)}
                    </Button>
                  </div>
                )}
              </>
            ) : (
              <Empty className="px-0">{a.noneOfKind}</Empty>
            )}
          </section>
        )
      })}

      {seasonal ? (
        <p className="mt-8 flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-rule pt-5 text-sm text-bone-dim" data-season-images={seasonal}>
          <Glyph name="tv" className="size-4 shrink-0 text-bone-faint" />
          <span>{p.seasonImages(seasonal)}</span>
          <Link
            to={`/admin/catalogue/${work.id}?tab=seasons`}
            className="inline-flex min-h-9 items-center gap-1 text-vermillion underline-offset-4 hover:underline"
          >
            {p.openSeasons}
            <Glyph name="chevronRight" className="size-3" />
          </Link>
        </p>
      ) : null}

      <Panel className="mt-8">
        <PanelHead title={c.addImage} />
        <p className="px-5 pt-3 pb-1 text-xs leading-relaxed text-bone-faint">{c.artworkHint}</p>
        <div className="flex flex-wrap items-center gap-3 px-5 py-3">
          <Button size="sm" onClick={() => setAdding(true)}>
            <Glyph name="plus" className="size-4" />
            {c.addImage}
          </Button>
        </div>
        {storeOn ? (
          <UploadTheme item={work} onDone={done} />
        ) : media.data ? (
          <p className="border-t border-rule px-5 py-3 text-xs leading-relaxed text-bone-faint">{c.storeOff}</p>
        ) : null}
      </Panel>

      <ImagePicker
        open={adding}
        onClose={() => setAdding(false)}
        title={p.titleAdd}
        subtitle={work.title}
        work={work}
        shape={addKind === 'poster' ? 'poster' : 'wide'}
        uploadFields={{ coverType: addKind }}
        onChoose={(url) =>
          api
            .post<{ id: string }>(`/items/${work.id}/images`, {
              coverType: addKind,
              url,
              sortOrder: 0,
              language: addLanguage.trim() || undefined,
            })
            .then(done)
        }
        onUploaded={async () => done()}
        confirm={{ file: p.add, address: p.add }}
        hints={{ file: p.addHint, address: p.addHint }}
        extra={
          <div className="grid gap-4 sm:grid-cols-2">
            <FormField label={c.imageKind} htmlFor="image-kind">
              <Select id="image-kind" value={addKind} onChange={(event) => setAddKind(event.target.value)}>
                {ADDABLE.map((value) => (
                  <option key={value} value={value}>
                    {coverLabel(value)}
                  </option>
                ))}
              </Select>
            </FormField>
            <FormField label={a.language} htmlFor="image-language" hint={a.languageHint}>
              <Input id="image-language" value={addLanguage} placeholder="fr" maxLength={5} onChange={(event) => setAddLanguage(event.target.value)} />
            </FormField>
          </div>
        }
      />

      {dialog}
    </section>
  )
}

function KindChip({ on, onClick, label, n }: { on: boolean; onClick: () => void; label: string; n: number }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={cn(
        'inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-full border px-4 text-sm transition-colors duration-150',
        on ? 'border-vermillion-deep bg-vermillion/10 text-bone' : 'border-rule-bright text-bone-dim hover:text-bone',
      )}
    >
      {label}
      <Count>{n}</Count>
    </button>
  )
}

/* ── One picture ──────────────────────────────────────────────────────────── */

function ArtCard({
  image,
  kept,
  primary,
  caption,
  star,
  onForget,
  forgetting,
  onRemove,
}: {
  image: Image
  kept?: WorkMedium
  primary: boolean
  caption: string
  star: React.ReactNode
  onForget?: () => void
  forgetting: boolean
  onRemove?: () => void
}) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const wide = isWide(image.coverType)
  // An upload whose copy is gone keeps its `upload:` origin as its address:
  // nothing a browser can show, so the frame stays empty rather than asking
  // for it and being refused.
  const showable = /^(https?:)?\//.test(image.url)

  return (
    <li
      className={cn(
        'overflow-hidden rounded-panel border bg-ink-raised transition-colors duration-150',
        primary ? 'border-brass' : image.isManual ? 'border-brass-deep' : 'border-rule hover:border-rule-bright',
      )}
    >
      <div className={cn('relative bg-ink-high', wide ? (image.coverType === 'banner' ? 'aspect-[758/140]' : 'aspect-video') : 'aspect-2/3')}>
        {showable ? (
          <Picture
            url={image.url}
            role={isDrawn(image.coverType) ? 'logo' : wide ? 'still' : 'card'}
            alt=""
            className={cn('size-full', isDrawn(image.coverType) ? 'object-contain p-3' : 'object-cover')}
          />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="image" className="size-5 text-bone-faint" />
          </div>
        )}
        <div className="absolute top-2 left-2 flex flex-wrap gap-1">
          {primary ? (
            <Chip tone="manual" className="bg-ink/85">
              <Glyph name="star" className="size-3" />
              {c.primary}
            </Chip>
          ) : null}
          {image.isManual ? (
            <Chip tone="manual" className="bg-ink/85">
              {c.yours}
            </Chip>
          ) : null}
          <Kept medium={kept} />
        </div>
      </div>
      <div className="flex items-center justify-between gap-1 border-t border-rule py-0.5 pr-0.5 pl-3">
        <span className="min-w-0 truncate font-mono text-[0.6875rem] text-bone-faint" title={kept?.origin ?? image.url}>
          {caption}
        </span>
        <span className="flex shrink-0 items-center">
          {star}
          {onForget ? <IconButton glyph="cloud" label={c.forgetCopy} busy={forgetting} onClick={onForget} /> : null}
          {onRemove ? <IconButton glyph="trash" tone="danger" label={t.common.delete} onClick={onRemove} /> : null}
        </span>
      </div>
    </li>
  )
}

/** What is kept of a picture, in a word. */
function Kept({ medium }: { medium: WorkMedium | undefined }) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  if (!medium || medium.status === 'absent') return null
  if (medium.status === 'stored') {
    return (
      <Chip tone="provider" className="bg-ink/85">
        <Glyph name="database" className="size-3" />
        {c.kept}
      </Chip>
    )
  }
  return (
    <Chip tone={medium.status === 'failed' ? 'accent' : 'neutral'} className="bg-ink/85">
      <Glyph name={medium.status === 'failed' ? 'alert' : 'clock'} className="size-3" />
      {medium.status === 'failed' ? c.keptFailed : c.keptPending}
    </Chip>
  )
}

/* ── The theme, from a file ───────────────────────────────────────────────── */

/**
 * The work's theme music, put on it from a file. The file goes to the server
 * as it is; the server reads what it is, and locks the field on it.
 */
function UploadTheme({ item, onDone }: { item: MediaItem; onDone: () => void }) {
  const { t } = useI18n()
  const c = t.admin.editor.children
  const [open, setOpen] = useState(false)
  const [file, setFile] = useState<File | null>(null)
  const input = useRef<HTMLInputElement>(null)
  // The button the form replaces: focus comes back to it on cancel.
  const trigger = useRef<HTMLButtonElement>(null)

  const upload = useMutation({
    mutationFn: (form: FormData) => api.upload<Uploaded>(`/items/${item.id}/media`, form),
    onSuccess: () => {
      setFile(null)
      if (input.current) input.current.value = ''
      onDone()
    },
  })

  if (!open) {
    return (
      <div className="flex flex-wrap items-center gap-3 border-t border-rule px-5 py-3">
        <Button ref={trigger} size="sm" onClick={() => setOpen(true)}>
          <Glyph name="play" className="size-4" />
          {c.uploadTheme}
        </Button>
        {upload.isSuccess ? (
          <span role="status" className="text-xs text-moss">
            {c.uploaded}
          </span>
        ) : null}
      </div>
    )
  }

  return (
    <form
      className="border-t border-rule bg-ink/40 px-5 py-4"
      onSubmit={(event) => {
        event.preventDefault()
        if (!file) return
        const form = new FormData()
        form.set('file', file)
        form.set('kind', 'theme')
        upload.mutate(form)
      }}
    >
      <p className="mb-3 text-sm font-medium text-bone">{c.uploadTheme}</p>
      <FormField label={c.uploadFile} htmlFor="upload-theme-file" hint={c.themeHint}>
        <input
          ref={input}
          id="upload-theme-file"
          type="file"
          required
          autoFocus
          accept="audio/*"
          className="block w-full text-sm text-bone-dim file:mr-3 file:rounded-full file:border file:border-rule-bright file:bg-transparent file:px-3 file:py-1.5 file:text-sm file:text-bone"
          onChange={(event) => setFile(event.target.files?.[0] ?? null)}
        />
      </FormField>
      {upload.isError ? (
        <p role="alert" className="mt-3 text-xs text-vermillion">
          {upload.error.message || t.common.actionFailed}
        </p>
      ) : null}
      <div className="mt-3 flex flex-wrap gap-2">
        <Button type="submit" variant="primary" size="sm" disabled={!file || upload.isPending}>
          {upload.isPending ? <Glyph name="clock" className="size-4" /> : <Glyph name="download" className="size-4" />}
          {upload.isPending ? c.uploading : c.uploadTheme}
        </Button>
        <Button
          type="button"
          size="sm"
          onClick={() => {
            setOpen(false)
            requestAnimationFrame(() => trigger.current?.focus())
          }}
        >
          {t.common.cancel}
        </Button>
      </div>
    </form>
  )
}
