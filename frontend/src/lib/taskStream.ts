/**
 * The tasks, followed as they go.
 *
 * The server sends the whole board — every task, and the newest run — each
 * time it changes, over one stream (`/api/v1/admin/tasks/stream`), whichever
 * instance made the change. What it sends is written where the page reads
 * the tasks anyway (`['tasks']`), so a card is drawn the same whether it came
 * from the stream or from asking. When the stream cannot be opened — a proxy
 * that holds answers back, a browser without `EventSource` — the page asks
 * instead, every few seconds while something runs; and while the tab is
 * hidden it does neither: the stream is closed, and a hidden tab's timers
 * are left alone.
 */

import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'

import { api } from './api'
import type { Task, TaskBoard } from './types'

/** How the tasks are being followed: live, asked for, or not while the tab is hidden. */
export type Following = 'live' | 'asking' | 'paused'

/** How many errors in a row, with no event between them, before the page asks instead. */
const FAILURES = 3

export const STREAM = '/api/v1/admin/tasks/stream'

/** What tells the history it moved: the newest run, and which runs are in progress. */
export function runsOf(board: TaskBoard): string {
  const running = board.tasks
    .map((task) => task.running?.id)
    .filter(Boolean)
    .sort()
    .join(',')
  return `${board.newestRun ?? ''}|${running}`
}

/** Write a board where the page reads the tasks, and tell the history and the logs what moved. */
export function take(client: QueryClient, board: TaskBoard, runs: { current: string | undefined }): void {
  client.setQueryData<Task[]>(['tasks'], board.tasks)
  const now = runsOf(board)
  if (runs.current !== undefined && runs.current !== now) {
    void client.invalidateQueries({ queryKey: ['jobs'] })
  }
  runs.current = now
  // A run under way writes as it goes: its log, where it is open, is read again.
  for (const task of board.tasks) {
    if (task.running) void client.invalidateQueries({ queryKey: ['job', task.running.id] })
  }
}

function visibleNow(): boolean {
  return typeof document === 'undefined' || document.visibilityState !== 'hidden'
}

export function useTaskBoard() {
  const client = useQueryClient()
  const streams = typeof EventSource !== 'undefined'
  const [asking, setAsking] = useState(!streams)
  const [visible, setVisible] = useState(visibleNow)
  const [live, setLive] = useState(false)
  const runs = useRef<string | undefined>(undefined)

  useEffect(() => {
    const changed = () => {
      const now = visibleNow()
      setVisible(now)
      // Shown again: the stream is tried anew.
      if (now && streams) setAsking(false)
    }
    document.addEventListener('visibilitychange', changed)
    return () => document.removeEventListener('visibilitychange', changed)
  }, [streams])

  useEffect(() => {
    if (asking || !visible) return
    const source = new EventSource(STREAM)
    let failures = 0
    source.addEventListener('tasks', (event) => {
      failures = 0
      setLive(true)
      try {
        take(client, JSON.parse((event as MessageEvent<string>).data) as TaskBoard, runs)
      } catch {
        // A board that does not read is passed over: the next one will.
      }
    })
    source.onerror = () => {
      setLive(false)
      failures += 1
      // Closed for good, or failing again and again: ask instead.
      if (source.readyState === EventSource.CLOSED || failures >= FAILURES) {
        source.close()
        setAsking(true)
      }
    }
    return () => {
      source.close()
      setLive(false)
    }
  }, [asking, visible, client])

  const tasks = useQuery({
    queryKey: ['tasks'],
    queryFn: () => api.get<Task[]>('/tasks'),
    // Asked for while the stream is not there: every few seconds while
    // something runs, to follow it; otherwise the next due dates only move
    // by the minute. Not at all while the tab is hidden (the default).
    refetchInterval: (query) =>
      live ? false : query.state.data?.some((task) => task.running) ? 3_000 : 30_000,
  })

  const following: Following = !visible ? 'paused' : live ? 'live' : 'asking'
  return { tasks, following }
}
