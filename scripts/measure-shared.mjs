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
const radius = Number(process.env.NBMCBOT_CHUNK_RADIUS ?? 2);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const binary = resolve(root, 'target/release/nbmcbot');
const fixtureBinary = resolve(root, 'target/release/examples/fixture_server');
const report = {
  started_at: new Date().toISOString(), platform: `${os.platform()} ${os.release()} ${os.arch()}`,
  minecraft: '1.21.11', protocol: 774, bot_count: count, soak_seconds: soakSeconds,
  chunk_radius: radius, expected_chunks_per_bot: (radius * 2 + 1) ** 2,
  workload: 'same-server flat chunks, chat, goto, 32 plugin load/unload cycles, one active plugin per account during soak',
  measurement: '100 ms ps RSS samples; independent-process RSS sums count shared OS pages per process, not unique physical RAM',
  excluded: 'fixture server and Node/time/ps measurement tools', runs: [], errors: [],
};

async function measure(mode) {
  const started = Date.now(), children = [], jobs = [], discoveries = [];
  const result = { mode, started_at: new Date().toISOString(), processes: [], samples: [], fixture_events: [] };
  report.runs.push(result);
  let canceled = false, phase = 'startup', timer, sampling, busy = false, pendingSample = Promise.resolve();
  let fail;
  const fatal = new Promise((_, reject) => { fail = reject; });
  fatal.catch(() => {});
  const check = () => { if (canceled) throw new Error('measurement canceled'); };

  function launch(path, args) {
    check();
    const child = spawn(path, args, { cwd: root, stdio: ['pipe', 'pipe', 'pipe'] });
    children.push(child);
    child.completion = new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', (code, signal) => { child.finished = true; resolve({ code, signal }); });
    });
    child.completion.catch(fail);
    child.stdin.on('error', error => { if (!canceled) fail(error); });
    return child;
  }

  function watch(job) {
    const waiting = new Set();
    createInterface({ input: job.child.stdout }).on('line', line => {
      let event;
      try { event = JSON.parse(line); } catch { fail(new Error(`non-JSON bot output: ${line}`)); return; }
      job.record.events.push({ elapsed_ms: Date.now() - started, ...event });
      if (event.event === 'spawn') job.online.add(event.bot ?? job.names[0]);
      if (['error', 'command_error', 'task_error', 'plugin_error', 'connection_failed', 'bot_failed', 'disconnected'].includes(event.event) && !job.closing) {
        fail(new Error(JSON.stringify(event)));
      }
      for (const waiter of waiting) {
        if (waiter.matches(event)) { waiting.delete(waiter); waiter.resolve(event); }
      }
    });
    job.child.completion.then(exit => {
      for (const waiter of waiting) waiter.reject(new Error(`process exited while waiting: ${JSON.stringify(exit)}`));
      waiting.clear();
      if (!job.closing) fail(new Error('bot process exited early'));
    }).catch(fail);
    job.wait = matches => {
      const promise = new Promise((resolve, reject) => {
        if (job.child.finished) reject(new Error('bot process already exited'));
        else waiting.add({ matches, resolve, reject });
      });
      promise.catch(() => {});
      return promise;
    };
  }

  function send(job, command, username) {
    check();
    const prefix = mode === 'shared' && username ? `@${username} ` : '';
    return new Promise((resolve, reject) => job.child.stdin.write(`${prefix}${command}\n`, error => error ? reject(error) : resolve()));
  }
  const forBot = (job, name, type) => job.wait(event => event.event === type && (event.bot === name || (mode === 'independent' && !event.bot)));

  async function discover(job) {
    for (let n = 0; n < 100; n++) {
      check();
      let output;
      try { output = await exec('pgrep', ['-P', String(job.child.pid)], { timeout: 2000 }); } catch { await delay(20); continue; }
      const ids = output.stdout.trim().split(/\s+/).map(Number);
      if (ids.length !== 1 || !Number.isInteger(ids[0]) || ids[0] < 1) throw new Error('ambiguous time child');
      const { stdout } = await exec('ps', ['-o', 'pid=,ppid=,comm=', '-p', String(ids[0])], { timeout: 2000 });
      const row = stdout.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
      if (!row || Number(row[1]) !== ids[0] || Number(row[2]) !== job.child.pid || basename(row[3].trim()) !== 'nbmcbot') throw new Error('bot PID identity mismatch');
      job.pid = ids[0]; job.record.pid = ids[0]; return;
    }
    throw new Error('could not identify bot PID');
  }

  async function sample() {
    const active = jobs.filter(job => job.pid && !job.child.finished);
    if (!active.length) return;
    const ids = active.map(job => job.pid), rss = {};
    if (new Set(ids).size !== ids.length) throw new Error('duplicate bot PID');
    const { stdout } = await exec('ps', ['-o', 'pid=,rss=,comm=', '-p', ids.join(',')], { timeout: 2000 });
    for (const line of stdout.trim().split('\n')) {
      const row = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
      if (!row || !ids.includes(Number(row[1])) || basename(row[3].trim()) !== 'nbmcbot') throw new Error(`invalid RSS row: ${line}`);
      rss[row[1]] = Number(row[2]) * 1024;
    }
    if (Object.keys(rss).length !== ids.length) throw new Error('lost a bot process during RSS sampling');
    const online = active.reduce((sum, job) => sum + job.online.size, 0);
    result.samples.push({ elapsed_ms: Date.now() - started, phase, online_count: online,
      complete: ids.length === (mode === 'shared' ? 1 : count) && online === count,
      rss_by_pid: rss, total_rss_bytes: Object.values(rss).reduce((a, b) => a + b, 0) });
  }

  async function status(job, name, requireChunks = true) {
    const pending = forBot(job, name, 'status'); await send(job, 'status', name);
    const event = await pending;
    if (event.connected !== true) throw new Error(`${name} is not connected`);
    if (requireChunks && event.loaded_chunks !== report.expected_chunks_per_bot) throw new Error(`${name} has ${event.loaded_chunks} loaded chunks; expected ${report.expected_chunks_per_bot}`);
    return event;
  }

  async function waitForWorld(job, name) {
    const deadline = Date.now() + 30000;
    while (Date.now() < deadline) {
      const event = await status(job, name, false);
      // Azalea can emit Spawn as soon as chunks exist, before the initial teleport.
      if (event.loaded_chunks === report.expected_chunks_per_bot && event.health === 20
          && event.position?.map(Math.floor).join(',') === '4,64,4') return;
      await delay(50); check();
    }
    throw new Error(`${name} did not load the full fixture world`);
  }

  async function sharedStatus() {
    const job = jobs[0], pending = job.wait(event => event.event === 'swarm_status');
    await send(job, 'status');
    const event = await pending;
    if (event.connected !== count || event.bots !== count || event.unique_worlds !== 1
        || event.unique_chunks !== report.expected_chunks_per_bot
        || event.chunk_references !== count * report.expected_chunks_per_bot) {
      throw new Error(`sharing evidence mismatch: ${JSON.stringify(event)}`);
    }
    return event;
  }

  async function work(job, name) {
    const arrived = job.wait(event => event.event === 'task_finished' && event.task === 'goto' && (event.bot === name || (mode === 'independent' && !event.bot)));
    await send(job, `say shared-measure-${name}`, name);
    await send(job, 'goto 7 64 4', name);
    for (let n = 0; n < 32; n++) {
      const loaded = forBot(job, name, 'plugin_loaded');
      await send(job, 'plugin load examples/plugins/anti-idle.wat', name); const event = await loaded;
      const unloaded = forBot(job, name, 'plugin_unloaded');
      await send(job, `plugin unload ${event.id}`, name); await unloaded;
    }
    await arrived;
    const loaded = forBot(job, name, 'plugin_loaded');
    await send(job, 'plugin load examples/plugins/anti-idle.wat', name); await loaded;
    const event = await status(job, name);
    if (!event.position || event.position.map(Math.floor).join(',') !== '7,64,4') throw new Error(`${name} did not reach target`);
    if (!result.fixture_events.some(line => line.includes(`chat shared-measure-${name}`))) throw new Error(`${name} chat missing at server`);
  }

  try {
    timer = setTimeout(() => fail(new Error(`${mode} measurement timed out`)), (soakSeconds + 120) * 1000);
    await Promise.race([fatal, (async () => {
      const fixture = launch(fixtureBinary, ['--clients', String(count), '--chunk-radius', String(radius)]);
      createInterface({ input: fixture.stderr }).on('line', line => result.fixture_events.push(line));
      const address = await new Promise((resolve, reject) => {
        const lines = createInterface({ input: fixture.stdout });
        lines.once('line', line => { lines.close(); /^127\.0\.0\.1:\d+$/.test(line) ? resolve(line) : reject(new Error(`invalid fixture address ${line}`)); });
        fixture.completion.then(() => reject(new Error('fixture exited before address'))).catch(reject);
      });
      result.server = address;
      const names = Array.from({ length: count }, (_, n) => `SharedBot${String(n + 1).padStart(2, '0')}`);
      const groups = mode === 'shared' ? [names] : names.map(name => [name]);
      const ready = groups.map(names => {
        const args = mode === 'shared' ? ['swarm', '--usernames', names.join(',')] : ['run', '--username', names[0]];
        const record = { usernames: names, events: [], stderr: '' }; result.processes.push(record);
        const child = launch('/usr/bin/time', [os.platform() === 'darwin' ? '-l' : '-v', binary, ...args, '--server', address, '--auth', 'offline', '--no-reconnect']);
        const job = { names, child, record, online: new Set(), closing: false }; jobs.push(job); watch(job);
        child.stderr.on('data', chunk => { record.stderr += chunk; if (record.stderr.length > 1048576) fail(new Error('excessive stderr')); });
        const discovery = discover(job); discoveries.push(discovery);
        return Promise.all([...names.map(name => forBot(job, name, 'spawn')), discovery]);
      });
      sampling = setInterval(() => {
        if (busy || canceled) return;
        busy = true; pendingSample = sample().catch(fail).finally(() => { busy = false; });
      }, 100);
      await Promise.all(ready);
      const accounts = jobs.flatMap(job => job.names.map(name => ({ job, name })));
      await Promise.all(accounts.map(({ job, name }) => waitForWorld(job, name)));
      phase = 'workload';
      await Promise.all(accounts.map(({ job, name }) => work(job, name)));
      if (mode === 'shared') result.sharing_before_soak = await sharedStatus();
      phase = 'soak'; result.all_ready_ms = Date.now() - started;
      console.log(JSON.stringify({ event: 'all_bots_ready', mode, bots: count, soak_seconds: soakSeconds }));
      for (let n = 0; n < soakSeconds; n++) {
        await delay(1000); check();
        await Promise.all(accounts.map(({ job, name }) => status(job, name)));
      }
      if (mode === 'shared') result.sharing_after_soak = await sharedStatus();
      result.last_statuses = await Promise.all(accounts.map(({ job, name }) => status(job, name)));
      clearInterval(sampling); await pendingSample; await sample(); phase = 'closing';
      await Promise.all(accounts.map(async ({ job, name }) => {
        const unloaded = forBot(job, name, 'plugin_unloaded'); await send(job, 'plugin unload anti-idle', name); await unloaded;
      }));
      await Promise.all(jobs.map(async job => {
        job.closing = true;
        const stopped = job.wait(event => event.event === 'stopped'); await send(job, 'quit'); await stopped;
        job.record.exit = await job.child.completion;
        if (job.record.exit.code !== 0 || /panicked at|Encountered a panic/.test(job.record.stderr)) throw new Error('bot shutdown failed');
        const peak = os.platform() === 'darwin' ? job.record.stderr.match(/(\d+)\s+maximum resident set size/) : job.record.stderr.match(/Maximum resident set size \(kbytes\):\s*(\d+)/);
        if (!peak) throw new Error('OS peak RSS missing');
        job.record.os_peak_rss_bytes = Number(peak[1]) * (os.platform() === 'darwin' ? 1 : 1024);
        if (job.record.os_peak_rss_bytes > 256000000) throw new Error('a bot process exceeded 256 MB');
      }));
      const fixtureExit = await fixture.completion;
      if (fixtureExit.code !== 0) throw new Error(`fixture failed: ${JSON.stringify(fixtureExit)}`);
      if (result.fixture_events.filter(line => line.includes('handshake protocol=774')).length !== count) throw new Error('missing protocol handshakes');
      const complete = result.samples.filter(sample => sample.complete), soak = complete.filter(sample => sample.phase === 'soak');
      if (soak.length < soakSeconds * 5) throw new Error('insufficient complete soak samples');
      result.sampled_peak_rss_bytes = complete.reduce((peak, sample) => Math.max(peak, sample.total_rss_bytes), 0);
      result.mean_soak_rss_bytes = Math.round(soak.reduce((sum, sample) => sum + sample.total_rss_bytes, 0) / soak.length);
      result.sum_of_process_os_peaks_bytes = jobs.reduce((sum, job) => sum + job.record.os_peak_rss_bytes, 0);
      result.success = true;
    })()]);
  } finally {
    canceled = true; clearTimeout(timer); clearInterval(sampling); await pendingSample;
    // Finish in-flight identity checks before killing wrappers, so no known child is orphaned.
    await Promise.allSettled(discoveries);
    for (const job of jobs) if (job.pid && !job.child.finished) { try { process.kill(job.pid, 'SIGTERM'); } catch {} }
    for (const child of children) if (!child.finished) child.kill('SIGTERM');
    await Promise.race([Promise.allSettled(children.map(child => child.completion)), delay(2000)]);
    for (const job of jobs) if (job.pid && !job.child.finished) { try { process.kill(job.pid, 'SIGKILL'); } catch {} }
    for (const child of children) if (!child.finished) child.kill('SIGKILL');
    await Promise.race([Promise.allSettled(children.map(child => child.completion)), delay(2000)]);
    if (children.some(child => !child.finished)) throw new Error('child cleanup incomplete');
    result.finished_at = new Date().toISOString();
  }
}

try {
  if (!['darwin', 'linux'].includes(os.platform())) throw new Error('macOS or Linux required');
  if (!Number.isInteger(count) || count < 1 || count > 32) throw new Error('NBMCBOT_COUNT must be 1..32');
  if (!Number.isInteger(soakSeconds) || soakSeconds < 1 || soakSeconds > 3600) throw new Error('NBMCBOT_SOAK_SECONDS must be 1..3600');
  if (!Number.isInteger(radius) || radius < 0 || radius > 2) throw new Error('NBMCBOT_CHUNK_RADIUS must be 0..2 for view_distance=2');
  await Promise.all([access(binary), access(fixtureBinary), access('/usr/bin/time')]);
  report.binary_sha256 = createHash('sha256').update(await readFile(binary)).digest('hex');
  await measure('independent');
  await measure('shared');
  report.success = true;
  report.mean_rss_reduction_percent = 100 * (1 - report.runs[1].mean_soak_rss_bytes / report.runs[0].mean_soak_rss_bytes);
} catch (error) {
  report.success = false; report.errors.push(error.stack ?? String(error)); process.exitCode = 1;
} finally {
  report.finished_at = new Date().toISOString();
  const reportPath = resolve(root, `.runtime/measure-shared-${count}.json`);
  await mkdir(dirname(reportPath), { recursive: true });
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ event: 'comparison_complete', success: report.success,
    runs: report.runs.map(({ mode, mean_soak_rss_bytes, sampled_peak_rss_bytes, sum_of_process_os_peaks_bytes }) => ({ mode, mean_soak_rss_bytes, sampled_peak_rss_bytes, sum_of_process_os_peaks_bytes })),
    mean_rss_reduction_percent: report.mean_rss_reduction_percent, report: reportPath, errors: report.errors }));
}
