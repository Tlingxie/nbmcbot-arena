const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const failure = message => Object.assign(new Error(message), { statusCode: 400 });

export function desiredFleets(botCount) {
  if (!Number.isInteger(botCount) || botCount < 1 || botCount > 100) throw failure('botCount must be an integer from 1 to 100');
  return [
    { kind: 'mace-team', prefix: 'Mace', count: Math.ceil(botCount / 2) },
    { kind: 'spear-team', prefix: 'Spear', count: Math.floor(botCount / 2) },
  ].map(group => ({ ...group, names: Array.from({ length: group.count }, (_, i) => `${group.prefix}${String(i + 1).padStart(3, '0')}`) }));
}

export async function reconcileFleets(botCount, { control, telemetryAddress = '127.0.0.1:4211', cancelled = () => false }) {
  const groups = desiredFleets(botCount);
  const current = new Map();
  for (const group of groups) {
    if (cancelled()) throw new Error('Round cancelled');
    const record = await control(group.kind, 'status');
    current.set(group.kind, record);
    if (record.running) await control(group.kind, 'command', 'stop');
  }
  for (const group of groups) {
    if (cancelled()) throw new Error('Round cancelled');
    let record = current.get(group.kind);
    const matches = record.running && JSON.stringify(record.usernames) === JSON.stringify(group.names);
    if (matches) continue;
    if (record.running) {
      await control(group.kind, 'command', 'quit');
      const deadline = Date.now() + 10000;
      do {
        record = await control(group.kind, 'status');
        if (!record.running) break;
        if (Date.now() > deadline) throw new Error(`${group.kind} did not exit; no replacement process was started`);
        await sleep(150);
      } while (!cancelled());
    }
    if (cancelled()) throw new Error('Round cancelled');
    if (!group.count) continue;
    await control(group.kind, 'start', undefined, {
      NBMCBOT_DUEL_COUNT: String(group.count), NBMCBOT_TELEMETRY_ADDR: telemetryAddress,
    });
    const started = await control(group.kind, 'status');
    if (!started.running || JSON.stringify(started.usernames) !== JSON.stringify(group.names)) {
      throw new Error(`${group.kind} did not start with the requested accounts`);
    }
  }
  return groups;
}
