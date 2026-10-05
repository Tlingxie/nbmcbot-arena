import http from 'node:http';
import dgram from 'node:dgram';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { createReadStream, statSync } from 'node:fs';
import { dirname, resolve, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ArenaState } from './lib/state.mjs';
import { Arena } from './lib/arena.mjs';
import { createRecordingStore } from './lib/recordings.mjs';

const webRoot = dirname(fileURLToPath(import.meta.url));
const defaultRoot = resolve(webRoot, '..');
const bad = (statusCode, message) => Object.assign(new Error(message), { statusCode });
async function body(req, limit = 16384) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limit) throw bad(413, 'Request body exceeds limit');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}
async function jsonBody(req) {
  try {
    const value = JSON.parse((await body(req)).toString('utf8'));
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw bad(400, 'JSON object required');
    return value;
  }
  catch (error) { if (error.statusCode) throw error; throw bad(400, 'Invalid JSON'); }
}
function json(res, value, status = 200) {
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' });
  res.end(JSON.stringify(value));
}

export async function createViewer({ root = defaultRoot, port = 4210, udpPort = 4211, poll = true, ffmpegPath } = {}) {
  const state = new ArenaState();
  const arena = new Arena({ root, state });
  const recordings = await createRecordingStore({ directory: resolve(root, '.runtime/recordings'), ffmpegPath });
  const token = randomBytes(32).toString('hex');
  const clients = new Set();
  let actualPort = port;
  let broadcastBusy = false;
  let receivingChunk = false;
  const snapshot = async () => ({ now: Date.now(), token, server: { address: '127.0.0.1:25566', version: '1.21.11', telemetryHz: 10 },
    players: state.players(), events: state.events, round: state.round, observer: state.observer, recordings: await recordings.list() });
  const server = http.createServer(async (req, res) => {
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'");
    try {
      const hosts = [`127.0.0.1:${actualPort}`, `localhost:${actualPort}`];
      if (!hosts.includes(req.headers.host)) throw bad(403, 'Invalid host');
      const url = new URL(req.url, `http://${req.headers.host}`);
      const path = url.pathname;
      if (!['GET', 'HEAD'].includes(req.method)) {
        const supplied = Buffer.from(String(req.headers['x-nbmc-token'] ?? ''));
        const expected = Buffer.from(token);
        if (req.headers.origin !== `http://${req.headers.host}` || supplied.length !== expected.length || !timingSafeEqual(supplied, expected)) throw bad(403, 'Same-origin session token required');
      }
      if (req.method === 'GET' && path === '/api/state') return json(res, await snapshot());
      if (req.method === 'GET' && path === '/api/events') {
        res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store', Connection: 'keep-alive' });
        res.write(`event: state\ndata: ${JSON.stringify(await snapshot())}\n\n`);
        clients.add(res);
        req.on('close', () => clients.delete(res));
        return;
      }
      if (req.method === 'POST' && path === '/api/spectate') return json(res, await arena.spectate(await jsonBody(req)));
      if (req.method === 'POST' && path === '/api/rounds') return json(res, await arena.startRound(await jsonBody(req)), 202);
      if (req.method === 'GET' && path === '/api/recordings') return json(res, await recordings.list());
      if (req.method === 'POST' && path === '/api/recordings') return json(res, await recordings.create(await jsonBody(req)), 201);
      const recording = path.match(/^\/api\/recordings\/([a-zA-Z0-9_-]+)(?:\/(finish|download|chunks\/\d+))?$/);
      if (recording) {
        const [, id, action] = recording;
        if (req.method === 'PUT' && action?.startsWith('chunks/')) {
          if (receivingChunk) throw bad(409, 'Another upload is in progress');
          receivingChunk = true;
          try { return json(res, await recordings.append(id, Number(action.split('/')[1]), await body(req, 16 * 1024 * 1024))); }
          finally { receivingChunk = false; }
        }
        if (req.method === 'POST' && action === 'finish') return json(res, await recordings.finish(id), 202);
        if (req.method === 'GET' && !action) return json(res, await recordings.get(id));
        if (req.method === 'GET' && action === 'download') {
          const file = await recordings.download(id);
          const stat = statSync(file.path);
          res.writeHead(200, { 'Content-Type': file.mimeType, 'Content-Length': stat.size, 'Content-Disposition': `attachment; filename="${file.filename}"` });
          createReadStream(file.path).on('error', () => res.destroy()).pipe(res);
          return;
        }
      }
      const staticPaths = new Set(['/', '/index.html', '/src/app.js', '/src/style.css', '/src/map.js', '/src/recording.js']);
      if (req.method === 'GET' && staticPaths.has(path)) {
        const file = resolve(webRoot, path === '/' ? 'index.html' : path.slice(1));
        const stat = statSync(file);
        const type = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8' }[extname(file)];
        res.writeHead(200, { 'Content-Type': type, 'Content-Length': stat.size, 'Cache-Control': 'no-cache' });
        createReadStream(file).on('error', () => res.destroy()).pipe(res);
        return;
      }
      throw bad(404, 'Not found');
    } catch (error) {
      if (res.headersSent) res.destroy();
      else json(res, { error: error.statusCode ? error.message : 'Local service error; inspect viewer log' }, error.statusCode ?? 500);
      if (!error.statusCode) console.error(error);
    }
  });
  server.requestTimeout = 30000;
  server.headersTimeout = 10000;
  const udp = dgram.createSocket('udp4');
  udp.on('message', (buffer, remote) => {
    if (remote.address !== '127.0.0.1' || buffer.length > 60000) return;
    try { state.ingest(JSON.parse(buffer)); } catch {}
  });
  await new Promise((resolve, reject) => { udp.once('error', reject); udp.bind(udpPort, '127.0.0.1', resolve); });
  try { await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve); }); }
  catch (error) { udp.close(); await recordings.close(); throw error; }
  actualPort = server.address().port;
  const ticker = setInterval(async () => {
    if (poll) arena.tick();
    if (broadcastBusy || !clients.size) return;
    broadcastBusy = true;
    try {
      const event = `event: state\ndata: ${JSON.stringify(await snapshot())}\n\n`;
      for (const client of clients) {
        if (client.writableLength > 262144) { client.destroy(); clients.delete(client); }
        else client.write(event);
      }
    } catch (error) { console.error(error); }
    finally { broadcastBusy = false; }
  }, 100);
  return { state, arena, server, recordings, url: `http://127.0.0.1:${actualPort}`, udpPort: udp.address().port,
    async close() {
      clearInterval(ticker); arena.close(); udp.close();
      for (const client of clients) client.end();
      await new Promise(resolve => server.close(resolve));
      await recordings.close();
    },
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const viewer = await createViewer({ port: Number(process.env.NBMCBOT_VIEWER_PORT ?? 4210), udpPort: Number(process.env.NBMCBOT_TELEMETRY_PORT ?? 4211) });
  console.log(JSON.stringify({ event: 'viewer_ready', url: viewer.url, udpPort: viewer.udpPort }));
  for (const signal of ['SIGTERM', 'SIGINT']) process.once(signal, async () => { await viewer.close(); process.exit(0); });
}
