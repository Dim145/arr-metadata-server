# End-to-end

These run against the **Rust binary**, not the Vite dev server: that binary is
what ships, serving the built assets from the same origin its API answers on,
with the real guards in front of both. A suite that passed against a dev proxy
would say nothing about whether a visitor can actually read the catalogue.

```bash
cargo build
cd frontend && npm run build          # the binary serves frontend/dist

AMS_PUBLIC_BROWSE=true \
AMS_BIND_ADDRESS=127.0.0.1:8479 \
AMS_DATABASE_URL='sqlite://data/e2e.db?mode=rwc' \
AMS_ADMIN_USERNAME=admin AMS_ADMIN_PASSWORD='choose-one' \
AMS_RATE_LIMIT_PER_MINUTE=6000 \
  ./target/debug/arr-metadata-server
```

The rate limit is raised because every request of the suite comes from one
address, two browsers at a time: at the default of 600 a minute, a second run
started straight after the first is refused partway, and its tests fail or skip
for a reason that has nothing to do with the interface.

Then, with a catalogue that has a few works in it:

```bash
cd frontend
npm run test:e2e                                   # the public catalogue only
AMS_E2E_USER=admin AMS_E2E_PASSWORD='choose-one' npm run test:e2e   # and the admin side
```

The administration tests skip themselves when no credential is given, rather
than falling back to a guessed one: a suite that signed in as `admin/admin`
would eventually run against somebody's real instance.

`AMS_E2E_URL` points the suite somewhere other than `http://127.0.0.1:8479`.

`accessibility.spec.ts` scans every screen against WCAG 2.1 AA with axe-core, at
both widths. An automated scan catches perhaps a third of what a person with a
screen reader would, but it catches it every time — contrast, names, roles,
list structure, scroll regions a keyboard cannot reach — which is the part that
regresses without anyone noticing.
