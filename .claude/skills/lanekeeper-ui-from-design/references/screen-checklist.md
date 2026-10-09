# Per-screen checklist

Copy this list into `plans/08-*.md` once per screen.

- [ ] Reference file(s): `design/handoff/screens/<name>.dc.html`. The inventory is written.
- [ ] Route and URL parameters match the screen inventory, and every selection is reflected in the URL.
- [ ] Components are mapped to shadcn/ui, with no copied markup.
- [ ] Data comes only from generated hooks, with the SSE invalidation keys listed.
- [ ] States implemented: loading, empty, error, permission, offline, paused, truncated, 409, 423, 202, expired token.
- [ ] Keyboard map implemented and tested.
- [ ] Light and dark screenshots match the reference within the threshold.
- [ ] axe is clean, and the CSP test is clean.
- [ ] Bundle impact checked, and heavy parts lazy-loaded.
- [ ] Any deviations are recorded in `design/DEVIATIONS.md`.
