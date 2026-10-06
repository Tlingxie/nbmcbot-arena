import { constants, openSync, closeSync, writeSync, readSync, statSync, readFileSync } from 'node:fs';
import { execFile, execFileSync } from 'node:child_process';
import { promisify } from 'node:util';
import { resolve, basename } from 'node:path';
import { validName } from './state.mjs';
import { desiredFleets, reconcileFleets } from './fleets.mjs';

const run = promisify(execFile);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
export const validMode = mode => ['mace-vs-player', 'mace-vs-mace', 'mace-vs-spear'].includes(mode);
function failure(message, statusCode = 400) { return Object.assign(new Error(message), { statusCode }); }

export class Arena {
  constructor({ root, state }) {
    this.root = root;
    this.state = state;
    this.directory = resolve(root, '.runtime/vanilla-1.21.11');
    this.tails = new Map();
    this.humans = new Set();
    this.cached = new Map();
    this.lastList = 0;
    this.lastSample = 0;
    this.closed = false;
  }
  record(kind) {
    const old = this.cached.get(kind);
    if (old && Date.now() - old.checked < 2000) return old.record;
    const record = JSON.parse(readFileSync(resolve(this.directory, `${kind}-process.json`), 'utf8'));
    if (!Number.isSafeInteger(record.pid) || record.pid < 2) throw failure(`${kind} is unavailable`, 503);
    const command = execFileSync('ps', ['-o', 'comm=', '-p', String(record.pid)], { encoding: 'utf8', timeout: 1000 }).trim();
    if (basename(command) !== (kind === 'server' ? 'java' : 'nbmcbot')) throw failure(`${kind} is not running`, 503);
    this.cached.set(kind, { record, checked: Date.now() });
    return record;
  }
  command(kind, command) {
    if (!['server', 'mace-team', 'spear-team'].includes(kind) || /[\r\n]/.test(command)) throw failure('Invalid command');
    this.record(kind);
    const fd = openSync(resolve(this.directory, `${kind}-stdin.pipe`), constants.O_WRONLY | constants.O_NONBLOCK);
    try { writeSync(fd, `${command}\n`); } finally { closeSync(fd); }
  }
  tail(kind) {
    const path = resolve(this.directory, `${kind}-console.log`);
    try {
      const stat = statSync(path);
      let cursor = this.tails.get(kind);
      if (!cursor) { this.tails.set(kind, { offset: stat.size, ino: stat.ino, pending: '' }); return; }
      if (cursor.ino !== stat.ino || stat.size < cursor.offset) cursor = { offset: 0, ino: stat.ino, pending: '' };
      const size = Math.min(stat.size - cursor.offset, 262144);
      if (size) {
        const fd = openSync(path, 'r');
        const data = Buffer.alloc(size);
        let read;
        try { read = readSync(fd, data, 0, size, cursor.offset); } finally { closeSync(fd); }
        cursor.offset += read;
        const lines = (cursor.pending + data.subarray(0, read).toString('utf8')).split('\n');
        cursor.pending = lines.pop().slice(-65536);
        for (const line of lines) this.line(kind, line);
      }
      this.tails.set(kind, cursor);
    } catch (error) { if (error.code !== 'ENOENT') this.lastError = error.message; }
  }
  line(kind, line) {
    if (kind !== 'server') {
      if (!line.startsWith('{')) return;
      try {
        const event = JSON.parse(line);
        if (['duel_attack', 'duel_target', 'duel_finished'].includes(event.event)) {
          this.state.event(event.event, `${event.bot ?? ''} ${event.event} ${event.target ?? ''}`, { bot: event.bot, target: event.target });
        }
      } catch {}
      return;
    }
    const message = line.replace(/^.*?\]:\s*/, '');
    const list = message.match(/^There are \d+ of a max of \d+ players online:\s*(.*)$/);
    if (list) {
      const online = new Set(list[1].split(', ').filter(validName));
      for (const name of this.humans) if (!online.has(name)) this.state.observe(name, { connected: false });
      this.humans = new Set([...online].filter(name => !/^(Mace|Spear)\d{3}$/.test(name)));
    }
    const data = message.match(/^([A-Za-z0-9_]{1,16}) has the following entity data: (.+)$/);
    if (data && this.humans.has(data[1])) {
      const raw = data[2];
      if (/^"minecraft:[a-z_]+"$/.test(raw)) this.state.observe(data[1], { dimension: raw.slice(1, -1) });
      else if (raw.startsWith('[')) {
        const numbers = raw.slice(1, -1).split(',').map(n => Number(n.trim().replace(/[df]$/, '')));
        if (numbers.every(Number.isFinite)) {
          if (numbers.length === 3) this.state.observe(data[1], { position: numbers });
          if (numbers.length === 2) this.state.observe(data[1], { yaw: numbers[0], pitch: numbers[1] });
        }
      } else if (/^[-\d.]+f$/.test(raw)) {
        const health = Number(raw.slice(0, -1));
        if (Number.isFinite(health)) this.state.observe(data[1], { health, alive: health > 0 });
      }
    }
    if (/^[A-Za-z0-9_]{1,16} (?:was (?:smashed|slain|killed)|fell|hit the ground|died|joined the game|left the game)/.test(message)) {
      this.state.event('server', message);
    }
  }
  tick() {
    for (const kind of ['server', 'mace-team', 'spear-team']) this.tail(kind);
    const now = Date.now();
    try {
      if (now - this.lastList >= 3000) {
        this.command('server', 'list');
        for (const name of [...this.humans].slice(0, 32)) this.command('server', `data get entity ${name} Dimension`);
        this.lastList = now;
      }
      if (now - this.lastSample >= 250) {
        for (const name of [...this.humans].slice(0, 32)) {
          this.command('server', `data get entity ${name} Pos`);
          this.command('server', `data get entity ${name} Rotation`);
          this.command('server', `data get entity ${name} Health`);
        }
        this.lastSample = now;
      }
    } catch (error) { this.lastError = error.message; }
  }
  async spectate({ viewer, target }) {
    const players = this.state.players();
    if (!validName(viewer) || (target !== null && !validName(target)) || viewer === target) throw failure('Invalid observer or target');
    if (!this.humans.has(viewer) || !players.some(p => p.name === viewer && p.connected)) throw failure('Observer account must be an online human player', 409);
    if (target !== null && !players.some(p => p.name === target && p.connected && !p.stale && p.alive !== false)) throw failure('Target is unavailable or dead', 409);
    this.command('server', `gamemode spectator ${viewer}`);
    this.command('server', target === null ? `execute as ${viewer} run spectate` : `spectate ${target} ${viewer}`);
    this.state.observer = { viewer, target };
    this.state.event('camera', target ? `${viewer} → ${target}` : `${viewer}: free camera`);
    return this.state.observer;
  }
  async startRound({ mode, countdown = 10, botCount = 10, player = null }) {
    if (!validMode(mode) || !Number.isInteger(countdown) || countdown < 0 || countdown > 30) throw failure('Invalid round settings');
    desiredFleets(botCount);
    if (mode !== 'mace-vs-player' && botCount < 2) throw failure('两队对战至少需要 2 个机器人');
    if (mode === 'mace-vs-player') this.requireParticipant(player);
    else player = null;
    if (['preparing', 'countdown'].includes(this.state.round.state)) throw failure('A round is already being prepared', 409);
    this.record('server');
    this.state.round = { state: 'preparing', mode, countdown, botCount, player, error: null };
    this.roundPromise = this.prepareRound({ mode, countdown, botCount, player });
    return this.state.round;
  }
  requireParticipant(name) {
    if (!validName(name) || /^(Mace|Spear)\d{3}$/.test(name)) throw failure('请选择有效的真人玩家');
    const player = this.state.players().find(p => p.name === name);
    if (!this.humans.has(name) || !player?.connected || player.stale) throw failure('目标玩家需要在线且位置数据有效', 409);
    if (player.alive === false) throw failure('请先在 Minecraft 中复活，再开启围攻', 409);
  }
  async fleetControl(kind, action, command, extraEnv = {}) {
    const result = await run(process.execPath, [resolve(this.root, 'scripts/arena-process.mjs'), kind, action, ...(command ? [command] : [])], {
      cwd: this.root, env: { ...process.env, ...extraEnv }, timeout: 10000, maxBuffer: 1024 * 1024,
    });
    this.cached.delete(kind);
    return result.stdout.trim() ? JSON.parse(result.stdout) : null;
  }
  async prepareRound({ mode, countdown, botCount, player }) {
    const settings = { mode, botCount, player };
    const env = { ...process.env, NBMCBOT_DUEL_MODE: mode, NBMCBOT_DUEL_TOTAL: String(botCount), NBMCBOT_DUEL_PLAYER: player ?? '' };
    try {
      await reconcileFleets(botCount, {
        control: (...args) => this.fleetControl(...args),
        telemetryAddress: `127.0.0.1:${process.env.NBMCBOT_TELEMETRY_PORT ?? 4211}`,
        cancelled: () => this.closed,
      });
      for (const action of ['setup', 'resupply']) {
        if (this.closed) return;
        if (player) this.requireParticipant(player);
        await run(process.execPath, [resolve(this.root, 'scripts/duel-arena.mjs'), action], {
          cwd: this.root, env, timeout: 180000, maxBuffer: 4 * 1024 * 1024,
        });
      }
      if (player) this.state.observer = { viewer: player, target: null };
      this.command('server', 'effect give @a[tag=nbmc_duel] minecraft:instant_health 1 5 true');
      if (player) this.command('server', `effect give ${player} minecraft:instant_health 1 5 true`);
      this.state.round.state = 'countdown';
      for (let remaining = countdown; remaining > 0; remaining--) {
        if (this.closed) return;
        this.state.round.countdown = remaining;
        if (player) {
          this.requireParticipant(player);
          this.command('server', `title ${player} title ${JSON.stringify({ text: String(remaining), color: 'gold' })}`);
        }
        await sleep(1000);
      }
      if (this.closed) return;
      if (player) {
        this.requireParticipant(player);
        this.command('server', `title ${player} title ${JSON.stringify({ text: 'GO!', color: 'red' })}`);
      }
      await run(process.execPath, [resolve(this.root, 'scripts/duel-arena.mjs'), 'fight'], {
        cwd: this.root, env, timeout: 15000,
      });
      this.state.round = { state: 'running', ...settings, countdown: 0, error: null };
      this.state.event('round', `${mode}: ${botCount} bots${player ? ` → ${player}` : ''}`);
    } catch (error) {
      this.state.round = { state: 'error', ...settings, countdown: 0, error: error.message.slice(0, 500) };
      this.state.event('error', 'Round setup failed; inspect arena console logs');
    }
  }
  close() { this.closed = true; }
}
