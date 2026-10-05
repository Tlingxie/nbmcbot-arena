#!/usr/bin/env node
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createInterface } from 'node:readline';
import { mkdir, writeFile, access } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import os from 'node:os';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const exec = promisify(execFile);
const soakSeconds = Number(process.env.NBMCBOT_SOAK_SECONDS ?? 60);
const report = { started_at: new Date().toISOString(), platform: `${os.platform()} ${os.release()} ${os.arch()}`, minecraft: '1.21.11', protocol: 774, fixture_is_required_helper: false, events: [], fixture_events: [], rss_samples: [], errors: [] };
const children = [];
const timeoutMs = 120000 + soakSeconds * 1000;
let timer, sampling, botPid, botProcess;

function launch(binary, args) {
  const child = spawn(binary, args, { cwd: root, stdio: ['pipe', 'pipe', 'pipe'] });
  children.push(child);
  child.completion = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => { child.finished = true; resolve({ code, signal }); });
  });
  child.completion.catch(() => {});
  return child;
}
function streamLines(stream, consume) {
  const lines = createInterface({ input: stream });
  lines.on('line', consume);
}
function monitor(child, consume) {
  const pending = new Set();
  streamLines(child.stdout, line => {
    let event;
    try { event = JSON.parse(line); } catch { report.errors.push(`non-JSON bot output: ${line}`); return; }
    consume(event);
    for (const waiter of pending) if (waiter.match(event)) { pending.delete(waiter); waiter.resolve(event); }
  });
  child.completion.then(exit => { for (const waiter of pending) waiter.reject(new Error(`bot exited while waiting: ${JSON.stringify(exit)}`)); pending.clear(); });
  return match => {
    const result = new Promise((resolve, reject) => {
    if (child.finished) { reject(new Error('bot already exited')); return; }
    pending.add({ match, resolve, reject });
    });
    result.catch(() => {});
    return result;
  };
}
function send(child, text) {
  return new Promise((resolve, reject) => child.stdin.write(`${text}\n`, error => error ? reject(error) : resolve()));
}
async function sampleRss(wrapperPid) {
  try {
    if (!botPid) {
      const { stdout } = await exec('pgrep', ['-P', String(wrapperPid)]);
      botPid = Number(stdout.trim().split(/\s+/)[0]);
    }
    const { stdout } = await exec('ps', ['-o', 'rss=', '-p', String(botPid)]);
    const rss = Number(stdout.trim());
    if (Number.isFinite(rss) && rss > 0) report.rss_samples.push({ elapsed_ms: Date.now() - report.start_ms, rss_bytes: rss * 1024 });
  } catch { /* The process may exit between discovery and sampling. */ }
}

try {
  if (!Number.isInteger(soakSeconds) || soakSeconds < 0 || soakSeconds > 86400) throw new Error('NBMCBOT_SOAK_SECONDS must be an integer in 0..86400');
  report.soak_seconds = soakSeconds;
  if (!['darwin', 'linux'].includes(os.platform())) throw new Error('measurement currently supports macOS and Linux');
  const binary = resolve(root, 'target/release/nbmcbot');
  const fixtureBinary = resolve(root, 'target/release/examples/fixture_server');
  await Promise.all([access(binary), access(fixtureBinary), access('/usr/bin/time')]);
  const deadline = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`measurement exceeded ${timeoutMs}ms`)), timeoutMs); });
  await Promise.race([deadline, (async () => {
    const fixture = launch(fixtureBinary, []);
    const address = await new Promise((resolve, reject) => {
      const lines = createInterface({ input: fixture.stdout });
      lines.once('line', line => { lines.close(); /^127\.0\.0\.1:\d+$/.test(line) ? resolve(line) : reject(new Error(`invalid fixture address: ${line}`)); });
      fixture.completion.then(() => reject(new Error('fixture exited before address')));
    });
    report.server = address;
    streamLines(fixture.stderr, line => report.fixture_events.push(line));
    report.start_ms = Date.now();
    const bot = launch('/usr/bin/time', [os.platform() === 'darwin' ? '-l' : '-v', binary, 'run', '--server', address, '--username', 'MeasureBot', '--auth', 'offline', '--no-reconnect']);
    botProcess = bot;
    report.command = `${binary} run --server ${address} --username MeasureBot --auth offline --no-reconnect`;
    const wait = monitor(bot, event => report.events.push({ elapsed_ms: Date.now() - report.start_ms, ...event }));
    let stderr = '';
    bot.stderr.on('data', chunk => { stderr += chunk.toString(); if (stderr.length > 1048576) stderr = stderr.slice(-1048576); });
    sampling = setInterval(() => { void sampleRss(bot.pid); }, 100);
    await wait(event => event.event === 'spawn');
    const status = wait(event => event.event === 'status'); await send(bot, 'status'); await status;
    const navigation = wait(event => event.event === 'task_finished' && event.task === 'goto');
    const chat = wait(event => event.event === 'chat_sent');
    await send(bot, 'say measurement-hello'); await send(bot, 'goto 7 64 4'); await chat;
    const pluginPath = 'examples/plugins/anti-idle.wat';
    for (let cycle = 0; cycle < 32; cycle++) {
      const loaded = wait(event => event.event === 'plugin_loaded'); await send(bot, `plugin load ${pluginPath}`); const info = await loaded;
      const pluginStatus = wait(event => event.event === 'status'); await send(bot, 'status'); await pluginStatus;
      const unloaded = wait(event => event.event === 'plugin_unloaded' && event.id === info.id); await send(bot, `plugin unload ${info.id}`); await unloaded;
    }
    await navigation;
    const activePlugin = wait(event => event.event === 'plugin_loaded'); await send(bot, `plugin load ${pluginPath}`); const activeInfo = await activePlugin;
    for (let second = 0; second < soakSeconds; second++) {
      await new Promise(resolve => setTimeout(resolve, 1000));
      const idleStatus = wait(event => event.event === 'status'); await send(bot, 'status'); await idleStatus;
    }
    const idleUnloaded = wait(event => event.event === 'plugin_unloaded' && event.id === activeInfo.id); await send(bot, `plugin unload ${activeInfo.id}`); await idleUnloaded;
    const finalStatus = wait(event => event.event === 'status'); await send(bot, 'status'); await finalStatus;
    const stopped = wait(event => event.event === 'stopped'); await send(bot, 'quit'); await stopped;
    const exit = await bot.completion;
    report.exit = exit;
    if (exit.code !== 0) throw new Error(`bot failed: ${JSON.stringify(exit)}`);
    await fixture.completion;
    report.bot_stderr = stderr;
    if (/panicked at|Encountered a panic/.test(stderr)) throw new Error('bot reported a runtime panic');
    const peak = os.platform() === 'darwin' ? stderr.match(/(\d+)\s+maximum resident set size/) : stderr.match(/Maximum resident set size \(kbytes\):\s*(\d+)/);
    if (!peak) throw new Error('OS peak RSS was not reported by /usr/bin/time');
    report.os_peak_rss_bytes = Number(peak[1]) * (os.platform() === 'darwin' ? 1 : 1024);
    report.sampled_peak_rss_bytes = Math.max(0, ...report.rss_samples.map(sample => sample.rss_bytes));
    report.memory_limit_bytes = 256000000;
    report.within_memory_target = report.os_peak_rss_bytes <= report.memory_limit_bytes;
    if (!report.fixture_events.includes('chat measurement-hello')) throw new Error('fixture did not observe actual chat');
    if (!report.fixture_events.includes('handshake protocol=774')) throw new Error('fixture did not observe protocol 774');
    if (report.events.some(event => ['command_error', 'task_error', 'plugin_error', 'connection_failed'].includes(event.event))) throw new Error('runtime emitted an error event');
    if (!report.within_memory_target) throw new Error('bot OS peak RSS exceeded target');
    report.success = true;
  })()]);
} catch (error) {
  report.success = false; report.errors.push(error.stack ?? String(error)); process.exitCode = 1;
} finally {
  clearTimeout(timer); clearInterval(sampling);
  if (botPid && !botProcess?.finished) { try { process.kill(botPid, 'SIGTERM'); } catch {} }
  for (const child of children) if (!child.finished) child.kill('SIGTERM');
  delete report.start_ms;
  report.finished_at = new Date().toISOString();
  await mkdir(resolve(root, '.runtime'), { recursive: true });
  await writeFile(resolve(root, '.runtime/measure.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ event: 'measurement_complete', success: report.success, os_peak_rss_bytes: report.os_peak_rss_bytes, report: '.runtime/measure.json', errors: report.errors }));
}
