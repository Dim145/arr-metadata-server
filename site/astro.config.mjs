// The documentation site: README.md and docs/*.md, copied into the content
// collection by scripts/sync-docs.mjs before every build, and published on
// GitHub Pages by .github/workflows/docs.yml.

import starlight from '@astrojs/starlight'
import { defineConfig } from 'astro/config'

import { base, repository, site } from './site.mjs'

/** A page of the sidebar: its file under docs/, and a label shorter than its title. */
const page = (slug, label) => ({ slug, label })

export default defineConfig({
  site,
  base,
  integrations: [
    starlight({
      title: 'arr-metadata-server',
      description:
        'A self-hosted metadata server for Sonarr, Radarr, Plex and the TMDB, TheTVDB and AniList clients.',
      social: [{ icon: 'github', label: 'GitHub', href: repository }],
      sidebar: [
        {
          label: 'Getting started',
          items: [
            { label: 'Overview', link: '/' },
            page('features', 'What it does'),
            page('install', 'Installing and running'),
            page('configuration', 'Configuration'),
          ],
        },
        {
          label: 'Clients',
          items: [
            page('integration', 'Pointing your stack here'),
            page('sonarr-radarr', 'Sonarr and Radarr'),
            page('plex', 'Plex'),
            page('relays', 'The relays'),
          ],
        },
        {
          label: 'The catalogue',
          items: [
            page('sources', 'Where the data comes from'),
            page('languages', 'Languages'),
            page('notifications', 'Notifications'),
            page('exports', 'Exports'),
            page('editing', 'Editing'),
            page('data', 'Data'),
            page('interface', 'The interface'),
            page('media', 'Keeping the media'),
          ],
        },
        {
          label: 'Running it',
          items: [
            page('access', 'Accounts and access'),
            page('security', 'What is closed, and how'),
            page('reverse-proxy', 'Behind a reverse proxy'),
            page('caching', 'Caching'),
            page('instances', 'Several instances'),
          ],
        },
        {
          label: 'Reference',
          items: [page('settings', 'Settings'), page('api', 'The API')],
        },
      ],
    }),
  ],
})
