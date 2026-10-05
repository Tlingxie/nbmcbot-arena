#!/usr/bin/env node
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createInterface } from 'node:readline';
import { access, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { dirname, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import os from 'node:os';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const exec = promisify(execFile);
const count = Number(process.env.NBMCBOT_COUNT ?? 10);
const soakSeconds = Number(process.env.NBMCBOT_SOAK_SECONDS ?? 60);
const children = [], jobs = [];
const report = {
  started_at: new Date().toISOString(), platform: `${os.platform()} ${os.release()} ${os.arch()}`,
  minecraft: '1.21.11', protocol: 774, process_model: 'one independent release process per bot',
  measurement: 'sum of bot RSS from a single ps invocation per sample; shared pages are counted per process',
  excluded: 'local fixture servers and Node/time/ps measurement tools; bots have no required helper processes',
  bot_count: count, soak_seconds: soakSeconds, bots: [], samples: [], errors: [],
};
const started = Date.now();
let phase = 'startup', timer, sampling, sampleBusy = false, samplePending = Promise.resolve(), canceled = false;
let fail;
const fatal = new Promise((_, reject) => { fail = reject; });
fatal.catch(() => {});
const delay = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

function launch(binary, args) {
  if (canceled) throw new Error('measurement canceled');
  const child = spawn(binary, args, { cwd: root, stdio: ['pipe', 'pipe', 'pipe'] });
  children.push(child);
  child.completion = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => { child.finished = true; resolve({ code, signal }); });
  });
  child.completion.catch(fail);
  child.stdin.on('error', error => { if (phase !== 'cleanup') fail(error); });
  return child;
}

function monitor(job) {
  const pending = new Set();
  createInterface({ input: job.bot.stdout }).on('line', line => {
    let event;
    try { event = JSON.parse(line); } catch { fail(new Error(`bot ${job.id}: non-JSON output ${line}`)); return; }
    job.record.events.push({ elapsed_ms: Date.now() - started, ...event });
    if (event.event === 'spawn') job.spawned = true;
    if (['error', 'command_error', 'task_error', 'plugin_error', 'connection_failed', 'disconnected'].includes(event.event)) {
      fail(new Error(`bot ${job.id}: ${JSON.stringify(event)}`));
    }
    for (const waiter of pending) {
      if (waiter.match(event)) { pending.delete(waiter); waiter.resolve(event); }
    }
  });
  job.bot.completion.then(exit => {
    for (const waiter of pending) waiter.reject(new Error(`bot ${job.id} exited while waiting: ${JSON.stringify(exit)}`));
    pending.clear();
    if (!job.closing) fail(new Error(`bot ${job.id} exited early`));
  }).catch(fail);
  return match => {
    const result = new Promise((resolve, reject) => {
      if (job.bot.finished) { reject(new Error(`bot ${job.id} already exited`)); return; }
      pending.add({ match, resolve, reject });
    });
    result.catch(() => {});
    return result;
  };
}

function send(job, command) {
  if (canceled) return Promise.reject(new Error('measurement canceled'));
  return new Promise((resolve, reject) => job.bot.stdin.write(`${command}\n`, error => error ? reject(error) : resolve()));
}

async function discoverBot(job) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (canceled) throw new Error('measurement canceled');
    let stdout;
    try { ({ stdout } = await exec('pgrep', ['-P', String(job.bot.pid)])); } catch { await delay(20); continue; }
    const ids = stdout.trim().split(/\s+/).map(Number);
    if (ids.length !== 1 || !Number.isInteger(ids[0]) || ids[0] <= 0) throw new Error(`bot ${job.id}: ambiguous time child`);
    const details = await exec('ps', ['-o', 'pid=,ppid=,comm=', '-p', String(ids[0])]);
    const row = details.stdout.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
    if (!row || Number(row[1]) !== ids[0] || Number(row[2]) !== job.bot.pid || basename(row[3].trim()) !== 'nbmcbot') {
      throw new Error(`bot ${job.id}: child identity verification failed`);
    }
    job.pid = ids[0]; job.record.pid = job.pid; return;
  }
  throw new Error(`bot ${job.id}: could not find process`);
}

async function sample() {
  const known = jobs.filter(job => job.pid && !job.bot.finished);
  if (!known.length) return;
  const ids = known.map(job => job.pid);
  if (new Set(ids).size !== ids.length) throw new Error('duplicate bot PID');
  const { stdout } = await exec('ps', ['-o', 'pid=,rss=,comm=', '-p', ids.join(',')]);
  const rss = {};
  for (const line of stdout.trim().split('\n')) {
    const row = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
    if (!row || !ids.includes(Number(row[1])) || basename(row[3].trim()) !== 'nbmcbot') throw new Error(`invalid RSS row: ${line}`);
    rss[row[1]] = Number(row[2]) * 1024;
  }
  if (Object.keys(rss).length !== ids.length) throw new Error('RSS sample lost a bot process');
  report.samples.push({ elapsed_ms: Date.now() - started, phase, complete: ids.length === count,
    online_count: jobs.filter(job => job.spawned && !job.bot.finished).length,
    rss_by_pid: rss, total_rss_bytes: Object.values(rss).reduce((sum, value) => sum + value, 0) });
}

async function status(job) {
  const result = job.wait(event => event.event === 'status'); await send(job, 'status');
  const event = await result;
  if (event.connected !== true) throw new Error(`bot ${job.id}: not connected`);
  return event;
}

async function setup(id, binary, fixtureBinary) {
  const fixture = launch(fixtureBinary, []);
  const record = { id, username: `MeasureBot${id}`, events: [], fixture_events: [], stderr: '' };
  report.bots.push(record);
  createInterface({ input: fixture.stderr }).on('line', line => record.fixture_events.push(line));
  const address = await new Promise((resolve, reject) => {
    const lines = createInterface({ input: fixture.stdout });
    lines.once('line', line => { lines.close(); /^127\.0\.0\.1:\d+$/.test(line) ? resolve(line) : reject(new Error(`invalid fixture address ${line}`)); });
    fixture.completion.then(() => reject(new Error('fixture exited before address'))).catch(reject);
  });
  record.server = address;
  const args = [os.platform() === 'darwin' ? '-l' : '-v', binary, 'run', '--server', address, '--username', record.username, '--auth', 'offline', '--no-reconnect'];
  const job = { id, record, fixture, bot: launch('/usr/bin/time', args), spawned: false, closing: false };
  jobs.push(job);
  job.bot.stderr.on('data', chunk => { record.stderr += chunk.toString(); if (record.stderr.length > 1048576) fail(new Error(`bot ${id}: excessive stderr`)); });
  job.wait = monitor(job);
  const ready = job.wait(event => event.event === 'spawn');
  await Promise.all([ready, discoverBot(job)]);
  await status(job);
  return job;
}

async function workload(job) {
  const navigation = job.wait(event => event.event === 'task_finished' && event.task === 'goto');
  await send(job, `say measurement-hello-${job.id}`);
  await send(job, 'goto 7 64 4');
  for (let cycle = 0; cycle < 32; cycle++) {
    const loaded = job.wait(event => event.event === 'plugin_loaded'); await send(job, 'plugin load examples/plugins/anti-idle.wat'); const info = await loaded;
    const unloaded = job.wait(event => event.event === 'plugin_unloaded' && event.id === info.id); await send(job, `plugin unload ${info.id}`); await unloaded;
  }
  await navigation;
  const loaded = job.wait(event => event.event === 'plugin_loaded'); await send(job, 'plugin load examples/plugins/anti-idle.wat'); await loaded;
  const final = await status(job);
  if (!final.position || Math.floor(final.position[0]) !== 7 || Math.floor(final.position[1]) !== 64 || Math.floor(final.position[2]) !== 4) throw new Error(`bot ${job.id}: wrong final position`);
  if (!job.record.fixture_events.includes(`chat measurement-hello-${job.id}`)) throw new Error(`bot ${job.id}: server did not receive chat`);
}

try {
  if (!['darwin', 'linux'].includes(os.platform())) throw new Error('only macOS and Linux are supported');
  if (!Number.isInteger(count) || count < 1 || count > 32) throw new Error('NBMCBOT_COUNT must be 1..32');
  if (!Number.isInteger(soakSeconds) || soakSeconds < 1 || soakSeconds > 86400) throw new Error('NBMCBOT_SOAK_SECONDS must be 1..86400');
  const binary = resolve(root, 'target/release/nbmcbot'), fixtureBinary = resolve(root, 'target/release/examples/fixture_server');
  await Promise.all([access(binary), access(fixtureBinary), access('/usr/bin/time')]);
  report.binary_sha256 = createHash('sha256').update(await readFile(binary)).digest('hex');
  timer = setTimeout(() => fail(new Error('cohort measurement timed out')), (soakSeconds + 120) * 1000);
  sampling = setInterval(() => {
    if (sampleBusy) return;
    sampleBusy = true;
    samplePending = sample().catch(fail).finally(() => { sampleBusy = false; });
  }, 100);
  await Promise.race([fatal, (async () => {
    const cohort = await Promise.all(Array.from({ length: count }, (_, index) => setup(index + 1, binary, fixtureBinary)));
    phase = 'workload';
    await Promise.all(cohort.map(workload));
    phase = 'soak'; report.all_ready_ms = Date.now() - started;
    console.log(JSON.stringify({ event: 'all_bots_ready', bots: count, soak_seconds: soakSeconds }));
    for (let second = 0; second < soakSeconds; second++) {
      await delay(1000); await Promise.all(cohort.map(status));
    }
    clearInterval(sampling); await samplePending; await sample();
    phase = 'closing';
    await Promise.all(cohort.map(async job => {
      job.record.last_status = await status(job);
      const unloaded = job.wait(event => event.event === 'plugin_unloaded'); await send(job, 'plugin unload anti-idle'); await unloaded;
      job.closing = true;
      const stopped = job.wait(event => event.event === 'stopped'); await send(job, 'quit'); await stopped;
      const exit = await job.bot.completion; job.record.exit = exit;
      const fixtureExit = await job.fixture.completion;
      if (exit.code !== 0 || fixtureExit.code !== 0 || /panicked at|Encountered a panic/.test(job.record.stderr)) throw new Error(`bot ${job.id}: unsuccessful shutdown`);
      const peak = os.platform() === 'darwin' ? job.record.stderr.match(/(\d+)\s+maximum resident set size/) : job.record.stderr.match(/Maximum resident set size \(kbytes\):\s*(\d+)/);
      if (!peak) throw new Error(`bot ${job.id}: missing OS peak RSS`);
      job.record.os_peak_rss_bytes = Number(peak[1]) * (os.platform() === 'darwin' ? 1 : 1024);
      if (!job.record.fixture_events.includes('handshake protocol=774')) throw new Error(`bot ${job.id}: missing handshake`);
    }));
    const complete = report.samples.filter(sample => sample.complete && sample.online_count === count);
    const soak = complete.filter(sample => sample.phase === 'soak');
    if (soak.length < soakSeconds * 5) throw new Error('insufficient simultaneous samples');
    report.sampled_simultaneous_peak_rss_bytes = complete.reduce((peak, sample) => Math.max(peak, sample.total_rss_bytes), 0);
    report.mean_soak_total_rss_bytes = Math.round(soak.reduce((sum, sample) => sum + sample.total_rss_bytes, 0) / soak.length);
    report.last_simultaneous_rss_bytes = soak.at(-1).total_rss_bytes;
    report.sum_of_individual_os_peaks_bytes = report.bots.reduce((sum, bot) => sum + bot.os_peak_rss_bytes, 0);
    report.every_bot_under_256_mb = report.bots.every(bot => bot.os_peak_rss_bytes <= 256000000);
    report.success = true;
  })()]);
} catch (error) {
  report.success = false; report.errors.push(error.stack ?? String(error)); process.exitCode = 1;
} finally {
  clearTimeout(timer); clearInterval(sampling); canceled = true; phase = 'cleanup';
  await samplePending;
  for (const job of jobs) if (job.pid && !job.bot.finished) { try { process.kill(job.pid, 'SIGTERM'); } catch {} }
  for (const child of children) if (!child.finished) child.kill('SIGTERM');
  await Promise.race([Promise.allSettled(children.map(child => child.completion)), delay(2000)]);
  for (const job of jobs) if (job.pid && !job.bot.finished) { try { process.kill(job.pid, 'SIGKILL'); } catch {} }
  for (const child of children) if (!child.finished) child.kill('SIGKILL');
  await Promise.race([Promise.allSettled(children.map(child => child.completion)), delay(2000)]);
  if (children.some(child => !child.finished)) {
    report.success = false; report.errors.push('child process cleanup did not complete'); process.exitCode = 1;
  }
  report.finished_at = new Date().toISOString();
  const reportPath = resolve(root, `.runtime/measure-${count}.json`);
  await mkdir(dirname(reportPath), { recursive: true });
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ event: 'measurement_complete', success: report.success, bots: count,
    sampled_simultaneous_peak_rss_bytes: report.sampled_simultaneous_peak_rss_bytes,
    sum_of_individual_os_peaks_bytes: report.sum_of_individual_os_peaks_bytes,
    report: reportPath, errors: report.errors }));
}
