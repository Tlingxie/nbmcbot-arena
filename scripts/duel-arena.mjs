#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { cpSync, readFileSync, statSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const helper = resolve(root, 'scripts/arena-process.mjs');
const [action, ...options] = process.argv.slice(2);
const dryRun = options.length === 1 && options[0] === '--dry-run';
if (!['setup', 'fight', 'stop', 'status', 'resupply'].includes(action) || (options.length && !dryRun)) {
  throw new Error('usage: node scripts/duel-arena.mjs setup|fight|stop|status|resupply [--dry-run]');
}
const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
function control(kind, action, command) {
  if (dryRun) {
    console.log(JSON.stringify({ kind, action, ...(command ? { command } : {}) }));
    return null;
  }
  return execFileSync(process.execPath, [helper, kind, action, ...(command ? [command] : [])], {
    cwd: root, encoding: 'utf8', timeout: 5000,
  }).trim();
}
function requireRunning(kind) {
  const record = JSON.parse(control(kind, 'status'));
  if (!record.running) throw new Error(`${kind} is not running; start it with scripts/arena-process.mjs first`);
  return record;
}
const count = Number(process.env.NBMCBOT_DUEL_COUNT ?? 5);
if (!Number.isInteger(count) || count < 1 || count > 50) throw new Error('NBMCBOT_DUEL_COUNT must be an integer from 1 to 50');
const mode = process.env.NBMCBOT_DUEL_MODE ?? 'mace-vs-spear';
if (!['mace-vs-mace', 'mace-vs-spear'].includes(mode)) throw new Error('NBMCBOT_DUEL_MODE must be mace-vs-mace or mace-vs-spear');
const groups = [
  { kind: 'mace-team', prefix: 'Mace', team: 'nbmc_mace', style: 'mace', enemy: 'Spear', x: -12, yaw: -90 },
  { kind: 'spear-team', prefix: 'Spear', team: 'nbmc_spear', style: mode === 'mace-vs-mace' ? 'mace' : 'spear', enemy: 'Mace', x: 12, yaw: 90 },
].map(group => {
  const record = dryRun ? null : requireRunning(group.kind);
  const names = record?.usernames ?? Array.from({ length: count }, (_, index) => `${group.prefix}${String(index + 1).padStart(3, '0')}`);
  if (names.length < 1 || names.length > 50 || names.some(name => !new RegExp(`^${group.prefix}\\d{3}$`).test(name))) {
    throw new Error(`${group.kind}: invalid test-account list in process record`);
  }
  return { ...group, names, record };
});
const server = dryRun ? null : requireRunning('server');
function logSince(record, offset) {
  return readFileSync(record.log).subarray(offset).toString('utf8');
}
function setupErrors(output) {
  return output.split('\n').filter(line => {
    const message = line.replace(/^.*\]:\s*/, '');
    if (/already exists by that name|already has (?:that|the) tag|No effects found|No items were found|^Nothing changed\./i.test(message)) return false;
    return /unknown (?:or incomplete command|item|item component|argument|player|team)|incorrect argument|invalid (?:item|component|argument|number|player)|malformed|expected |not a valid|could not parse|can't parse|cannot parse|no (?:player|entity|entities) (?:was |were )?found|only players may|failed to|error occurred|<--\[HERE\]/i.test(message);
  });
}
async function serverCommands(commands) {
  const offset = dryRun ? 0 : statSync(server.log).size;
  for (const command of commands) control('server', 'command', command);
  if (dryRun) return '';
  const marker = `NBMC_DUEL_DONE_${process.pid}_${Date.now()}`;
  control('server', 'command', `say ${marker}`);
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    const output = logSince(server, offset);
    if (output.includes(marker)) return output.split('\n').filter(line => !line.includes(marker)).join('\n').trim();
    await pause(100);
  }
  throw new Error('server did not confirm processing commands within 10 seconds; inspect its console log');
}
async function groupStatus(group) {
  const offset = dryRun ? 0 : statSync(group.record.log).size;
  control(group.kind, 'command', 'status');
  if (dryRun) return null;
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    const statuses = logSince(group.record, offset).split('\n').filter(line => line.startsWith('{')).flatMap(line => {
      try { return [JSON.parse(line)]; } catch { return []; }
    }).filter(event => event.event === 'swarm_status');
    if (statuses.length) return statuses.at(-1);
    await pause(100);
  }
  throw new Error(`${group.kind}: no fresh status within 10 seconds`);
}
function livingStatusProblems(names, statuses) {
  return names.flatMap(name => {
    const status = statuses.get(name);
    if (!status) return [`${name}: no fresh status`];
    if (status.connected !== true) return [`${name}: disconnected`];
    if (!Number.isFinite(status.health) || status.health <= 0) return [`${name}: health=${status.health}`];
    if (status.task != null) return [`${name}: task is still ${status.task}`];
    return [];
  });
}
async function waitForLivingBots() {
  if (dryRun) {
    for (const group of groups) control(group.kind, 'command', 'status');
    console.log(JSON.stringify({ check: 'fresh per-account status: connected, health > 0, task stopped before any server setup', timeout_ms: 10000 }));
    return;
  }
  const offsets = new Map(groups.map(group => [group.kind, statSync(group.record.log).size]));
  const statuses = new Map();
  const names = groups.flatMap(group => group.names);
  const deadline = Date.now() + 10000;
  let nextRequest = 0;
  while (Date.now() < deadline) {
    if (Date.now() >= nextRequest) {
      for (const group of groups) control(group.kind, 'command', 'status');
      nextRequest = Date.now() + 500;
    }
    for (const group of groups) {
      for (const line of logSince(group.record, offsets.get(group.kind)).split('\n')) {
        if (!line.startsWith('{')) continue;
        let event;
        try { event = JSON.parse(line); } catch { continue; }
        if (event.event === 'status' && group.names.includes(event.bot)) statuses.set(event.bot, event);
      }
    }
    if (!livingStatusProblems(names, statuses).length) return;
    await pause(100);
  }
  throw new Error(`test accounts did not finish respawning within 10 seconds: ${livingStatusProblems(names, statuses).join('; ')}. No server healing, equipment or teleport commands were sent.`);
}
function setupCommands() {
  const commands = [];
  const unbreakable = 'minecraft:unbreakable={}';
  const armor = `${unbreakable},minecraft:enchantments={"minecraft:protection":4}`;
  const boots = `${unbreakable},minecraft:enchantments={"minecraft:protection":4,"minecraft:feather_falling":4}`;
  for (const group of groups) {
    commands.push(`team add ${group.team}`, `team modify ${group.team} friendlyFire false`,
      `team modify ${group.team} color ${group.kind === 'mace-team' ? 'red' : 'blue'}`);
    for (const [index, name] of group.names.entries()) {
      const z = (index - (group.names.length - 1) / 2) * 3;
      commands.push(
        `tag ${name} add nbmc_duel`,
        `team join ${group.team} ${name}`,
        `gamemode survival ${name}`,
        `clear ${name}`,
        `effect clear ${name}`,
        `effect give ${name} minecraft:instant_health 1 5 true`,
        `effect give ${name} minecraft:saturation 1 5 true`,
        `item replace entity ${name} armor.head with minecraft:netherite_helmet[${armor}]`,
        `item replace entity ${name} armor.legs with minecraft:netherite_leggings[${armor}]`,
        `item replace entity ${name} armor.feet with minecraft:netherite_boots[${boots}]`,
        `item replace entity ${name} armor.chest with minecraft:elytra[${unbreakable}]`,
        `item replace entity ${name} hotbar.0 with minecraft:${group.style === 'mace' ? 'mace' : 'netherite_spear'}[minecraft:unbreakable={}]`,
        `item replace entity ${name} weapon.offhand with minecraft:firework_rocket[minecraft:fireworks={flight_duration:1,explosions:[]}] 64`,
      );
      if (group.style === 'mace') {
        commands.push(`item replace entity ${name} hotbar.1 with minecraft:wind_charge 64`,
          `item replace entity ${name} hotbar.2 with minecraft:ender_pearl 16`,
          `item replace entity ${name} hotbar.3 with minecraft:netherite_chestplate[${armor}]`);
      }
      commands.push(`spawnpoint ${name} ${group.x} 64 ${Math.floor(z)} ${group.yaw} 0`,
        `tp ${name} ${group.x} 64 ${z} ${group.yaw} 0`);
    }
  }
  return commands;
}

if (action === 'resupply') {
  const pack = resolve(root, 'datapacks/nbmcbot_supply');
  const destination = resolve(root, '.runtime/vanilla-1.21.11/arena/datapacks/nbmcbot_supply');
  if (dryRun) console.log(JSON.stringify({ copy: pack, destination }));
  else cpSync(pack, destination, { recursive: true });
  const reload = await serverCommands(['reload']);
  if (reload) console.log(reload);
  // A reload can finish after the console marker; this function call also
  // verifies that the pack is enabled before any supplies are issued.
  if (!dryRun) await pause(1000);
  const probe = await serverCommands(['function nbmcbot_supply:tick']);
  const reloadErrors = setupErrors(`${reload}\n${probe}`);
  if (reloadErrors.length || /unknown function|failed to load|couldn't load/i.test(`${reload}\n${probe}`)) {
    throw new Error(`rocket reload pack was not verified:\n${reload}\n${probe}`);
  }
  const rocket = 'minecraft:firework_rocket[minecraft:fireworks={flight_duration:1,explosions:[]}] 64';
  const commands = groups.flatMap(group => group.names.flatMap(name => [
    `tag ${name} add nbmc_duel`,
    `execute unless items entity ${name} weapon.offhand * run item replace entity ${name} weapon.offhand with ${rocket}`,
    ...Array.from({ length: 16 }, (_, slot) =>
      `execute unless items entity ${name} inventory.${slot} * run item replace entity ${name} inventory.${slot} with ${rocket}`),
  ]));
  const output = await serverCommands(commands);
  if (output) console.log(output);
  const errors = setupErrors(output);
  if (errors.length) throw new Error(`resupply failed:\n${errors.join('\n')}`);
  console.log(JSON.stringify({ action, accounts: groups.reduce((sum, group) => sum + group.names.length, 0), reserve_slots: 16, rockets_per_empty_slot: 64, dry_run: dryRun }));
} else if (action === 'setup') {
  for (const group of groups) control(group.kind, 'command', 'module auto-respawn on');
  for (const group of groups) control(group.kind, 'command', 'stop');
  await waitForLivingBots();
  const output = await serverCommands(setupCommands());
  if (output) console.log(output);
  const errors = setupErrors(output);
  if (errors.length) throw new Error(`arena setup was not verified; server rejected commands:\n${errors.join('\n')}`);
  console.log(JSON.stringify({ action, prepared_accounts: groups.reduce((sum, group) => sum + group.names.length, 0), dry_run: dryRun }));
} else if (action === 'fight' || action === 'stop') {
  if (action === 'fight') {
    for (const group of groups) control(group.kind, 'command', 'module auto-respawn off');
  }
  for (const group of groups) control(group.kind, 'command', action === 'stop' ? 'stop' : `duel ${group.style} ${group.enemy}`);
  console.log(JSON.stringify({ action, groups: groups.map(({ kind, names }) => ({ kind, accounts: names.length })), dry_run: dryRun }));
} else {
  const statuses = [];
  for (const group of groups) statuses.push({ kind: group.kind, status: await groupStatus(group) });
  const output = await serverCommands(groups.flatMap(group => group.names.map(name => `data get entity ${name} Health`)));
  if (output) console.log(output);
  console.log(JSON.stringify({ groups: statuses, dry_run: dryRun }));
}
