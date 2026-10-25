import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it, vi } from 'vitest'

import { runsOf, take } from './taskStream'
import type { Task, TaskBoard } from './types'

function task(id: Task['id'], running?: string): Task {
  return {
    id,
    mode: 'scheduled',
    schedulable: false,
    cancelable: false,
    ...(running
      ? {
          running: {
            id: running,
            kind: id,
            target: null,
            status: 'running',
            startedAt: '2026-10-25T10:00:00Z',
            finishedAt: null,
            error: null,
            detail: '3 of 10: 3 refreshed, 0 failed',
            createdAt: '2026-10-25T10:00:00Z',
          },
          progress: { done: 3, total: 10 },
        }
      : {}),
  }
}

describe('the tasks followed', () => {
  it('tell the history it moved when a run starts or ends', () => {
    const idle: TaskBoard = { tasks: [task('refresh.sweep')], newestRun: 'a' }
    const running: TaskBoard = { tasks: [task('refresh.sweep', 'b')], newestRun: 'b' }
    expect(runsOf(idle)).toBe('a|')
    expect(runsOf(running)).toBe('b|b')

    const client = new QueryClient()
    const invalidate = vi.spyOn(client, 'invalidateQueries')
    const runs = { current: undefined as string | undefined }

    // The first board is what the page opened on: nothing to tell.
    take(client, idle, runs)
    expect(client.getQueryData(['tasks'])).toEqual(idle.tasks)
    expect(invalidate).not.toHaveBeenCalled()

    // A run started: the history is read again, and so is the run's log.
    take(client, running, runs)
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['jobs'] })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['job', 'b'] })

    // How far it got, said again: only its log.
    invalidate.mockClear()
    take(client, { ...running, tasks: [{ ...task('refresh.sweep', 'b'), progress: { done: 5, total: 10 } }] }, runs)
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: ['jobs'] })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['job', 'b'] })
  })
})
