import { defineCollection } from 'astro:content'
import { docsLoader } from '@astrojs/starlight/loaders'
import { docsSchema } from '@astrojs/starlight/schema'

// The pages under src/content/docs/ are not written here: scripts/sync-docs.mjs
// makes them from README.md and docs/*.md before every build.
export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
}
