import test from 'node:test';
import assert from 'node:assert/strict';
import { ArenaState, validName } from '../lib/state.mjs';

const player = (extra = {}) => ({ name: 'Mace001', source: 'self', position: [1, 64, 3], health: 20, alive: true, connected: true, ...extra });
const frame = (players, sequence = 1) => ({ event: 'telemetry_frame', version: 1, process_id: 42, sequence, sampled_at_ms: 1000, players });

test('validated positions age without inventing movement or health', () => {
  const state = new ArenaState();
  assert.equal(state.ingest(frame([player()]), 1000), true);
  assert.equal(state.players(1100)[0].stale, false);
  assert.equal(state.players(3101)[0].stale, true);
  assert.equal(state.players(6101)[0].connected, false);
  assert.deepEqual(state.players(6101)[0].position, [1, 64, 3]);
  assert.equal(state.players(6101)[0].team, 'red');
});
test('reject malformed, duplicate and delayed frames; merge independently numbered packets', () => {
  const state = new ArenaState();
  assert.equal(state.ingest(frame([player()], 2), 1000), true);
  assert.equal(state.ingest(frame([player({ name: 'Spear001' })], 3), 1001), true);
  assert.equal(state.ingest(frame([player({ health: 0 })], 3), 1002), false);
  assert.equal(state.ingest(frame([player({ health: 0 })], 1), 1002), false);
  assert.equal(state.ingest(frame([player({ name: 'bad\nsay evil' })], 3), 1003), false);
  assert.equal(state.ingest(frame([player({ position: [Infinity, 1, 2] })], 4), 1004), false);
  assert.equal(state.players(1004).length, 2);
  assert.equal(validName('@a'), false);
  assert.equal(state.ingest(frame([player()], 9), 5000), false);
});
test('partial human health observation never refreshes an old position', () => {
  const state = new ArenaState();
  state.observe('sdkl', { position: [0, 64, 0] }, 1000);
  state.observe('sdkl', { health: 20 }, 7000);
  assert.equal(state.players(7000)[0].stale, true);
  assert.equal(state.players(7000)[0].connected, true);
  state.observe('sdkl', { position: [1, 64, 0] }, 7100);
  assert.equal(state.players(7100)[0].stale, false);
});
test('fresh self samples win over observations, dead and disconnected states survive', () => {
  const state = new ArenaState();
  state.ingest(frame([player({ alive: false, health: 0 })]), 1000);
  assert.equal(state.players(1000)[0].position, null);
  state.observe('Mace001', { health: 20 }, 1100);
  assert.equal(state.players(1100)[0].health, 0);
  state.ingest(frame([player({ connected: false, alive: null, health: null, position: null })], 2), 1200);
  assert.equal(state.players(1200)[0].position, null);
  assert.equal(state.players(1200)[0].connected, false);
});
