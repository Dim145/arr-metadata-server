import { useEffect, useState } from 'react'

/**
 * A value that settles before anything acts on it.
 *
 * For a free-text filter that is part of a query key: typing "interstellar"
 * otherwise issues twelve requests, of which eleven are already stale by the
 * time they land. Nothing is wrong with the results — each term is its own
 * cache entry — but the box this runs on is usually also transcoding.
 *
 * The delay is deliberately short. This is a filter over a list the operator is
 * watching, not a search they submit, so it has to keep feeling immediate.
 */
export function useSettled<T>(value: T, delay = 250): T {
  const [settled, setSettled] = useState(value)

  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), delay)
    return () => clearTimeout(timer)
  }, [value, delay])

  return settled
}
