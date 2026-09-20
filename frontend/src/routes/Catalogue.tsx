import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Link } from 'react-router'

import { api, query } from '../lib/api'
import type { MediaItem, MediaKind } from '../lib/types'
import {
  Alert,
  Button,
  Display,
  Empty,
  Input,
  Label,
  Lock,
  Mono,
  Panel,
  Select,
  Spinner,
  Tag,
  Textarea,
} from '../components/ui'

interface ListResponse {
  items: MediaItem[]
  total: number
}

export function Catalogue() {
  const [term, setTerm] = useState('')
  const [kind, setKind] = useState<'' | MediaKind>('')
  const [manualOnly, setManualOnly] = useState(false)
  const [composing, setComposing] = useState(false)

  const list = useQuery({
    queryKey: ['items', term, kind, manualOnly],
    queryFn: () =>
      api.get<ListResponse>(
        `/items${query({ term, kind, manualOnly, limit: 60, includeDisabled: true })}`,
      ),
  })

  return (
    <div className="mx-auto max-w-5xl">
      <header className="reveal mb-8 flex flex-wrap items-end justify-between gap-4">
        <div>
          <Label>Catalogue</Label>
          <Display className="mt-2">Everything this server knows</Display>
        </div>
        <Button variant="primary" onClick={() => setComposing((open) => !open)}>
          {composing ? 'Cancel' : 'New entry'}
        </Button>
      </header>

      {composing && <Composer onDone={() => setComposing(false)} />}

      <div
        className="reveal mb-6 flex flex-wrap items-center gap-3"
        style={{ animationDelay: '60ms' }}
      >
        <Input
          placeholder="Search titles…"
          value={term}
          onChange={(event) => setTerm(event.target.value)}
          className="max-w-xs"
        />
        <Select value={kind} onChange={(event) => setKind(event.target.value as '' | MediaKind)}>
          <option value="">all kinds</option>
          <option value="series">series</option>
          <option value="movie">movies</option>
        </Select>
        <label className="flex cursor-pointer items-center gap-2 font-mono text-[11px] tracking-[0.1em] text-faint uppercase select-none hover:text-dim">
          <input
            type="checkbox"
            checked={manualOnly}
            onChange={(event) => setManualOnly(event.target.checked)}
            className="accent-phos"
          />
          manual only
        </label>
        {list.isFetching && <Spinner />}
        {list.data && (
          <Mono className="ml-auto text-faint">
            {list.data.items.length} / {list.data.total}
          </Mono>
        )}
      </div>

      <Panel className="reveal overflow-hidden" style={{ animationDelay: '100ms' }}>
        {list.isPending ? (
          <div className="px-5 py-16 text-center">
            <Spinner />
          </div>
        ) : list.isError ? (
          <div className="p-5">
            <Alert>Could not load the catalogue.</Alert>
          </div>
        ) : list.data.items.length === 0 ? (
          <Empty
            title="Nothing here yet"
            hint={
              term
                ? 'No stored entry matches that search. Clients searching through this server will still reach the providers.'
                : 'Entries appear as clients request them, or create one by hand.'
            }
          />
        ) : (
          <ul className="divide-y divide-line">
            {list.data.items.map((item) => (
              <Row key={item.id} item={item} />
            ))}
          </ul>
        )}
      </Panel>
    </div>
  )
}

function Row({ item }: { item: MediaItem }) {
  const ids = item.externalIds

  return (
    <li>
      <Link
        to={`/catalogue/${item.id}`}
        className="edge flex items-center gap-4 px-5 py-3.5 transition-colors hover:bg-raised"
      >
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-[15px] text-paper">{item.title}</span>
            {item.year && <Mono className="shrink-0 text-faint">{item.year}</Mono>}
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
            {ids.tvdb && <Mono className="text-[11px] text-faint">tvdb:{ids.tvdb}</Mono>}
            {ids.tmdb && <Mono className="text-[11px] text-faint">tmdb:{ids.tmdb}</Mono>}
            {ids.imdb && <Mono className="text-[11px] text-faint">{ids.imdb}</Mono>}
          </div>
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          {!item.isEnabled && <Tag tone="bad">disabled</Tag>}
          {item.isManual && <Tag tone="manual">manual</Tag>}
          {(item.lockedFields?.length ?? 0) > 0 && (
            <Tag tone="manual">
              <Lock className="h-[11px] w-[11px]" />
              {item.lockedFields!.length}
            </Tag>
          )}
          <Tag tone="neutral">{item.kind}</Tag>
        </div>
      </Link>
    </li>
  )
}

/** Create an entry that exists on no provider at all. */
function Composer({ onDone }: { onDone: () => void }) {
  const queryClient = useQueryClient()
  const [kind, setKind] = useState<MediaKind>('series')
  const [title, setTitle] = useState('')
  const [year, setYear] = useState('')
  const [overview, setOverview] = useState('')
  const [tvdb, setTvdb] = useState('')
  const [tmdb, setTmdb] = useState('')

  const create = useMutation({
    mutationFn: () =>
      api.post<MediaItem>('/items', {
        kind,
        title,
        year: year ? Number(year) : undefined,
        overview: overview || undefined,
        status: kind === 'series' ? 'continuing' : 'tba',
        externalIds: {
          ...(tvdb ? { tvdb: Number(tvdb) } : {}),
          ...(tmdb ? { tmdb: Number(tmdb) } : {}),
        },
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['items'] })
      void queryClient.invalidateQueries({ queryKey: ['stats'] })
      onDone()
    },
  })

  return (
    <Panel className="reveal mb-6">
      <form
        className="grid gap-4 p-5 sm:grid-cols-2"
        onSubmit={(event) => {
          event.preventDefault()
          create.mutate()
        }}
      >
        <div className="flex flex-col gap-1.5">
          <Label>Kind</Label>
          <Select value={kind} onChange={(event) => setKind(event.target.value as MediaKind)}>
            <option value="series">series</option>
            <option value="movie">movie</option>
          </Select>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label>Year</Label>
          <Input
            inputMode="numeric"
            value={year}
            onChange={(event) => setYear(event.target.value.replace(/\D/g, '').slice(0, 4))}
            placeholder="2026"
          />
        </div>

        <div className="flex flex-col gap-1.5 sm:col-span-2">
          <Label>Title</Label>
          <Input required autoFocus value={title} onChange={(e) => setTitle(e.target.value)} />
        </div>

        <div className="flex flex-col gap-1.5 sm:col-span-2">
          <Label>Overview</Label>
          <Textarea rows={3} value={overview} onChange={(e) => setOverview(e.target.value)} />
        </div>

        <div className="flex flex-col gap-1.5">
          <Label>TVDB id (optional)</Label>
          <Input
            inputMode="numeric"
            value={tvdb}
            onChange={(e) => setTvdb(e.target.value.replace(/\D/g, ''))}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <Label>TMDB id (optional)</Label>
          <Input
            inputMode="numeric"
            value={tmdb}
            onChange={(e) => setTmdb(e.target.value.replace(/\D/g, ''))}
          />
        </div>

        <p className="text-[12px] leading-relaxed text-faint sm:col-span-2">
          Supplying an external id makes the entry refreshable: a provider will fill in what you
          leave blank, and never touch what you fill in.
        </p>

        {create.isError && (
          <div className="sm:col-span-2">
            <Alert>{create.error.message}</Alert>
          </div>
        )}

        <div className="flex gap-2 sm:col-span-2">
          <Button type="submit" variant="primary" disabled={create.isPending || !title.trim()}>
            {create.isPending ? <Spinner className="border-void/40 border-t-void" /> : 'Create'}
          </Button>
          <Button type="button" onClick={onDone}>
            Cancel
          </Button>
        </div>
      </form>
    </Panel>
  )
}
