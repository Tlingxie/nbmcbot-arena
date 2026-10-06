import test from 'node:test';
import assert from 'node:assert/strict';
import { desiredFleets, reconcileFleets } from '../lib/fleets.mjs';

test('total player count is exact for one, odd and maximum rounds', () => {
  assert.deepEqual(desiredFleets(1).map(g => g.names), [['Mace001'], []]);
  assert.deepEqual(desiredFleets(3).map(g => g.names), [['Mace001', 'Mace002'], ['Spear001']]);
  assert.deepEqual(desiredFleets(100).map(g => [g.names.length, g.names.at(-1)]), [[50, 'Mace050'], [50, 'Spear050']]);
  for (const n of [0, -1, 101, 1.5, '10', null]) assert.throws(() => desiredFleets(n), /botCount/);
});

test('shrinking stops surplus accounts and does not create a zero-account process', async () => {
  const processes = new Map([
    ['mace-team', { running: true, usernames: ['Mace001', 'Mace002'] }],
    ['spear-team', { running: true, usernames: ['Spear001'] }],
  ]);
  const actions = [];
  const control = async (kind, action, command, env) => {
    assert.ok(['mace-team', 'spear-team'].includes(kind));
    actions.push({ kind, action, command });
    if (action === 'command' && command === 'quit') processes.get(kind).running = false;
    if (action === 'start') {
      assert.equal(processes.get(kind).running, false);
      assert.equal(env.NBMCBOT_DUEL_COUNT, '1');
      assert.equal(env.NBMCBOT_TELEMETRY_ADDR, '127.0.0.1:4211');
      processes.set(kind, { running: true, usernames: ['Mace001'] });
    }
    return { ...processes.get(kind) };
  };
  await reconcileFleets(1, { control });
  assert.deepEqual([...processes.values()], [{ running: true, usernames: ['Mace001'] }, { running: false, usernames: ['Spear001'] }]);
  assert.deepEqual(actions.filter(a => a.action === 'start').map(a => a.kind), ['mace-team']);
});

test('same roster reuses processes but stops the old battle before setup', async () => {
  const actions = [];
  await reconcileFleets(3, { control: async (kind, action, command) => {
    actions.push([kind, action, command]);
    return { running: true, usernames: kind === 'mace-team' ? ['Mace001', 'Mace002'] : ['Spear001'] };
  } });
  assert.equal(actions.some(([, action, command]) => action === 'start' || command === 'quit'), false);
  assert.deepEqual(actions.filter(([, action]) => action === 'command').map(([kind, , command]) => [kind, command]), [['mace-team', 'stop'], ['spear-team', 'stop']]);
});
