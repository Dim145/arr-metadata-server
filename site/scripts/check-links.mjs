// Checks every link the built site makes to itself: the page it names has to
// exist, and so does the heading its anchor names. Run after `astro build`;
// a link that leads nowhere stops the build, naming the page it is on.
//
// The pages' anchors are the headings' slugs, made as GitHub makes them, so
// a link between two files of docs/ that works here works on GitHub too.

import { existsSync } from 'node:fs'
import { readFile, readdir } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { base } from '../site.mjs'

const site = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const dist = path.join(site, 'dist')

/** Every HTML file under `dir`. */
async function pages(dir) {
  const found = []
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) {
      found.push(...(await pages(full)))
    } else if (entry.name.endsWith('.html')) {
      found.push(full)
    }
  }
  return found
}

// `&amp;` last: decoded first, `&amp;lt;` would become `<` rather than `&lt;`.
const decode = (text) =>
  text
    .replaceAll('&quot;', '"')
    .replaceAll('&#39;', "'")
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>')
    .replaceAll('&amp;', '&')

/** The file a path of the site is served from, or nothing for one outside it. */
function fileOf(pathname) {
  if (!pathname.startsWith(`${base}/`) && pathname !== base) {
    return null
  }
  const rest = decodeURIComponent(pathname.slice(base.length)).replace(/^\//, '')
  if (rest === '' || rest.endsWith('/')) {
    return path.join(dist, rest, 'index.html')
  }
  if (path.extname(rest)) {
    return path.join(dist, rest)
  }
  return path.join(dist, rest, 'index.html')
}

async function main() {
  if (!existsSync(dist)) {
    throw new Error('no dist/ to check: run astro build first')
  }

  const files = await pages(dist)
  const ids = new Map()
  const links = []

  for (const file of files) {
    const html = await readFile(file, 'utf8')
    ids.set(file, new Set([...html.matchAll(/\sid="([^"]*)"/g)].map((m) => decode(m[1]))))
    for (const match of html.matchAll(/\shref="([^"]*)"/g)) {
      links.push({ file, href: decode(match[1]) })
    }
  }

  const broken = []
  for (const { file, href } of links) {
    if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('//')) {
      continue
    }

    const cut = href.indexOf('#')
    const pathname = (cut === -1 ? href : href.slice(0, cut)).split('?')[0]
    const anchor = cut === -1 ? '' : decodeURIComponent(href.slice(cut + 1))

    const target = pathname === '' ? file : fileOf(pathname)
    if (target === null) {
      broken.push(`${path.relative(dist, file)}: ${href} is outside ${base}/`)
      continue
    }
    if (!existsSync(target)) {
      broken.push(`${path.relative(dist, file)}: ${href} leads to no page`)
      continue
    }
    if (anchor && target.endsWith('.html') && !ids.get(target)?.has(anchor)) {
      broken.push(`${path.relative(dist, file)}: ${href} names no heading of that page`)
    }
  }

  if (broken.length) {
    console.error(`check-links: ${broken.length} broken:\n  ${broken.join('\n  ')}`)
    process.exit(1)
  }
  console.log(`check-links: ${links.length} links on ${files.length} pages, none broken`)
}

main().catch((error) => {
  console.error(`check-links: ${error.message}`)
  process.exit(1)
})
