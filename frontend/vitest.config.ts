import { defineConfig } from 'vitest/config'

// The helpers under src/lib, which are plain functions: dates and formats,
// the feeds, the shelf of works opened lately, the dictionaries' merging.
// Run in Node rather than a page — what needs a browser is the end-to-end
// suite's (e2e/).
export default defineConfig({
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
    // West of Greenwich, where a day read as midnight UTC became the evening
    // before: the bugs these helpers were written against show up here, and
    // pass unseen in UTC.
    env: { TZ: 'America/Los_Angeles' },
  },
})
