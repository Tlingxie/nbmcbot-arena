#!/usr/bin/env node
import { spawn, execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, openSync, closeSync, readFileSync, writeFileSync, statSync, constants } from 'node:fs';
import { dirname, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const [kind, action, ...command] = process.argv.slice(2);
if (!['server', 'bot', 'probe', 'swarm', 'swarm-a', 'swarm-b', 'mace-team', 'spear-team'].includes(kind) || !['start', 'command', 'status'].includes(action)) {
  throw new Error('usage: node scripts/arena-process.mjs server|bot|probe|swarm|swarm-a|swarm-b|mace-team|spear-team start|status|command [words...]');
}
const arena = resolve(root, '.runtime/vanilla-1.21.11');
mkdirSync(arena, { recursive: true });
const recordPath = resolve(arena, `${kind}-process.json`);
const pipePath = resolve(arena, `${kind}-stdin.pipe`);
const logPath = resolve(arena, `${kind}-console.log`);
const existing = existsSync(recordPath) ? JSON.parse(readFileSync(recordPath, 'utf8')) : null;
function alive(record) {
  if (!record) return false;
  try {
    const command = execFileSync('ps', ['-o', 'comm=', '-p', String(record.pid)], { encoding: 'utf8' }).trim();
    return basename(command) === (kind === 'server' ? 'java' : 'nbmcbot');
  } catch { return false; }
}

if (action === 'status') {
  console.log(JSON.stringify({ kind, running: alive(existing), ...existing }));
} else if (action === 'command') {
  if (!alive(existing)) throw new Error(`${kind} is not running`);
  if (!command.length || command.join(' ').includes('\n')) throw new Error('one command is required');
  const pipe = openSync(pipePath, constants.O_WRONLY | constants.O_NONBLOCK);
  writeFileSync(pipe, `${command.join(' ')}\n`);
  closeSync(pipe);
} else {
  if (alive(existing)) throw new Error(`${kind} is already running with PID ${existing.pid}`);
  if (!existsSync(pipePath)) execFileSync('mkfifo', ['-m', '600', pipePath]);
  // Both ends stay open in the child, so stdin does not reach EOF between commands.
  const input = openSync(pipePath, constants.O_RDWR);
  const output = openSync(logPath, 'a', 0o600);
  const logOffset = statSync(logPath).size;
  const javaHome = process.env.JAVA_HOME || (process.platform === 'darwin'
    ? execFileSync('/usr/libexec/java_home', ['-v', '21'], { encoding: 'utf8' }).trim() : null);
  const binary = kind === 'server' ? (javaHome ? resolve(javaHome, 'bin/java') : 'java') : resolve(root, 'target/release/nbmcbot');
  const isDuel = kind === 'mace-team' || kind === 'spear-team';
  const duelCount = Number(process.env.NBMCBOT_DUEL_COUNT ?? 5);
  if (isDuel && (!Number.isInteger(duelCount) || duelCount < 1 || duelCount > 50)) {
    throw new Error('NBMCBOT_DUEL_COUNT must be an integer from 1 to 50');
  }
  const isSwarm = kind.startsWith('swarm') || isDuel;
  const count = isDuel ? duelCount : kind === 'swarm' ? 100 : 50;
  const prefix = isDuel ? (kind === 'mace-team' ? 'Mace' : 'Spear') : 'Fist';
  const names = Array.from({ length: count }, (_, index) => `${prefix}${String(index + (kind === 'swarm-b' ? 51 : 1)).padStart(3, '0')}`);
  const startupConfig = resolve(arena, `${kind}-startup.toml`);
  if (isDuel) writeFileSync(startupConfig, 'modules = ["auto-respawn"]\n', { mode: 0o600 });
  const args = kind === 'server'
    ? ['-Xms512M', '-Xmx2G', '-XX:MaxDirectMemorySize=2G', '-XX:+UseG1GC', '-jar', 'server.jar', 'nogui']
    : [...(isDuel ? ['--config', startupConfig] : []),
      ...(isSwarm ? ['swarm', '--usernames', names.join(',')]
      : ['run', '--username', kind === 'bot' ? 'FistBot' : 'ArenaProbe']),
      '--server', '127.0.0.1:25566', '--auth', 'offline', ...(isSwarm ? [] : ['--no-reconnect'])];
  const child = spawn(binary, args, { cwd: kind === 'server' ? arena : root, detached: true, stdio: [input, output, output] });
  await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
  const record = { pid: child.pid, kind, started_at: new Date().toISOString(), binary, log: logPath, log_offset: logOffset, pipe: pipePath,
    ...(isSwarm ? { usernames: names } : {}) };
  writeFileSync(recordPath, `${JSON.stringify(record, null, 2)}\n`);
  closeSync(input); closeSync(output); child.unref();
  console.log(JSON.stringify(record));
}
