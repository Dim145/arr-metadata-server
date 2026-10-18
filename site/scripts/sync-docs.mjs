// Copies README.md and docs/*.md into the site's content collection, so that
// every page is written once, for GitHub, and published from that same file.
//
// A page is written as GitHub reads it: its title as its first line, `# …`,
// and its links to the others as files, `plex.md#a-key-in-the-address`. Here
// the title becomes the page's frontmatter, a link to another page becomes
// the site's route to it, and a link to anything else in the repository — a
// script, a compose file, a directory — becomes its address on GitHub. A
// relative link to something that is not there stops the build, naming it.
//
// README.md is the home page; docs/<name>.md is <base>/<name>/. The directory
// written, src/content/docs/, is made anew each time and is not committed.

import { existsSync, statSync } from 'node:fs'
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { base, branch, repository } from '../site.mjs'

const site = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const root = path.resolve(site, '..')
const out = path.join(site, 'src', 'content', 'docs')

/** Every page published: its path in the repository, its file here, and its route. */
async function pages() {
  const docs = (await readdir(path.join(root, 'docs'), { withFileTypes: true }))
    .filter((entry) => entry.isFile() && entry.name.endsWith('.md'))
    .map((entry) => entry.name)
    .sort()

  return new Map([
    ['README.md', { file: 'index.md', route: `${base}/` }],
    ...docs.map((name) => {
      const slug = name.slice(0, -'.md'.length)
      return [`docs/${name}`, { file: name, route: `${base}/${slug}/` }]
    }),
  ])
}

/** The title on a page's first line, and what follows it. */
function split(source, from) {
  const [first, ...rest] = source.split('\n')
  const heading = /^# (.+)$/.exec(first.trim())
  if (!heading) {
    throw new Error(`${from}: the first line has to be the page's title, "# …"`)
  }
  // Inline code is the only markup a title here carries; the title is text.
  const title = heading[1].replaceAll('`', '').trim()
  return { title, body: rest.join('\n').replace(/^\s*\n/, '') }
}

/** Where a link written in `from` leads, as the site has it. */
function destination(target, from, published) {
  // An address, a mail link, an anchor on the same page: as written.
  if (/^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('#')) {
    return target
  }

  const cut = target.indexOf('#')
  const file = cut === -1 ? target : target.slice(0, cut)
  const hash = cut === -1 ? '' : target.slice(cut)

  const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(from), file))
  if (resolved.startsWith('../') || path.posix.isAbsolute(resolved)) {
    throw new Error(`${from}: ${target} leads out of the repository`)
  }

  const page = published.get(resolved)
  if (page) {
    return page.route + hash
  }

  const onDisk = path.join(root, resolved)
  if (!existsSync(onDisk)) {
    throw new Error(`${from}: ${target} names ${resolved}, which is not in the repository`)
  }
  const kind = statSync(onDisk).isDirectory() ? 'tree' : 'blob'
  return `${repository}/${kind}/${branch}/${resolved.replace(/\/$/, '')}${hash}`
}

/**
 * Every link's destination rewritten, outside code: a fenced block, or a span
 * between backticks, shows a link's text as it is written.
 */
function rewrite(body, from, published) {
  let fence = null
  return body
    .split('\n')
    .map((line) => {
      const opening = /^\s*(`{3,}|~{3,})/.exec(line)
      if (fence) {
        if (opening && opening[1][0] === fence[0] && opening[1].length >= fence.length) {
          fence = null
        }
        return line
      }
      if (opening) {
        fence = opening[1]
        return line
      }

      // Code spans set aside, the links rewritten, the spans put back.
      const spans = []
      const masked = line.replace(/(`+)[^`]*?\1/g, (span) => {
        spans.push(span)
        return `\u0000${spans.length - 1}\u0000`
      })
      const linked = masked.replace(
        /\]\(\s*([^)\s]+)(\s+"[^"]*")?\s*\)/g,
        (_, target, title = '') => `](${destination(target, from, published)}${title})`,
      )
      return linked.replace(/\u0000(\d+)\u0000/g, (_, index) => spans[Number(index)])
    })
    .join('\n')
}

async function main() {
  const published = await pages()

  await rm(out, { recursive: true, force: true })
  await mkdir(out, { recursive: true })

  for (const [from, page] of published) {
    const source = (await readFile(path.join(root, from), 'utf8')).replaceAll('\r\n', '\n')
    const { title, body } = split(source, from)
    const frontmatter = [
      '---',
      // JSON's quoted string is YAML's too.
      `title: ${JSON.stringify(title)}`,
      `editUrl: ${repository}/edit/${branch}/${from}`,
      '---',
      '',
    ].join('\n')
    await writeFile(path.join(out, page.file), frontmatter + rewrite(body, from, published))
  }

  console.log(`sync-docs: ${published.size} pages written to ${path.relative(site, out)}`)
}

main().catch((error) => {
  console.error(`sync-docs: ${error.message}`)
  process.exit(1)
})
