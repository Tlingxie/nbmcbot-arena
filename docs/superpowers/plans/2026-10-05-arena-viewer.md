# Arena Viewer Implementation Plan

**Goal:** Deliver and publish a usable local spectator and recorder for NBMCBot.

**Architecture:** Optional Rust UDP sampling feeds a loopback Node service.
The browser shows the explicitly captured original Minecraft window and a
2D position map. Recording chunks are written to disk and exported
through FFmpeg.

**Tech stack:** Rust/Azalea, Node.js >=22, browser Canvas/MediaRecorder,
FFmpeg, GitHub browser and connector.

**Spec:** ../specs/2026-10-05-arena-viewer-design.md

## Global constraints

- Preserve the running arena and existing user edits; no unrelated refactors.
- All runtime data, captures, dependencies and builds remain git-ignored.
- Use `sh scripts/cargo.sh` for Rust checks.
- Show original game imagery only; mark stale telemetry and absent capture.
- Publish only source and original assets; no Minecraft binaries/textures.

## Task 1: Independent telemetry (Rust worker)

Files: `crates/nbmcbot/src/telemetry.rs`, runtime control loops, tactics read-only
accessors, `scripts/arena-process.mjs` environment forwarding if needed.
Produces the exact UDP frame contract in the spec.

- [ ] Add failing tests for idle/dead/disconnected snapshots and output bounds.
- [ ] Implement opt-in loopback UDP sink and independent 100ms sampling.
- [ ] Run targeted tests, then leave root to run the complete workspace.

## Task 2: Local state and arena service (root)

Files: `web/package.json`, `web/server.mjs`, `web/lib/state.mjs`,
`web/lib/arena.mjs`, `web/test/*.test.mjs`, `scripts/viewer-process.mjs`.
Consumes telemetry and arena process records; produces the HTTP/SSE contract.

- [ ] Test freshness, malformed frames, source precedence and command allowlists.
- [ ] Implement bounded state, event tailing, static routing, origin/token checks.
- [ ] Add controlled round restart and explicit spectate actions.
- [ ] Start locally and verify real arena participants and streaming updates.

## Task 3: Live viewer UI (frontend worker)

Files owned: `web/index.html`, `web/src/app.js`, `web/src/style.css`,
`web/src/map.js`; consume HTTP/SSE state and Recorder from Task 4.
Recorder API: `new Recorder({getCanvas,getState,onChange})`,
`start({source:"minecraft",video?})`, `stop()`,
`compose()` after scene render, `captureGame()` returns a video element,
`releaseCapture()`. Recording module export `Recorder`.

- [ ] Build accessible responsive roster, team summary, viewport and status UI.
- [ ] Display the actual Minecraft window capture with a 2D telemetry map.
- [ ] Add player selection, real spectator following, and free camera exit.
- [ ] Connect recording controls, exports, round countdown and spectate action.
- [ ] Verify UI in an actual browser and address visible errors.

## Task 4: Recording client and export service (recording worker)

Files owned: `web/src/recording.js`, `web/lib/recordings.mjs`,
`web/test/recordings.test.mjs`. Server module exports
`createRecordingStore({directory,ffmpegPath?})` returning
`create(meta), append(id,sequence,buffer), finish(id), get(id), list(),
download(id), close()`. Methods may return Promises. `download()` returns
`{path,filename,mimeType}`; store errors carry `statusCode`.

- [ ] Test ordered finite uploads, invalid metadata, finalization and failures.
- [ ] Implement streaming uploads and bounded files under `.runtime/recordings`.
- [ ] Composite the live canvas/video with a HUD and send MediaRecorder chunks.
- [ ] Export playable MP4 through FFmpeg; preserve originals on export failure.

## Task 5: Integration and public delivery (root)

Files: README, docs/VIEWER.md, .gitignore, package lock, publication manifest.

- [ ] Run Node and Rust checks plus build/restart with telemetry enabled.
- [ ] Verify browser follow, live 5v5 data and a real playable exported clip.
- [ ] Inspect publishable files for local state and secrets; include license notices.
- [ ] Create `Tlingxie/nbmcbot-arena` public unless user supplies a different name.
- [ ] Publish the exact reviewed source and verify public repo and clone contents.
- [ ] Deliver viewer URL, recording file, repository link and material limitations.
