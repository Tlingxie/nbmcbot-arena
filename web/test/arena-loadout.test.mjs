import test from 'node:test';
import assert from 'node:assert/strict';
import { duelPlan, loadoutCommands, validateGroupNames } from '../../scripts/arena-loadout.mjs';

test('total splits handle one, odd and one hundred bots', () => {
  for (const [total, counts] of [[1,[1,0]],[7,[4,3]],[100,[50,50]]]) {
    assert.deepEqual(duelPlan({NBMCBOT_DUEL_TOTAL:String(total)}).groups.map(group=>group.count),counts);
  }
  assert.deepEqual(duelPlan({}).groups.map(group=>group.count),[5,5]);
  for (const value of ['0','101','1.5','bad']) assert.throws(()=>duelPlan({NBMCBOT_DUEL_TOTAL:value}));
});

test('player mode requires safe exact target and one friendly team', () => {
  for (const value of ['', '@a', 'Player run kill', 'Player\nstop', '=Player']) {
    assert.throws(()=>duelPlan({NBMCBOT_DUEL_MODE:'mace-vs-player',NBMCBOT_DUEL_PLAYER:value}));
  }
  const plan = duelPlan({NBMCBOT_DUEL_MODE:'mace-vs-player',NBMCBOT_DUEL_PLAYER:'sdkl',NBMCBOT_DUEL_TOTAL:'7'});
  assert.ok(plan.groups.every(group=>group.style==='mace'&&group.enemy==='=sdkl'));
  assert.equal(new Set(plan.groups.map(group=>group.team)).size,1);
  const commands = loadoutCommands(plan, plan.groups.map(group=>({...group,names:Array.from({length:group.count},(_,i)=>`${group.prefix}${String(i+1).padStart(3,'0')}`)})));
  assert.ok(commands.includes('team leave sdkl'));
  assert.ok(commands.includes('tag sdkl remove nbmc_duel'));
  assert.ok(commands.includes('gamemode survival sdkl'));
  assert.ok(commands.some(command=>command.startsWith('item replace entity sdkl hotbar.0 with minecraft:netherite_spear')));
  assert.ok(commands.some(command=>command.startsWith('item replace entity sdkl hotbar.3 with minecraft:netherite_chestplate')));
  assert.equal(commands.filter(command=>/^item replace entity sdkl inventory\./.test(command)).length,16);
  assert.ok(!commands.includes('clear sdkl'));
  const teleports = commands.filter(command=>/^tp (Mace|Spear)/.test(command));
  assert.equal(new Set(teleports.map(command=>command.split(' ').slice(2,5).join(' '))).size,7);
  assert.ok(commands.filter(command=>command.includes('hotbar.0 with minecraft:mace')).every(command=>command.includes('"minecraft:wind_burst":3')));
  assert.ok(commands.includes('team modify nbmc_attackers friendlyFire false'));
});

test('records must match explicit total but old records remain compatible', () => {
  const explicit = duelPlan({NBMCBOT_DUEL_TOTAL:'1'});
  assert.deepEqual(validateGroupNames(explicit.groups[0],['Mace001'],true),['Mace001']);
  assert.throws(()=>validateGroupNames(explicit.groups[0],['Mace001','Mace002'],true));
  assert.throws(()=>validateGroupNames(explicit.groups[0],['Mace001','Mace001'],false));
  assert.deepEqual(validateGroupNames(duelPlan({}).groups[0],['Mace001'],false),['Mace001']);
  const normal=duelPlan({NBMCBOT_DUEL_MODE:'mace-vs-spear'});
  assert.deepEqual(normal.groups.map(group=>[group.style,group.enemy,group.team]),[['mace','Spear','nbmc_mace'],['spear','Mace','nbmc_spear']]);
});

test('one hundred attackers have distinct ring positions and final healing follows equipment', () => {
  const plan = duelPlan({NBMCBOT_DUEL_MODE:'mace-vs-player',NBMCBOT_DUEL_PLAYER:'sdkl',NBMCBOT_DUEL_TOTAL:'100'});
  const groups = plan.groups.map(group=>({...group,names:Array.from({length:group.count},(_,i)=>`${group.prefix}${String(i+1).padStart(3,'0')}`)}));
  const commands = loadoutCommands(plan,groups);
  const teleports = commands.filter(command=>/^tp (Mace|Spear)/.test(command));
  assert.equal(teleports.length,100);
  assert.equal(new Set(teleports.map(command=>command.split(' ').slice(2,5).join(' '))).size,100);
  assert.equal(commands.filter(command=>command==='team add nbmc_attackers').length,1);
  assert.ok(commands.indexOf('gamemode spectator sdkl') < commands.indexOf('execute as sdkl run spectate'));
  assert.ok(commands.indexOf('execute as sdkl run spectate') < commands.indexOf('gamemode survival sdkl'));
  for (const name of [...groups.flatMap(group=>group.names),'sdkl']) {
    const lastEquipment = commands.findLastIndex(command=>command.startsWith(`item replace entity ${name} `));
    const heal = commands.indexOf(`effect give ${name} minecraft:instant_health 1 10 true`);
    assert.ok(heal > lastEquipment);
  }
});

test('team modes retain their distinct teams, weapons and line spawns', () => {
  for (const mode of ['mace-vs-spear','mace-vs-mace']) {
    const plan = duelPlan({NBMCBOT_DUEL_MODE:mode});
    const commands = loadoutCommands(plan,plan.groups.map(group=>({...group,names:[`${group.prefix}001`]})));
    assert.ok(commands.includes('team join nbmc_mace Mace001'));
    assert.ok(commands.includes('team join nbmc_spear Spear001'));
    assert.ok(commands.includes('tp Mace001 -12 64 0 -90 0'));
    assert.ok(commands.includes('tp Spear001 12 64 0 90 0'));
    assert.ok(commands.includes('spawnpoint Mace001 -12 64 0 -90 0'));
    assert.ok(commands.includes('spawnpoint Spear001 12 64 0 90 0'));
    assert.ok(commands.some(command=>command.startsWith(`item replace entity Spear001 hotbar.0 with minecraft:${mode==='mace-vs-mace'?'mace':'netherite_spear'}`)));
    assert.ok(commands.filter(command=>command.includes('hotbar.0 with minecraft:mace')).every(command=>command.includes('"minecraft:wind_burst":3')));
  }
});
