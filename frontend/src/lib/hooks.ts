/**
 * The two reads nearly every page makes, asked the same way everywhere.
 *
 * Sharing the query keys is the point: a work opened, then one of its seasons,
 * then an episode, is one request — the season and episode pages read what
 * the work's page already has.
 */

import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef } from 'react'
import { useLocation, useNavigationType } from 'react-router'

import { api, query } from './api'
import { useI18n } from './i18n'
import type { AuthOptions, CuratedLists, MediaItem, Me, Orders } from './types'

/**
 * The selections a reader may see: what the lists page shows, and what
 * decides whether the navigation offers it at all — a way to an empty page
 * is no way. Read once a visit; the page that composes a list announces it.
 */
export function useCuratedLists() {
  return useQuery({
    queryKey: ['lists'],
    queryFn: () => api.get<CuratedLists>('/lists'),
    staleTime: 5 * 60_000,
  })
}

/** Whether there is a selection to show, so far as is known: none while asking. */
export function useHasLists(): boolean {
  const lists = useCuratedLists()
  return (lists.data?.lists.length ?? 0) > 0
}

export function useWork(id: string) {
  const { lang } = useI18n()

  return useQuery({
    queryKey: ['work', id, lang],
    queryFn: () => api.get<MediaItem>(`/items/${id}${query({ language: lang })}`),
    enabled: Boolean(id),
  })
}

/** How else a series' episodes are numbered, where a provider keeps other orders. */
export function useOrders(id: string) {
  return useQuery({
    queryKey: ['orders', id],
    queryFn: () => api.get<Orders>(`/items/${id}/orders`),
    enabled: Boolean(id),
    staleTime: 10 * 60_000,
    retry: false,
  })
}

/**
 * The tab's title: what the page is, then the catalogue's name. Put back when
 * the page goes, so one that sets none does not wear the last one's.
 */
export function useTitle(...parts: (string | undefined)[]) {
  const title = [...parts.filter(Boolean), 'Cinémathèque'].join(' · ')

  useEffect(() => {
    document.title = title
    return () => {
      document.title = 'Cinémathèque'
    }
  }, [title])
}

/**
 * A page followed to is read from its top.
 *
 * The router leaves the window where it was, so "Next episode" at the foot of
 * one episode opened the next at its foot, and a screen reader stayed on the
 * link it had pressed with nothing said. On a new page the window goes to the
 * top and focus to the content, where the next Tab starts and a screen reader
 * reads on from — but not on Back, which the browser returns to where it
 * was, and not for a filter that changes only the address's query.
 */
export function useNavigationReset(mainId: string) {
  const { pathname } = useLocation()
  const type = useNavigationType()
  const seen = useRef(pathname)

  useEffect(() => {
    // Only a new page: the first one, and the same one asked for again with a
    // different query, keep their place.
    if (seen.current === pathname) return
    seen.current = pathname
    if (type === 'POP') return

    window.scrollTo(0, 0)
    document.getElementById(mainId)?.focus({ preventScroll: true })
  }, [pathname, type, mainId])
}

/** Who is looking — the same query the shell makes, so it is asked once. */
export function useMe() {
  return useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<Me>('/auth/me'),
    retry: false,
    staleTime: 5 * 60_000,
  })
}

/**
 * Whether the site is open, and whether one may sign up: readable before
 * anybody signs in, which is when it is needed.
 */
export function useAuthOptions() {
  return useQuery({
    queryKey: ['auth', 'options'],
    queryFn: () => api.get<AuthOptions>('/auth/options'),
    retry: false,
    staleTime: 5 * 60_000,
  })
}
