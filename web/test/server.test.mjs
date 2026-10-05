import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import dgram from 'node:dgram';
import { createViewer } from '../server.mjs';
import { Arena } from '../lib/arena.mjs';
import { ArenaState } from '../lib/state.mjs';

test('loopback HTTP validates origin/token and consumes real UDP without a game', async () => {
  const root = await mkdtemp(join(tmpdir(), 'nbmc-http-'));
  const viewer = await createViewer({ root, port: 0, udpPort: 0, poll: false });
  const socket = dgram.createSocket('udp4');
  try {
    const initial = await (await fetch(`${viewer.url}/api/state`)).json();
    assert.equal(initial.players.length, 0);
    const payload = Buffer.from(JSON.stringify({ event: 'telemetry_frame', version: 1, process_id: 123, sequence: 1, sampled_at_ms: Date.now(), players: [{ name: 'Mace001', source: 'self', position: [3, 64, 2], health: 20, alive: true, connected: true }] }));
    await new Promise((resolve, reject) => socket.send(payload, viewer.udpPort, '127.0.0.1', error => error ? reject(error) : resolve()));
    await new Promise(resolve => setTimeout(resolve, 40));
    const current = await (await fetch(`${viewer.url}/api/state`)).json();
    assert.equal(current.players[0].name, 'Mace001');
    const forbidden = await fetch(`${viewer.url}/api/spectate`, { method: 'POST', body: '{}' });
    assert.equal(forbidden.status, 403);
    const crossOrigin = await fetch(`${viewer.url}/api/spectate`, { method: 'POST', headers: { Origin: 'https://example.org', 'X-NBMC-Token': initial.token }, body: '{}' });
    assert.equal(crossOrigin.status, 403);
    const invalid = await fetch(`${viewer.url}/api/spectate`, { method: 'POST', headers: { Origin: viewer.url, 'X-NBMC-Token': initial.token }, body: JSON.stringify({ viewer: 'sdkl\nsay oops', target: 'Mace001' }) });
    assert.equal(invalid.status, 400);
    assert.equal((await fetch(`${viewer.url}/.runtime/recordings`)).status, 404);
    assert.equal((await fetch(`${viewer.url}/src/app.js`)).status, 200);
  } finally { socket.close(); await viewer.close(); await rm(root, { recursive: true, force: true }); }
});

test('spectating affects only selected online human; free view emits vanilla command', async () => {
  const state = new ArenaState();
  state.observe('sdkl', { position: [0, 64, 0], health: 20, alive: true });
  state.observe('Mace001', { position: [10, 100, 3], health: 20, alive: true });
  const arena = new Arena({ root: '/unused', state });
  arena.humans.add('sdkl');
  const commands = [];
  arena.command = (kind, command) => commands.push([kind, command]);
  await arena.spectate({ viewer: 'sdkl', target: 'Mace001' });
  await arena.spectate({ viewer: 'sdkl', target: null });
  assert.deepEqual(commands, [['server', 'gamemode spectator sdkl'], ['server', 'spectate Mace001 sdkl'], ['server', 'gamemode spectator sdkl'], ['server', 'execute as sdkl run spectate']]);
  await assert.rejects(arena.spectate({ viewer: 'Mace001', target: 'sdkl' }), /human/);
});
