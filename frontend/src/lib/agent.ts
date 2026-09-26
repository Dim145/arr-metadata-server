/**
 * A device as a person recognises it, from the User-Agent its browser sent:
 * "Firefox · macOS", not a line of version numbers.
 *
 * A guess, and said as one: an agent that names nothing known reads as
 * nothing, and the page says "unknown device".
 */
export function describeAgent(agent: string | undefined): string | undefined {
  if (!agent) return undefined

  const browser =
    /Edg\//.test(agent) ? 'Edge'
    : /OPR\//.test(agent) ? 'Opera'
    : /Firefox\//.test(agent) ? 'Firefox'
    : /Chrome\//.test(agent) ? 'Chrome'
    : /Safari\//.test(agent) ? 'Safari'
    : /curl\//i.test(agent) ? 'curl'
    : undefined

  const system =
    /iPhone|iPad/.test(agent) ? 'iOS'
    : /Android/.test(agent) ? 'Android'
    : /Mac OS X|Macintosh/.test(agent) ? 'macOS'
    : /Windows/.test(agent) ? 'Windows'
    : /Linux/.test(agent) ? 'Linux'
    : undefined

  const parts = [browser, system].filter(Boolean)
  return parts.length ? parts.join(' · ') : undefined
}
