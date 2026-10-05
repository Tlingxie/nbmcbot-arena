#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { closeSync, constants, openSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const duration = Number(process.argv[2] ?? 120);
const reportPath = resolve(root, '.runtime/arena-100-split-memory.json');
const started = Date.now();
const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const limit = 256000000;
const report = {
  started_at: new Date(started).toISOString(), minecraft: '1.21.11', requested_bots: 100,
  requested_seconds: duration, per_process_limit_bytes: limit,
  measurement: 'One ps invocation samples both bot PIDs every 500 ms; status requests to both groups every five seconds.',
  event_scope: 'Only log bytes at or after each process record log_offset are parsed. Event counts cover those process lifetimes; RSS samples cover this measurement window.',
  excluded: 'Minecraft server and temporary Node/ps measurement tools; bots have no required helpers.',
  processes: [], samples: [], errors: [],
};

function readEvents(group) {
  const bytes = readFileSync(group.record.log);
  const offset = group.record.log_offset ?? 0;
  if (!Number.isSafeInteger(offset) || offset < 0 || offset > bytes.length) throw new Error(`${group.name}: invalid log_offset or log truncated`);
  const raw = bytes.subarray(offset).toString('utf8');
  const complete = raw.slice(0, raw.lastIndexOf('\n') + 1);
  const events = complete.split('\n').filter(line => line.startsWith('{')).map(line => JSON.parse(line));
  return { events, raw };
}
function requestStatus(group) {
  const descriptor = openSync(group.record.pipe, constants.O_WRONLY | constants.O_NONBLOCK);
  try { writeFileSync(descriptor, 'status\n'); } finally { closeSync(descriptor); }
}
function collectSample(groups) {
  const ids = groups.map(group => group.record.pid);
  const rows = execFileSync('ps', ['-o', 'pid=,rss=,comm=', '-p', ids.join(',')], { encoding: 'utf8', timeout: 3000 }).trim().split('\n');
  const rss = {};
  for (const line of rows) {
    const row = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
    if (!row || !ids.includes(Number(row[1])) || basename(row[3].trim()) !== 'nbmcbot' || rss[row[1]] !== undefined) {
      throw new Error(`missing/duplicate bot PID or process identity changed: ${line}`);
    }
    rss[row[1]] = Number(row[2]) * 1024;
  }
  if (Object.keys(rss).length !== 2) throw new Error('RSS sample must contain both bot processes');
  report.samples.push({ elapsed_ms: Date.now() - started, rss_by_pid: rss, total_rss_bytes: Object.values(rss).reduce((sum, value) => sum + value, 0) });
  for (const group of groups) if (rss[group.record.pid] >= limit) throw new Error(`${group.name}: sampled RSS reached the per-process limit`);
}
let groups = [];
try {
  if (!Number.isInteger(duration) || duration < 1 || duration > 3600) throw new Error('usage: node scripts/measure-arena-split.mjs [seconds:1..3600]');
  groups = ['swarm-a', 'swarm-b'].map(name => {
    const recordPath = resolve(root, `.runtime/vanilla-1.21.11/${name}-process.json`);
    const record = JSON.parse(readFileSync(recordPath));
    if (!Number.isSafeInteger(record.pid) || record.pid <= 0) throw new Error(`${name}: invalid PID`);
    const output = { name, pid: record.pid, process_started_at: record.started_at, log: record.log, log_offset: record.log_offset ?? 0,
      binary_sha256: createHash('sha256').update(readFileSync(record.binary)).digest('hex') };
    report.processes.push(output);
    return { name, record, output };
  });
  if (groups[0].record.pid === groups[1].record.pid) throw new Error('group records refer to the same PID');
  let lastRequest = -Infinity;
  while (Date.now() - started < duration * 1000) {
    collectSample(groups);
    if (Date.now() - started - lastRequest >= 5000) {
      for (const group of groups) requestStatus(group);
      lastRequest = Date.now() - started;
    }
    await pause(500);
  }
  // Require a new reply from both processes rather than accepting an old status.
  const previousCounts = groups.map(group => readEvents(group).events.filter(event => event.event === 'swarm_status').length);
  for (const group of groups) requestStatus(group);
  const deadline = Date.now() + 10000;
  while (true) {
    collectSample(groups);
    const fresh = groups.every((group, index) => readEvents(group).events.filter(event => event.event === 'swarm_status').length > previousCounts[index]);
    if (fresh) break;
    if (Date.now() >= deadline) throw new Error('did not receive fresh final status from both groups');
    await pause(500);
  }
} catch (error) {
  report.errors.push(error.stack ?? String(error));
} finally {
  for (const group of groups) {
    try {
      const { events, raw } = readEvents(group);
      const output = group.output;
      output.statuses = events.filter(event => event.event === 'swarm_status');
      output.last_status = output.statuses.at(-1) ?? null;
      output.disconnects = events.filter(event => event.event === 'disconnected');
      output.disconnect_count = output.disconnects.length;
      output.spawned_bots = [...new Set(events.filter(event => event.event === 'spawn').map(event => event.bot))];
      output.attack_count = events.filter(event => event.event === 'combat_attack').length;
      output.attacking_bots = [...new Set(events.filter(event => event.event === 'combat_attack').map(event => event.bot))];
      output.runtime_errors = events.filter(event => ['error', 'command_error', 'bot_failed', 'task_error', 'plugin_error', 'connection_failed'].includes(event.event));
      if (output.runtime_errors.length) report.errors.push({ group: group.name, runtime_errors: output.runtime_errors });
      if (/panicked at|Encountered a panic|thread .* panicked/.test(raw)) report.errors.push(`${group.name}: panic found after log_offset`);
      const samples = report.samples.map(sample => sample.rss_by_pid[group.record.pid]).filter(Number.isFinite);
      output.mean_rss_bytes = samples.length ? Math.round(samples.reduce((sum, value) => sum + value, 0) / samples.length) : null;
      output.sampled_peak_rss_bytes = samples.reduce((peak, value) => Math.max(peak, value), 0);
      output.reported_os_peak_rss_bytes = output.statuses.reduce((peak, status) => Math.max(peak, Number(status.peak_rss_bytes) || 0), 0);
      output.under_process_limit = Math.max(output.sampled_peak_rss_bytes, output.reported_os_peak_rss_bytes) < limit;
      if (!output.under_process_limit) report.errors.push(`${group.name}: sampled or reported peak exceeded/reached ${limit} bytes`);
      if (output.last_status?.bots !== 50 || output.last_status?.connected !== 50) report.errors.push(`${group.name}: final status must show 50/50 connected`);
    } catch (error) { report.errors.push(`${group.name}: ${error.stack ?? String(error)}`); }
  }
  report.summary = {
    sampled_simultaneous_peak_rss_bytes: report.samples.reduce((peak, sample) => Math.max(peak, sample.total_rss_bytes), 0),
    mean_simultaneous_rss_bytes: report.samples.length ? Math.round(report.samples.reduce((sum, sample) => sum + sample.total_rss_bytes, 0) / report.samples.length) : null,
    sum_of_individual_reported_os_peaks_bytes: report.processes.reduce((sum, group) => sum + (group.reported_os_peak_rss_bytes ?? 0), 0),
    latest_connected_total: report.processes.reduce((sum, group) => sum + (group.last_status?.connected ?? 0), 0),
    disconnect_count: report.processes.reduce((sum, group) => sum + (group.disconnect_count ?? 0), 0),
  };
  if (report.summary.latest_connected_total !== 100) report.errors.push('latest connected total is not 100');
  if (!report.samples.length) report.errors.push('no valid simultaneous RSS samples');
  report.elapsed_seconds = (Date.now() - started) / 1000;
  report.finished_at = new Date().toISOString();
  report.success = report.errors.length === 0;
  writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ report: reportPath, success: report.success, ...report.summary, processes: report.processes.map(({ name, pid, mean_rss_bytes, sampled_peak_rss_bytes, reported_os_peak_rss_bytes, disconnect_count }) => ({ name, pid, mean_rss_bytes, sampled_peak_rss_bytes, reported_os_peak_rss_bytes, disconnect_count })), errors: report.errors }));
  if (!report.success) process.exitCode = 1;
}
