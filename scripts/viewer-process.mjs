#!/usr/bin/env node
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, openSync, closeSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const directory = resolve(root, '.runtime/viewer');
mkdirSync(directory, { recursive: true });
const recordPath = resolve(directory, 'process.json');
const action = process.argv[2] ?? 'status';
let record;
try { record = JSON.parse(readFileSync(recordPath, 'utf8')); } catch {}
let running = false;
try {
  if (Number.isSafeInteger(record?.pid)) running = execFileSync('ps', ['-o', 'args=', '-p', String(record.pid)], { encoding: 'utf8' }).includes(resolve(root, 'web/server.mjs'));
} catch {}
if (action === 'status') console.log(JSON.stringify({ running, ...record }));
else if (action === 'stop') {
  if (running) process.kill(record.pid, 'SIGTERM');
  console.log(JSON.stringify({ stopped: running }));
} else if (action === 'start') {
  if (running) throw new Error(`Viewer is already running: ${record.url}`);
  const log = resolve(directory, 'console.log');
  const output = openSync(log, 'a', 0o600);
  const child = spawn(process.execPath, [resolve(root, 'web/server.mjs')], { cwd: root, detached: true, stdio: ['ignore', output, output] });
  await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
  closeSync(output);
  child.unref();
  const url = `http://127.0.0.1:${process.env.NBMCBOT_VIEWER_PORT ?? 4210}`;
  for (let attempt = 0; attempt < 30; attempt++) {
    try {
      if ((await fetch(`${url}/api/state`)).ok) {
        record = { pid: child.pid, url, log, startedAt: new Date().toISOString() };
        writeFileSync(recordPath, `${JSON.stringify(record, null, 2)}\n`, { mode: 0o600 });
        console.log(JSON.stringify(record));
        process.exit(0);
      }
    } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`Viewer did not start. Inspect ${log}`);
} else throw new Error('usage: node scripts/viewer-process.mjs start|stop|status');
