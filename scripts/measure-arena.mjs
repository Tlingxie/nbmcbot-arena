#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { closeSync, constants, openSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const duration = Number(process.argv[2] ?? 60);
if (!Number.isInteger(duration) || duration < 1 || duration > 3600) {
  throw new Error('usage: node scripts/measure-arena.mjs [seconds:1..3600]');
}
const record = JSON.parse(readFileSync(resolve(root, '.runtime/vanilla-1.21.11/swarm-process.json')));
const reportPath = resolve(root, '.runtime/arena-100-memory.json');
const started = Date.now();
const report = {
  started_at: new Date(started).toISOString(), pid: record.pid,
  process_started_at: record.started_at, server: '127.0.0.1:25566',
  minecraft: '1.21.11', requested_bots: 100, requested_seconds: duration,
  binary_sha256: createHash('sha256').update(readFileSync(record.binary)).digest('hex'),
  measurement: '500 ms ps RSS samples of one shared bot process; status every five seconds',
  event_scope: 'combat and spawn totals cover this process lifetime; RSS samples cover the measurement window',
  excluded: 'Minecraft server and this temporary Node/ps measurement tool',
  samples: [], statuses: [], errors: [],
};
let lastStatusRequest = -Infinity;
function events() {
  return readFileSync(record.log).subarray(record.log_offset ?? 0).toString('utf8')
    .split('\n').filter(line => line.startsWith('{')).flatMap(line => {
      try { return [JSON.parse(line)]; } catch { return []; }
    });
}
try {
  while (Date.now() - started < duration * 1000) {
    const elapsed = Date.now() - started;
    const row = execFileSync('ps', ['-o', 'rss=,comm=', '-p', String(record.pid)], { encoding: 'utf8' }).trim();
    const match = row.match(/^(\d+)\s+(.+)$/);
    if (!match || basename(match[2]) !== 'nbmcbot') throw new Error('swarm process exited or PID identity changed');
    report.samples.push({ elapsed_ms: elapsed, rss_bytes: Number(match[1]) * 1024 });
    if (elapsed - lastStatusRequest >= 5000) {
      const pipe = openSync(record.pipe, constants.O_WRONLY | constants.O_NONBLOCK);
      try { writeFileSync(pipe, 'status\n'); } finally { closeSync(pipe); }
      lastStatusRequest = elapsed;
    }
    await new Promise(resolve => setTimeout(resolve, 500));
  }
} catch (error) {
  report.errors.push(error.message);
}
const observed = events();
const rawLog = readFileSync(record.log).subarray(record.log_offset ?? 0).toString('utf8');
if (/panicked at|Encountered a panic/.test(rawLog)) report.errors.push('panic found in current process log');
report.statuses = observed.filter(event => event.event === 'swarm_status');
report.disconnects = observed.filter(event => event.event === 'disconnected');
report.keepalive_count = observed.filter(event => event.event === 'keepalive_received').length;
report.errors.push(...observed.filter(event => ['error', 'bot_failed', 'task_error'].includes(event.event)));
const attacks = observed.filter(event => event.event === 'combat_attack');
report.attack_count = attacks.length;
report.attacking_bots = [...new Set(attacks.map(event => event.bot))];
report.spawned_bots = [...new Set(observed.filter(event => event.event === 'spawn').map(event => event.bot))];
report.elapsed_seconds = (Date.now() - started) / 1000;
report.summary = {
  mean_rss_bytes: report.samples.length ? Math.round(report.samples.reduce((sum, sample) => sum + sample.rss_bytes, 0) / report.samples.length) : null,
  max_sample_rss_bytes: Math.max(0, ...report.samples.map(sample => sample.rss_bytes)),
  reported_peak_rss_bytes: Math.max(0, ...report.statuses.map(status => status.peak_rss_bytes)),
  max_connected: Math.max(0, ...report.statuses.map(status => status.connected)),
  last_status: report.statuses.at(-1) ?? null,
};
writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ report: reportPath, ...report.summary, errors: report.errors }));
if (report.errors.length || report.summary.last_status?.connected !== 100) process.exitCode = 1;
