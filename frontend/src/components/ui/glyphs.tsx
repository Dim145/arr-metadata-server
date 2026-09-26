/**
 * The icon set, drawn as strokes so it inherits the text's colour.
 */

import { cn } from '../../lib/cn'

/**
 * One stroke weight, one corner treatment, drawn here rather than pulled from a
 * package: the set is small enough that a dependency would cost more than it
 * saves, and every glyph inherits the palette by using `currentColor`.
 */
const PATHS = {
  search: <path d="M11 11 15 15M7 12.5a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11Z" />,
  sun: (
    <>
      <circle cx="8" cy="8" r="3" />
      <path d="M8 1.5V3M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1.1 1.1M11.5 11.5l1.1 1.1M3.4 12.6l1.1-1.1M11.5 4.5l1.1-1.1" />
    </>
  ),
  moon: <path d="M13.5 9.6A5.6 5.6 0 0 1 6.4 2.5a5.6 5.6 0 1 0 7.1 7.1Z" />,
  close: <path d="M4 4 12 12M12 4 4 12" />,
  menu: <path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" />,
  chevronRight: <path d="M6 3.5 10.5 8 6 12.5" />,
  chevronLeft: <path d="M10 3.5 5.5 8 10 12.5" />,
  chevronDown: <path d="M3.5 6 8 10.5 12.5 6" />,
  chevronUp: <path d="M3.5 10 8 5.5 12.5 10" />,
  lock: (
    <>
      <path d="M5 7V5a3 3 0 1 1 6 0v2" />
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.5" />
    </>
  ),
  cloud: <path d="M4.5 12.5a3 3 0 0 1 .3-6 4 4 0 0 1 7.6 1.2 2.4 2.4 0 0 1-.4 4.8Z" />,
  alert: <path d="M8 5v4M8 11.5v.01M8 2 1.5 13.5h13Z" />,
  reel: (
    <>
      <circle cx="8" cy="8" r="6" />
      <circle cx="8" cy="5.2" r="1.3" />
      <circle cx="10.4" cy="9.4" r="1.3" />
      <circle cx="5.6" cy="9.4" r="1.3" />
    </>
  ),
  film: (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M5.5 3v10M10.5 3v10" />
    </>
  ),
  tv: (
    <>
      <rect x="2" y="4.5" width="12" height="8" rx="1.5" />
      <path d="M5.5 2 8 4.5 10.5 2" />
    </>
  ),
  star: <path d="m8 2 1.8 3.9 4.2.5-3.1 2.9.8 4.2L8 11.4 4.3 13.5l.8-4.2L2 6.4l4.2-.5Z" />,
  settings: (
    <>
      <circle cx="8" cy="8" r="2.2" />
      <path d="M8 1.5v1.8M8 12.7v1.8M14.5 8h-1.8M3.3 8H1.5M12.6 3.4l-1.3 1.3M4.7 11.3l-1.3 1.3M12.6 12.6l-1.3-1.3M4.7 4.7 3.4 3.4" />
    </>
  ),
  user: (
    <>
      <circle cx="8" cy="5.5" r="2.8" />
      <path d="M2.8 14a5.2 5.2 0 0 1 10.4 0" />
    </>
  ),
  globe: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M2 8h12M8 2a9 9 0 0 1 0 12M8 2a9 9 0 0 0 0 12" />
    </>
  ),
  arrowLeft: <path d="M13 8H3M6.5 4.5 3 8l3.5 3.5" />,
  external: <path d="M9 3h4v4M13 3 7 9M11.5 9.5v3h-9v-9h3" />,

  /* The administration side: one glyph per section of the building, then the
     verbs an operator performs once inside it. */
  gauge: (
    <>
      <path d="M2.5 12a5.5 5.5 0 1 1 11 0" />
      <path d="M8 12 10.9 8.1" />
    </>
  ),
  list: <path d="M5.5 4h8M5.5 8h8M5.5 12h8M2.6 4h.01M2.6 8h.01M2.6 12h.01" />,
  key: (
    <>
      <circle cx="10.5" cy="5.5" r="3" />
      <path d="M8.4 7.6 2.5 13.5M4.6 11.4 6.4 13.2" />
    </>
  ),
  clock: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.5V8l2.6 1.8" />
    </>
  ),
  journal: (
    <>
      <rect x="3.5" y="2" width="9" height="12" rx="1.5" />
      <path d="M6 5.5h4M6 8h4M6 10.5h2.5" />
    </>
  ),
  signOut: <path d="M6.5 14h-3a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1h3M10.5 11.5 14 8l-3.5-3.5M14 8H6" />,
  plus: <path d="M8 3v10M3 8h10" />,
  trash: (
    <>
      <path d="M2.5 4.5h11" />
      <path d="M6.3 4.5V3.2a1 1 0 0 1 1-1h1.4a1 1 0 0 1 1 1v1.3" />
      <path d="M4.1 4.5l.6 8.2a1 1 0 0 0 1 .9h4.6a1 1 0 0 0 1-.9l.6-8.2" />
    </>
  ),
  refresh: (
    <>
      <path d="M13.5 8a5.5 5.5 0 1 1-5.5-5.5c1.5 0 2.9.6 4 1.6l1.5 1.4" />
      <path d="M13.5 2v3.5H10" />
    </>
  ),
  copy: (
    <>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
      <path d="M10.5 5.5v-2a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2" />
    </>
  ),
  check: <path d="M3 8.5 6.4 12 13 4.4" />,
  unlock: (
    <>
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.5" />
      <path d="M5 7V5a3 3 0 0 1 5.9-.8" />
    </>
  ),
  power: <path d="M8 2.5v5.6M11.9 4.7a5.2 5.2 0 1 1-7.8 0" />,
  download: <path d="M8 2.5v8M4.6 7.4 8 10.8l3.4-3.4M2.5 13.5h11" />,
  database: (
    <>
      <ellipse cx="8" cy="3.8" rx="5.5" ry="2" />
      <path d="M2.5 3.8v8.4c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2V3.8" />
      <path d="M13.5 8c0 1.1-2.5 2-5.5 2s-5.5-.9-5.5-2" />
    </>
  ),
  /** A value that follows another scope's, rather than being set here. */
  link: (
    <>
      <path d="M6.4 9.6 9.6 6.4" />
      <path d="M7.2 4.7 8.6 3.3a2.9 2.9 0 0 1 4.1 4.1l-1.4 1.4" />
      <path d="M8.8 11.3 7.4 12.7a2.9 2.9 0 0 1-4.1-4.1l1.4-1.4" />
    </>
  ),
  /** Searching a provider in order to take something from it. */
  discover: (
    <>
      <path d="M11 11 15 15" />
      <circle cx="7" cy="7" r="5.5" />
      <path d="M7 4.6v4.8M4.6 7h4.8" />
    </>
  ),
  play: <path d="M5 3.3v9.4l7.6-4.7Z" />,
  pause: <path d="M5.5 3.5v9M10.5 3.5v9" />,
  calendar: (
    <>
      <path d="M2.5 4.5h11v8.8h-11Z" />
      <path d="M2.5 7.2h11M5.5 2.7v3M10.5 2.7v3" />
    </>
  ),
  rss: (
    <>
      <path d="M3.2 3.4a9.4 9.4 0 0 1 9.4 9.4" />
      <path d="M3.2 7.4a5.4 5.4 0 0 1 5.4 5.4" />
      <path d="M3.7 12.3h.01" />
    </>
  ),
  image: (
    <>
      <path d="M2.5 3.5h11v9h-11Z" />
      <path d="m2.5 10.8 3.1-3.1 2.6 2.6 1.9-1.9 3.4 3.4" />
      <path d="M10.6 6.3h.01" />
    </>
  ),
  sliders: <path d="M2.5 4.5h6.5M12 4.5h1.5M2.5 11.5h1.5M7 11.5h6.5M10.5 3v3M5.5 10v3" />,
  sort: <path d="M5 2.8v10.4M2.9 11.1 5 13.2l2.1-2.1M11 13.2V2.8M8.9 4.9 11 2.8l2.1 2.1" />,
  pencil: (
    <>
      <path d="M2.5 13.5 3 10.8l7.4-7.4a1.7 1.7 0 0 1 2.4 2.4l-7.4 7.4Z" />
      <path d="M9.6 4.2 12 6.6" />
    </>
  ),
} as const

export type GlyphName = keyof typeof PATHS

export function Glyph({
  name,
  className,
  title,
}: {
  name: GlyphName
  className?: string
  title?: string
}) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : true}
      role={title ? 'img' : undefined}
      className={cn('size-4 shrink-0', className)}
    >
      {title ? <title>{title}</title> : null}
      {PATHS[name]}
    </svg>
  )
}
