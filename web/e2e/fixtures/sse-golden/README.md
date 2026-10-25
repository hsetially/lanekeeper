# Golden SSE frames

Byte-exact frames of `GET /api/v1/events`, as the hub writes them. The live client (T3) replays them through its parser, and
the e2e harness (`web/e2e/harness/serve.mjs`) writes them to the browser unchanged.

Provenance: they are built to the format of `crates/hub-platform/src/events/sse.rs` (`retry_frame`, `heartbeat_frame` and
`event_frame`) and of the `DomainEvent` JSON in `api/openapi.yaml`. The hub's own tests do not dump bytes, so these were
written by hand from that code, not captured from a running hub. `frames.test.ts` checks every file against the contract's
event schema, so a frame that the contract would not allow fails.

| File | What it is |
|---|---|
| `first-frame.txt` | the first frame of a stream: `retry: 3000` |
| `heartbeat.txt` | a `: heartbeat` comment, every 15 s, with no id |
| `file-observed.txt` | a stored event: `id: 41` (note the space after the colon), `event:` equals the `type` |
| `findings-changed.txt` | another stored event, `id: 42` |
| `resync.txt` | a `resync` the server makes up: it has no `id:` line, so a client cannot resume from it |
| `final-retry.txt` | the last frame before the server closes a stream (shutdown or 30 minutes) |
