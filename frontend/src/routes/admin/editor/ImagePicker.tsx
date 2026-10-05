/**
 * One picture, chosen from three places: the work's own, a file, an address.
 *
 * The same dialog wherever a picture is decided — a season's poster, an
 * episode's still, the work's own poster and background, a picture added to
 * the work — so that the gesture is learnt once. The work's own pictures
 * come first, at the shape wanted and filed by where they sit (this season,
 * the work, the other seasons), each saying where it is from and which is
 * in place now; a file can be dropped, pasted or picked; an address is
 * previewed before it is kept. One button closes it, and it says what it
 * does: choose and lock.
 *
 * The caller decides what a choice means — which override it writes, where
 * an upload is filed — and this component decides nothing but the picture.
 */

import { useMutation, useQuery } from '@tanstack/react-query'
import { useEffect, useId, useRef, useState, type ClipboardEvent, type DragEvent, type ReactNode } from 'react'

import { Artwork as Picture } from '../../../components/media'
import { Button, Dialog, FormField, Glyph, Input, Spinner } from '../../../components/ui'
import { api } from '../../../lib/api'
import { cn } from '../../../lib/cn'
import * as fmt from '../../../lib/format'
import { useI18n } from '../../../lib/i18n'
import { providerName } from '../../../lib/labels'
import type { Image, MediaItem, Uploaded, WorkMedia } from '../../../lib/types'

/** The shape a picture is wanted in: a poster's upright, or a still's wide. */
export type Shape = 'poster' | 'wide'

export type Source = 'own' | 'file' | 'address'

/** One of the work's pictures, offered, and the pile it comes from. */
export interface Candidate {
  image: Image
  group: 'season' | 'work' | 'seasons'
  /** The pile's own name where the group has several: "Season 3". */
  groupLabel?: string
}

const GROUPS: Candidate['group'][] = ['season', 'work', 'seasons']

const ACCEPT = 'image/jpeg,image/png,image/webp,image/gif,image/avif'

/** The schemes a typed address may have: a picture fetched over the web. */
const WEB = ['http:', 'https:'] as const
/** The scheme of a file chosen here, previewed from memory. */
const BLOB = ['blob:'] as const

/**
 * `given` parsed and written back as the browser reads it, when its scheme
 * is one of `schemes`; nothing otherwise. An `<img>` here is pointed at an
 * address only once it has been through this, so that what reaches it is an
 * address a picture loads from — never one that runs something, nor a line
 * that is no address at all.
 */
function addressOf(given: string, schemes: readonly string[]): string | undefined {
  try {
    const url = new URL(given.trim())
    return schemes.includes(url.protocol) ? url.href : undefined
  } catch {
    return undefined
  }
}

export function ImagePicker({
  open,
  onClose,
  title,
  subtitle,
  work,
  shape,
  candidates,
  current,
  uploadFields,
  onChoose,
  onUploaded,
  confirm,
  hints,
  extra,
}: {
  open: boolean
  onClose: () => void
  title: string
  /** What the picture is for, over the title: the work and its season. */
  subtitle?: string
  work: Pick<MediaItem, 'id'>
  shape: Shape
  /** The work's own pictures to offer; none offered when absent, as when adding to the pile itself. */
  candidates?: Candidate[]
  /** The address in place now, marked among the candidates. */
  current?: string
  /** The form fields that say where a file goes on the work; `false` where no file can be taken. */
  uploadFields?: Record<string, string> | false
  /** An address chosen or typed: the caller keeps it. Rejects with what to say. */
  onChoose: (url: string) => Promise<unknown>
  /** A file kept by the server: the caller claims it, where the server has not already. */
  onUploaded?: (uploaded: Uploaded) => Promise<unknown>
  /** The words on the button, per source; "choose and lock" by default. */
  confirm?: Partial<Record<Source, string>>
  /** The line under the panel, per source. */
  hints?: Partial<Record<Source, string>>
  /** Fields of the caller's above the sources: a kind and a language, for a picture added. */
  extra?: ReactNode
}) {
  const { t, locale } = useI18n()
  const p = t.admin.editor.picker
  const id = useId()
  const offered = candidates ?? []
  const canUpload = uploadFields !== false
  const sources: Source[] = [...(candidates ? (['own'] as const) : []), ...(canUpload ? (['file'] as const) : []), 'address']

  const [source, setSource] = useState<Source>(sources[0]!)
  const [group, setGroup] = useState<Candidate['group']>('season')
  const [picked, setPicked] = useState<string | undefined>(undefined)
  const [file, setFile] = useState<File | null>(null)
  const [over, setOver] = useState(false)
  const [address, setAddress] = useState('')
  const [probe, setProbe] = useState<{ state: 'idle' | 'ok' | 'failed'; width?: number; height?: number }>({ state: 'idle' })
  const fileInput = useRef<HTMLInputElement>(null)

  // Whether a file can be kept at all: what the store says, asked once the
  // dialog is open and only where a file is offered.
  const media = useQuery({
    queryKey: ['item', work.id, 'media'],
    queryFn: () => api.get<WorkMedia>(`/items/${work.id}/media`),
    enabled: open && canUpload,
  })
  const storeOn = media.data?.store ?? false

  const choose = useMutation({
    mutationFn: (url: string) => onChoose(url),
    onSuccess: onClose,
  })
  const upload = useMutation({
    mutationFn: async (chosen: File) => {
      const form = new FormData()
      form.set('file', chosen)
      form.set('kind', 'image')
      for (const [name, value] of Object.entries(uploadFields || {})) form.set(name, value)
      const kept = await api.upload<Uploaded>(`/items/${work.id}/media`, form)
      if (onUploaded) await onUploaded(kept)
      return kept
    },
    onSuccess: onClose,
  })

  // Afresh each time it opens: the first source there is, this season's
  // pile where there is one, and the picture in place already ticked.
  useEffect(() => {
    if (!open) return
    setSource(sources[0]!)
    setGroup(GROUPS.find((g) => offered.some((c) => c.group === g)) ?? 'work')
    setPicked(offered.find((c) => c.image.url === current)?.image.url)
    setFile(null)
    setAddress('')
    setProbe({ state: 'idle' })
    choose.reset()
    upload.reset()
    // The pictures and the sources are the caller's props, settled by the
    // time the dialog opens; opening is the moment to start over from them.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  // The file's own preview, and the memory it takes back when it goes.
  const [previewUrl, setPreviewUrl] = useState<string | undefined>(undefined)
  useEffect(() => {
    if (!file) {
      setPreviewUrl(undefined)
      return
    }
    const url = URL.createObjectURL(file)
    setPreviewUrl(url)
    return () => URL.revokeObjectURL(url)
  }, [file])
  const filePreview = previewUrl ? addressOf(previewUrl, BLOB) : undefined

  const takeFile = (taken: File | undefined) => {
    if (!taken || !taken.type.startsWith('image/')) return
    setFile(taken)
    setSource('file')
  }

  // A picture pasted anywhere in the dialog is a file to send; an address
  // pasted outside its own box moves to that box.
  const onPaste = (event: ClipboardEvent<HTMLDivElement>) => {
    const pasted = event.clipboardData.files?.[0]
    if (pasted && pasted.type.startsWith('image/') && canUpload) {
      event.preventDefault()
      takeFile(pasted)
      return
    }
    const text = event.clipboardData.getData('text').trim()
    if (source !== 'address' && addressOf(text, WEB)) {
      event.preventDefault()
      setAddress(text)
      setProbe({ state: 'idle' })
      setSource('address')
    }
  }
  const onDrop = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault()
    setOver(false)
    takeFile(event.dataTransfer.files?.[0])
  }

  const groups = GROUPS.filter((g) => offered.some((c) => c.group === g))
  const shownCandidates = offered.filter((c) => c.group === group)
  // The typed address as it will be kept and previewed: parsed, of the web.
  const webAddress = addressOf(address, WEB)
  const busy = choose.isPending || upload.isPending
  const error = choose.error ?? upload.error

  const submit = () => {
    if (source === 'own' && picked) choose.mutate(picked)
    else if (source === 'file' && file) upload.mutate(file)
    else if (source === 'address' && webAddress) choose.mutate(webAddress)
  }
  const ready = source === 'own' ? Boolean(picked) : source === 'file' ? Boolean(file) && storeOn : Boolean(webAddress)

  const onTabKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const at = sources.indexOf(source)
    const next =
      event.key === 'ArrowRight' ? (at + 1) % sources.length : event.key === 'ArrowLeft' ? (at - 1 + sources.length) % sources.length : undefined
    if (next === undefined) return
    event.preventDefault()
    setSource(sources[next]!)
    document.getElementById(`${id}-tab-${sources[next]}`)?.focus()
  }

  const tabLabel: Record<Source, string> = { own: p.tabs.own, file: p.tabs.file, address: p.tabs.address }
  const tabGlyph = { own: 'image', file: 'download', address: 'link' } as const
  const hint = hints?.[source] ?? (source === 'file' ? p.fileHint : source === 'address' ? p.addressHint : p.lockHint)
  const label = confirm?.[source] ?? (source === 'file' ? p.send : p.choose)
  const wide = shape === 'wide'

  return (
    <Dialog
      open={open}
      size="lg"
      onClose={onClose}
      title={
        <>
          {subtitle ? <span className="label mb-1 block font-normal">{subtitle}</span> : null}
          {title}
        </>
      }
      footer={
        <div className="flex w-full flex-wrap items-center justify-between gap-3">
          <span className="flex items-center gap-1.5 text-xs text-bone-faint">
            <Glyph name="lock" className="size-3 shrink-0 text-brass" />
            {hint}
          </span>
          <span className="flex items-center gap-2">
            <Button type="button" size="sm" onClick={onClose} disabled={busy}>
              {t.common.cancel}
            </Button>
            <Button type="button" variant="primary" size="sm" onClick={submit} disabled={!ready || busy}>
              {busy ? <Spinner className="size-4" /> : <Glyph name={source === 'file' ? 'download' : 'lock'} className="size-4" />}
              {busy && source === 'file' ? p.sending : label}
            </Button>
          </span>
        </div>
      }
    >
      <div onPaste={onPaste} className="-mx-5 -my-4">
        {extra ? <div className="border-b border-rule px-5 py-4">{extra}</div> : null}

        {/* Where the picture comes from: tabs, where there is more than one. */}
        {sources.length > 1 ? (
          <div
            role="tablist"
            aria-label={title}
            onKeyDown={onTabKey}
            // One line, scrolled sideways on a phone rather than wrapped into three.
            className="flex gap-1 overflow-x-auto border-b border-rule px-5 pt-2 [scrollbar-width:none]"
          >
            {sources.map((one) => {
              const selected = one === source
              return (
                <button
                  key={one}
                  id={`${id}-tab-${one}`}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  aria-controls={`${id}-panel`}
                  tabIndex={selected ? 0 : -1}
                  onClick={() => setSource(one)}
                  className={cn(
                    'relative inline-flex min-h-11 shrink-0 cursor-pointer items-center gap-2 px-3 text-sm whitespace-nowrap transition-colors duration-150 focus-visible:outline-offset-[-3px]',
                    selected ? 'text-bone' : 'text-bone-dim hover:text-bone',
                  )}
                >
                  <Glyph name={tabGlyph[one]} className="size-3.5" />
                  {tabLabel[one]}
                  {one === 'own' ? <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">{offered.length}</span> : null}
                  {selected ? <span aria-hidden className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-vermillion" /> : null}
                </button>
              )
            })}
          </div>
        ) : null}

        <div id={`${id}-panel`} role={sources.length > 1 ? 'tabpanel' : undefined} className="px-5 py-4">
          {source === 'own' ? (
            <>
              {groups.length > 1 ? (
                <div role="group" aria-label={p.tabs.own} className="mb-3 flex flex-wrap gap-1.5">
                  {groups.map((one) => {
                    const n = offered.filter((c) => c.group === one).length
                    const on = one === group
                    return (
                      <button
                        key={one}
                        type="button"
                        aria-pressed={on}
                        onClick={() => setGroup(one)}
                        className={cn(
                          'inline-flex min-h-9 cursor-pointer items-center gap-1.5 rounded-full border px-3 text-[0.8125rem] transition-colors duration-150',
                          on ? 'border-vermillion-deep bg-vermillion/10 text-bone' : 'border-rule-bright text-bone-dim hover:text-bone',
                        )}
                      >
                        {p.groups[one]}
                        <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">{n}</span>
                      </button>
                    )
                  })}
                </div>
              ) : null}
              {shownCandidates.length ? (
                <fieldset className="m-0 border-0 p-0">
                  <legend className="sr-only">{p.tabs.own}</legend>
                  <ul className={cn('grid gap-3', wide ? 'grid-cols-2 sm:grid-cols-3' : 'grid-cols-3 sm:grid-cols-4 md:grid-cols-5')}>
                    {shownCandidates.map(({ image, groupLabel }) => {
                      const isCurrent = image.url === current
                      const from = image.isManual ? p.yours : image.source && image.source !== 'manual' && image.source !== 'unknown' ? providerName(image.source) : undefined
                      const caption = [groupLabel, from, image.language].filter(Boolean).join(' · ')
                      return (
                        <li key={image.id}>
                          <label
                            className={cn(
                              'block cursor-pointer rounded-panel border-2 border-transparent p-1 transition-colors duration-150',
                              'has-checked:border-vermillion has-focus-visible:outline-2 has-focus-visible:outline-vermillion',
                            )}
                          >
                            <input
                              type="radio"
                              name={`${id}-candidate`}
                              value={image.url}
                              checked={picked === image.url}
                              onChange={() => setPicked(image.url)}
                              aria-label={p.choice([caption, isCurrent ? p.current : undefined].filter(Boolean).join(' · ') || image.url)}
                              className="sr-only"
                            />
                            <span className={cn('relative block overflow-hidden rounded-card border border-rule bg-ink-high', wide ? 'aspect-video' : 'aspect-2/3')}>
                              <Picture url={image.url} role={wide ? 'still' : 'card'} alt="" className="size-full object-cover" />
                              {picked === image.url ? (
                                <span aria-hidden className="absolute top-1.5 left-1.5 grid size-5 place-items-center rounded-full bg-vermillion text-ink">
                                  <Glyph name="check" className="size-3" />
                                </span>
                              ) : null}
                            </span>
                            <span className="mt-1.5 flex items-center justify-between gap-1 px-0.5 font-mono text-[0.625rem] text-bone-faint">
                              {isCurrent ? (
                                <span className="rounded-full border border-vermillion-deep px-1.5 text-vermillion">{p.current}</span>
                              ) : from ? (
                                <span className={cn('rounded-full border px-1.5', image.isManual ? 'border-brass-deep text-brass' : 'border-rule-bright')}>{from}</span>
                              ) : (
                                <span />
                              )}
                              <span className="truncate">{[groupLabel, image.language].filter(Boolean).join(' · ')}</span>
                            </span>
                          </label>
                        </li>
                      )
                    })}
                  </ul>
                </fieldset>
              ) : (
                <p className="py-6 text-center text-sm text-bone-faint">{p.none}</p>
              )}
            </>
          ) : source === 'file' ? (
            media.data && !storeOn ? (
              <p role="status" className="rounded-panel border border-brass-deep px-4 py-3 text-sm leading-relaxed text-bone-dim">
                {p.storeOff}
              </p>
            ) : (
              <div
                onDragOver={(event) => {
                  event.preventDefault()
                  setOver(true)
                }}
                onDragLeave={() => setOver(false)}
                onDrop={onDrop}
                className={cn(
                  'rounded-panel border border-dashed px-5 py-7 text-center transition-colors duration-150',
                  over ? 'border-vermillion bg-vermillion/5' : 'border-rule-bright',
                )}
              >
                {filePreview && file ? (
                  <div className="flex flex-col items-center gap-3">
                    <img
                      src={filePreview}
                      alt=""
                      className={cn('max-h-56 rounded-card border border-rule object-contain', wide ? 'aspect-video' : 'aspect-2/3')}
                    />
                    <span className="font-mono text-xs text-bone-dim">{p.chosenFile(file.name, fmt.bytes(file.size, locale))}</span>
                  </div>
                ) : (
                  <p className="text-sm text-bone-dim">
                    <span className="font-medium text-bone">{p.drop}</span>
                  </p>
                )}
                <div className="mt-3">
                  <Button type="button" size="sm" onClick={() => fileInput.current?.click()}>
                    <Glyph name="image" className="size-4" />
                    {p.browse}
                  </Button>
                  <input
                    ref={fileInput}
                    type="file"
                    accept={ACCEPT}
                    aria-label={p.browse}
                    className="sr-only"
                    onChange={(event) => takeFile(event.target.files?.[0])}
                  />
                </div>
                <p className="mt-3 font-mono text-[0.6875rem] text-bone-faint">{p.formats}</p>
              </div>
            )
          ) : (
            <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_11rem]">
              <FormField label={p.address} htmlFor={`${id}-address`} hint={p.previewHint}>
                <Input
                  id={`${id}-address`}
                  type="url"
                  placeholder="https://…"
                  autoFocus
                  value={address}
                  onChange={(event) => {
                    setAddress(event.target.value)
                    setProbe({ state: 'idle' })
                  }}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault()
                      submit()
                    }
                  }}
                />
              </FormField>
              <div>
                <span className="label mb-1.5 block">{p.preview}</span>
                <div className={cn('relative overflow-hidden rounded-card border border-rule bg-ink-high', wide ? 'aspect-video' : 'aspect-2/3 w-28')}>
                  {webAddress ? (
                    <img
                      key={webAddress}
                      src={webAddress}
                      alt=""
                      className="size-full object-cover"
                      onLoad={(event) => setProbe({ state: 'ok', width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })}
                      onError={() => setProbe({ state: 'failed' })}
                    />
                  ) : null}
                  {probe.state === 'ok' && probe.width ? (
                    <span className="absolute right-1.5 bottom-1.5 rounded-full bg-ink/85 px-1.5 font-mono text-[0.625rem] text-bone-dim tabular-nums">
                      {probe.width} × {probe.height}
                    </span>
                  ) : null}
                </div>
                {probe.state === 'failed' ? <p className="mt-1.5 text-xs text-vermillion">{p.previewFailed}</p> : null}
              </div>
            </div>
          )}

          {error ? (
            <p role="alert" className="mt-3 text-xs text-vermillion">
              {error.message || t.common.actionFailed}
            </p>
          ) : null}
        </div>
      </div>
    </Dialog>
  )
}
