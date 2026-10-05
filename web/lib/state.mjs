export const validName = name => typeof name === 'string' && /^[A-Za-z0-9_]{1,16}$/.test(name);
const vector = value => value === null || (Array.isArray(value) && value.length === 3 && value.every(n => Number.isFinite(n) && Math.abs(n) <= 6e7));
const number = value => Number.isFinite(value) ? value : null;
const string = value => typeof value === 'string' ? value.slice(0, 160) : null;
const boolean = value => typeof value === 'boolean' ? value : null;

export class ArenaState {
  constructor() {
    this.entries = new Map();
    this.sequences = new Map();
    this.events = [];
    this.round = { state: 'idle', mode: 'mace-vs-mace', countdown: 0, error: null };
    this.observer = { viewer: null, target: null };
  }
  ingest(frame, now = Date.now()) {
    if (frame?.event !== 'telemetry_frame' || frame.version !== 1 || !Number.isSafeInteger(frame.process_id)
      || !Number.isSafeInteger(frame.sequence) || frame.sequence < 0 || !Array.isArray(frame.players)
      || frame.players.length > 256 || !Number.isFinite(frame.sampled_at_ms)
      || now - frame.sampled_at_ms > 2000 || frame.sampled_at_ms - now > 2000) return false;
    if (frame.players.some(p => !p || !validName(p.name) || !['self', 'observed'].includes(p.source)
      || !vector(p.position) || (p.velocity !== undefined && !vector(p.velocity)) || typeof p.connected !== 'boolean')) return false;
    const last = this.sequences.get(frame.process_id);
    if (last && now - last.at < 10000 && frame.sequence <= last.sequence) return false;
    this.sequences.set(frame.process_id, { sequence: frame.sequence, at: now });
    for (const [pid, data] of this.sequences) if (now - data.at > 60000) this.sequences.delete(pid);
    for (const p of frame.players) this.update(p, now);
    return true;
  }
  update(p, now) {
    const old = this.entries.get(p.name);
    if (p.source !== 'self' && old?.source === 'self' && now - old.updatedAt < 2000) return;
    if (!old && this.entries.size >= 256) {
      const oldest = [...this.entries.values()].sort((a, b) => a.updatedAt - b.updatedAt)[0];
      this.entries.delete(oldest.name);
    }
    this.entries.set(p.name, {
      name: p.name, uuid: string(p.uuid), source: p.source,
      position: p.position ?? null, velocity: p.velocity ?? null,
      yaw: number(p.yaw), pitch: number(p.pitch), health: number(p.health),
      alive: boolean(p.alive), connected: p.connected === true,
      dimension: string(p.dimension), task: string(p.task), style: string(p.style),
      phase: string(p.phase), target: string(p.target), gliding: boolean(p.gliding),
      equipment: Object.fromEntries(['mainhand', 'chest', 'offhand'].map(key => [key, string(p.equipment?.[key])])),
      team: /^Mace\d+$/.test(p.name) ? 'red' : /^Spear\d+$/.test(p.name) ? 'blue' : 'observer',
      updatedAt: p.source === 'self' || p.positionSample ? now : old?.updatedAt ?? 0,
      seenAt: now,
    });
  }
  observe(name, data, now = Date.now()) {
    if (!validName(name) || ('position' in data && !vector(data.position))) return;
    this.update({ ...this.entries.get(name), name, source: 'observed', connected: true, ...data, positionSample: 'position' in data }, now);
  }
  players(now = Date.now()) {
    return [...this.entries.values()].map(p => ({ ...p, stale: now - p.updatedAt > 2000,
      connected: p.connected && now - p.seenAt <= 5000 })).sort((a, b) => a.name.localeCompare(b.name));
  }
  event(type, message, extra = {}) {
    this.events.push({ id: `${Date.now()}-${this.events.length}`, at: Date.now(), time: Date.now(), type, message: String(message).slice(0, 500), ...extra });
    if (this.events.length > 100) this.events.splice(0, this.events.length - 100);
  }
}
