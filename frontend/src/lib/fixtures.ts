/**
 * Works, episodes and pictures as the server sends them, for the unit tests:
 * the fields every one carries, and whatever a test sets on top.
 */

import type { Episode, Image, MediaItem } from './types'

export function work(over: Partial<MediaItem> = {}): MediaItem {
  return {
    id: 'w1',
    kind: 'series',
    slug: 'the-wire',
    title: 'The Wire',
    externalIds: {},
    isManual: false,
    isEnabled: true,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    genres: [],
    keywords: [],
    isAdult: false,
    ...over,
  }
}

export function episode(seasonNumber: number, episodeNumber: number, over: Partial<Episode> = {}): Episode {
  return {
    id: `e${seasonNumber}x${episodeNumber}`,
    seasonNumber,
    episodeNumber,
    title: `Episode ${episodeNumber}`,
    isManual: false,
    ...over,
  }
}

export function image(id: string, over: Partial<Image> = {}): Image {
  return { id, coverType: 'poster', url: `https://example.org/${id}.jpg`, sortOrder: 0, isManual: false, ...over }
}
