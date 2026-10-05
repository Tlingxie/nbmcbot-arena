#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { closeSync, constants, mkdirSync, openSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const dryRun = args.at(-1) === '--dry-run';
if (dryRun) args.pop();
const duration = Number(args[0] ?? 60);
if (args.length > 1 || !Number.isInteger(duration) || duration < 1 || duration > 3600) {
  throw new Error('usage: node scripts/measure-duel.mjs [seconds:1..3600] [--dry-run]');
}
const runtime = resolve(root, '.runtime/vanilla-1.21.11');
const reportPath = resolve(root, '.runtime/duel-measurement.json');
const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const limit = 256000000;
const observationNote = 'Position and velocity are client observations. Speed is the velocity norm times 20 ticks/second; it is not server-confirmed displacement.';
const evidenceNote = 'duel_action, duel_phase and duel_attack count issued decisions/attempts, not successful hits. Server-source duel_damage events can confirm an attributed opponent hit but contain no damage amount. Health can change from falls, pearls or regeneration; server death text separately attributes a kill.';
if (dryRun) {
  console.log(JSON.stringify({ report: reportPath, seconds: duration, groups: ['mace-team', 'spear-team'],
    samples_every_ms: 500, status_every_ms: 5000, log_scope: 'measurement window only',
    server_reads: 'Health queries for each recorded account at start/end',
    process_control: 'none; does not start, stop, equip, teleport or arm combat', observation_note: observationNote, evidence_note: evidenceNote }));
  process.exit(0);
}

const report = {
  minecraft: '1.21.11', requested_seconds: duration, per_process_limit_bytes: limit,
  measurement: 'Both bot PIDs sampled in one ps call every 500 ms; fresh status requested every five seconds.',
  event_scope: 'Only bytes appended after measurement_log_offset are counted. Process log offsets are provenance and never expand the event window.',
  excluded: 'Minecraft server, measurement helper and ps process memory.',
  observation_note: observationNote, evidence_note: evidenceNote,
  processes: [], samples: [], errors: [], server: null,
};
let groups = [];
let server = null;
let started = Date.now();
let windowComplete = false;
function processRecord(kind) {
  const record = JSON.parse(readFileSync(resolve(runtime, `${kind}-process.json`), 'utf8'));
  if (!Number.isSafeInteger(record.pid) || record.pid < 1) throw new Error(`${kind}: invalid process PID`);
  const command = execFileSync('ps', ['-o', 'comm=', '-p', String(record.pid)], { encoding: 'utf8', timeout: 3000 }).trim();
  if (basename(command) !== (kind === 'server' ? 'java' : 'nbmcbot')) throw new Error(`${kind}: process identity changed`);
  const offset = statSync(record.log).size;
  if (!Number.isSafeInteger(record.log_offset) || record.log_offset < 0 || record.log_offset > offset) throw new Error(`${kind}: invalid process log offset`);
  return { kind, record, offset };
}
function readWindow(process) {
  const bytes = readFileSync(process.record.log);
  if (bytes.length < process.offset) throw new Error(`${process.kind}: log was truncated during measurement`);
  const raw = bytes.subarray(process.offset).toString('utf8');
  return raw.slice(0, raw.lastIndexOf('\n') + 1);
}
function events(group) {
  return readWindow(group).split('\n').filter(line => line.startsWith('{')).map(line => JSON.parse(line));
}
function send(process, command) {
  const descriptor = openSync(process.record.pipe, constants.O_WRONLY | constants.O_NONBLOCK);
  try { writeFileSync(descriptor, `${command}\n`); } finally { closeSync(descriptor); }
}
function sample() {
  const ids = groups.map(group => group.record.pid);
  const rows = execFileSync('ps', ['-o', 'pid=,rss=,comm=', '-p', ids.join(',')], { encoding: 'utf8', timeout: 3000 }).trim().split('\n');
  const rss = {};
  for (const line of rows) {
    const match = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
    if (!match || !ids.includes(Number(match[1])) || basename(match[3]) !== 'nbmcbot' || rss[match[1]] !== undefined) throw new Error(`unexpected process in RSS sample: ${line}`);
    rss[match[1]] = Number(match[2]) * 1024;
  }
  if (Object.keys(rss).length !== groups.length) throw new Error('RSS sample is missing a bot process');
  report.samples.push({ elapsed_ms: Date.now() - started, rss_by_pid: rss, total_rss_bytes: Object.values(rss).reduce((sum, value) => sum + value, 0) });
}
async function healthSnapshot(label) {
  const offset = statSync(server.record.log).size;
  const names = groups.flatMap(group => group.record.usernames);
  const sentAt = new Date().toISOString();
  for (const name of names) send(server, `data get entity ${name} Health`);
  const deadline = Date.now() + 5000;
  const health = {};
  let raw = '';
  while (Date.now() < deadline) {
    raw = readFileSync(server.record.log).subarray(offset).toString('utf8');
    for (const line of raw.split('\n')) {
      const match = line.match(/\]: (\w+) has the following entity data: (-?[\d.]+)f\s*$/);
      if (match && names.includes(match[1])) health[match[1]] = Number(match[2]);
    }
    if (Object.keys(health).length === names.length) break;
    sample();
    await pause(500);
  }
  const missing = names.filter(name => health[name] === undefined);
  if (missing.length) report.errors.push(`${label} health query had no reply for: ${missing.join(', ')}`);
  return { requested_at: sentAt, received_at: new Date().toISOString(), health, missing,
    errors: raw.split('\n').filter(line => /No (?:player|entity|entities) (?:was |were )?found|Unknown or incomplete command|Incorrect argument/i.test(line)) };
}
function counts(items, key) {
  const result = {};
  for (const item of items) result[String(item[key] ?? 'unspecified')] = (result[String(item[key] ?? 'unspecified')] ?? 0) + 1;
  return result;
}
function confirmedOpponentHits(items, victimNames, opponentNames, opponentPrefix) {
  return items.filter(item => {
    const victim = item.victim ?? item.bot;
    return item.event === 'duel_damage' && item.source === 'server'
      && victimNames.includes(victim) && (item.bot === undefined || item.bot === victim)
      && typeof item.attacker === 'string' && item.attacker.startsWith(opponentPrefix)
      && opponentNames.includes(item.attacker);
  });
}
function observationSummary(items) {
  let peakHeight = null;
  let peakSpeed = null;
  let peakHorizontalSpeed = null;
  for (const item of items) {
    const y = Array.isArray(item.position) ? item.position[1] : null;
    if (Number.isFinite(y) && (peakHeight === null || y > peakHeight.value)) peakHeight = { value: y, bot: item.bot, tick: item.tick, phase: item.phase };
    if (Array.isArray(item.velocity) && item.velocity.length === 3 && item.velocity.every(Number.isFinite)) {
      const [x, yVelocity, z] = item.velocity;
      const speed = Math.hypot(x, yVelocity, z) * 20;
      const horizontal = Math.hypot(x, z) * 20;
      if (peakSpeed === null || speed > peakSpeed.value) peakSpeed = { value: speed, bot: item.bot, tick: item.tick, phase: item.phase };
      if (peakHorizontalSpeed === null || horizontal > peakHorizontalSpeed.value) peakHorizontalSpeed = { value: horizontal, bot: item.bot, tick: item.tick, phase: item.phase };
    }
  }
  return { count: items.length, gliding_observations: items.filter(item => item.gliding === true).length,
    max_y_blocks: peakHeight, max_speed_blocks_per_second: peakSpeed, max_horizontal_speed_blocks_per_second: peakHorizontalSpeed,
    first: items[0] ?? null, last: items.at(-1) ?? null };
}

try {
  groups = ['mace-team', 'spear-team'].map(kind => {
    const group = processRecord(kind);
    const prefix = kind === 'mace-team' ? 'Mace' : 'Spear';
    if (!Array.isArray(group.record.usernames) || !group.record.usernames.length || group.record.usernames.some(name => !new RegExp(`^${prefix}\\d{3}$`).test(name))) {
      throw new Error(`${kind}: missing or invalid recorded account list`);
    }
    group.output = { name: kind, pid: group.record.pid, accounts: group.record.usernames,
      process_started_at: group.record.started_at, log: group.record.log, process_log_offset: group.record.log_offset,
      measurement_log_offset: group.offset, binary_sha256: createHash('sha256').update(readFileSync(group.record.binary)).digest('hex') };
    report.processes.push(group.output);
    return group;
  });
  if (groups[0].record.pid === groups[1].record.pid) throw new Error('both groups refer to the same process PID');
  server = processRecord('server');
  report.server = { pid: server.record.pid, log: server.record.log, process_log_offset: server.record.log_offset, measurement_log_offset: server.offset };
  started = Date.now();
  report.started_at = new Date(started).toISOString();
  sample();
  report.server.start = await healthSnapshot('start');
  let lastStatus = -Infinity;
  while (Date.now() - started < duration * 1000) {
    sample();
    if (Date.now() - started - lastStatus >= 5000) {
      for (const group of groups) send(group, 'status');
      lastStatus = Date.now() - started;
    }
    await pause(500);
  }
  windowComplete = true;
  const previousCounts = groups.map(group => events(group).filter(event => event.event === 'swarm_status').length);
  for (const group of groups) send(group, 'status');
  const deadline = Date.now() + 10000;
  while (!groups.every((group, index) => events(group).filter(event => event.event === 'swarm_status').length > previousCounts[index])) {
    if (Date.now() >= deadline) throw new Error('fresh final status did not arrive from both groups');
    sample();
    await pause(500);
  }
  report.server.end = await healthSnapshot('end');
} catch (error) {
  report.errors.push(error.stack ?? String(error));
} finally {
  for (const group of groups) {
    try {
      const data = events(group);
      const output = group.output;
      output.statuses = data.filter(event => event.event === 'swarm_status');
      output.last_status = output.statuses.at(-1) ?? null;
      output.disconnects = data.filter(event => event.event === 'disconnected');
      output.runtime_errors = data.filter(event => ['error', 'command_error', 'bot_failed', 'task_error', 'plugin_error', 'connection_failed'].includes(event.event));
      output.issued_actions = counts(data.filter(event => event.event === 'duel_action'), 'action');
      output.phase_entries = counts(data.filter(event => event.event === 'duel_phase'), 'phase');
      output.duel_event_counts = Object.fromEntries(['duel_action', 'duel_phase', 'duel_attack', 'duel_damage'].map(type => [type, data.filter(event => event.event === type).length]));
      output.attack_attempts_by_bot = counts(data.filter(event => event.event === 'duel_attack'), 'bot');
      output.damage_events = data.filter(event => event.event === 'duel_damage');
      const opponents = groups.filter(other => other.kind !== group.kind).flatMap(other => other.record.usernames);
      output.server_confirmed_hits = confirmedOpponentHits(output.damage_events, group.record.usernames, opponents, group.kind === 'mace-team' ? 'Spear' : 'Mace');
      output.server_confirmed_hit_count = output.server_confirmed_hits.length;
      output.server_confirmed_hits_by_attacker = counts(output.server_confirmed_hits, 'attacker');
      output.damage_amount = null;
      output.observations = observationSummary(data.filter(event => event.event === 'duel_observation'));
      output.observations_by_bot = Object.fromEntries(group.record.usernames.map(name => [name, observationSummary(data.filter(event => event.event === 'duel_observation' && event.bot === name))]));
      const samples = report.samples.map(item => item.rss_by_pid[group.record.pid]).filter(Number.isFinite);
      output.mean_rss_bytes = samples.length ? Math.round(samples.reduce((sum, value) => sum + value, 0) / samples.length) : null;
      output.sampled_peak_rss_bytes = samples.length ? Math.max(...samples) : null;
      output.reported_process_lifetime_peak_rss_bytes = output.statuses.length ? Math.max(...output.statuses.map(item => Number(item.peak_rss_bytes) || 0)) : null;
      output.sampled_under_process_limit = samples.length ? output.sampled_peak_rss_bytes < limit : null;
      output.measurement_log_end_offset = statSync(group.record.log).size;
      if (output.disconnects.length) report.errors.push(`${group.kind}: ${output.disconnects.length} disconnect event(s) during observation`);
      if (output.runtime_errors.length) report.errors.push(`${group.kind}: ${output.runtime_errors.length} runtime error event(s)`);
      if (/panicked at|Encountered a panic|thread .* panicked/.test(readWindow(group))) report.errors.push(`${group.kind}: panic during observation`);
      if (output.last_status?.connected !== group.record.usernames.length) report.errors.push(`${group.kind}: final connected count differs from expected ${group.record.usernames.length}`);
      if (output.sampled_under_process_limit === false) report.errors.push(`${group.kind}: sampled RSS reached the ${limit}-byte limit`);
    } catch (error) { report.errors.push(`${group.kind}: ${error.stack ?? String(error)}`); }
  }
  if (server) {
    try {
      const names = groups.flatMap(group => group.record.usernames);
      const lines = readWindow(server).split('\n');
      report.server.death_lines = lines.filter(line => names.some(name => line.includes(`]: ${name} `)) && / was | died| fell | hit the ground| experienced kinetic energy| drowned| blew up| suffocated| burned| tried to swim| froze to death/.test(line));
      report.server.movement_error_lines = lines.filter(line => /moved too quickly|moved wrongly|Invalid (?:player |vehicle )?movement|invalid move player|flying is not enabled/i.test(line));
      report.server.runtime_error_lines = lines.filter(line => /OutOfMemoryError|Encountered an unexpected exception|Exception in server tick loop|\/ERROR\]/.test(line));
      report.server.measurement_log_end_offset = statSync(server.record.log).size;
      if (report.server.runtime_error_lines.length) report.errors.push('server emitted runtime error(s) during observation');
    } catch (error) { report.errors.push(`server: ${error.stack ?? String(error)}`); }
  }
  report.summary = {
    sample_count: report.samples.length,
    sampled_simultaneous_peak_rss_bytes: report.samples.length ? Math.max(...report.samples.map(item => item.total_rss_bytes)) : null,
    mean_simultaneous_rss_bytes: report.samples.length ? Math.round(report.samples.reduce((sum, item) => sum + item.total_rss_bytes, 0) / report.samples.length) : null,
    final_connected_total: report.processes.reduce((sum, group) => sum + (group.last_status?.connected ?? 0), 0),
    attack_attempt_count: report.processes.reduce((sum, group) => sum + (group.duel_event_counts?.duel_attack ?? 0), 0),
    server_confirmed_opponent_hit_count: report.processes.reduce((sum, group) => sum + (group.server_confirmed_hit_count ?? 0), 0),
    server_confirmed_hits_by_attacker: counts(report.processes.flatMap(group => group.server_confirmed_hits ?? []), 'attacker'),
    server_death_line_count: report.server?.death_lines?.length ?? 0,
    server_movement_error_count: report.server?.movement_error_lines?.length ?? 0,
  };
  report.started_at ??= new Date(started).toISOString();
  report.finished_at = new Date().toISOString();
  report.elapsed_seconds = (Date.now() - started) / 1000;
  report.requested_window_elapsed = windowComplete;
  report.observation_complete = windowComplete && report.errors.length === 0 && report.samples.length > 0;
  report.combat_success = 'Not inferred from attempts; inspect server-confirmed opponent hits (no damage amount), health snapshots, attributed server death lines and tactical events together.';
  mkdirSync(dirname(reportPath), { recursive: true });
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ report: reportPath, observation_complete: report.observation_complete, ...report.summary,
    processes: report.processes.map(({ name, mean_rss_bytes, sampled_peak_rss_bytes, duel_event_counts }) => ({ name, mean_rss_bytes, sampled_peak_rss_bytes, duel_event_counts })), errors: report.errors }));
  if (!report.observation_complete) process.exitCode = 1;
}
