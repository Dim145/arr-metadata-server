<!-- What this changes, and why. Link the issue if there is one. -->

## Checklist

- [ ] `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] If the web interface changed: `cd frontend && npx tsc -b && npx vite build` pass, and the Playwright specs that cover it
- [ ] If the schema changed: a new migration in both `migrations/sqlite/` and `migrations/postgres/`, and none edited
- [ ] If the dependencies changed: `cargo deny check` passes
- [ ] A line under `[Unreleased]` in `CHANGELOG.md`, if someone running the server would notice
- [ ] Comments say why, there is no `TODO`, and it is all in English
