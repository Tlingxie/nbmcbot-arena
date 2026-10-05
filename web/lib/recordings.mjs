import { randomUUID } from 'node:crypto';
import { mkdir, open, readFile, readdir, rename, stat, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const MAX_CHUNK = 16 * 1024 * 1024;
const MAX_TOTAL = 2 * 1024 * 1024 * 1024;
const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
function fail(statusCode, message) { return Object.assign(new Error(message), { statusCode }); }

export function createRecordingStore({ directory, ffmpegPath = 'ffmpeg' }) {
  const root = resolve(directory);
  const records = new Map();
  let active = null;
  let closed = false;
  let child = null;
  let job = null;
  let uploading = false;
  let serial = Promise.resolve();
  const sourcePath = record => join(root, `${record.id}.original.${record.sourceExtension}`);
  const exportPath = record => join(root, `${record.id}.mp4`);
  const view = record => ({
    id: record.id, status: record.status, mimeType: record.mimeType,
    width: record.width, height: record.height, fps: record.fps, source: record.source,
    bytes: record.bytes, nextSequence: record.nextSequence,
    createdAt: record.createdAt, finishedAt: record.finishedAt ?? null,
    error: record.error ?? null, downloadUrl: ['complete', 'failed'].includes(record.status)
      ? `/api/recordings/${record.id}/download` : null,
    downloadMimeType: record.status === 'complete' ? 'video/mp4' : record.mimeType.split(';')[0],
  });
  async function persist(record) {
    const path = join(root, `${record.id}.json`);
    await writeFile(`${path}.tmp`, JSON.stringify({ ...view(record), sourceExtension: record.sourceExtension }));
    await rename(`${path}.tmp`, path);
  }
  const ready = (async () => {
    await mkdir(root, { recursive: true });
    for (const file of await readdir(root)) {
      if (!file.endsWith('.json') || !ID.test(file.slice(0, -5))) continue;
      try {
        const record = JSON.parse(await readFile(join(root, file), 'utf8'));
        if (record.id !== file.slice(0, -5) || !['webm', 'mp4'].includes(record.sourceExtension)) continue;
        if (['recording', 'processing'].includes(record.status)) {
          record.status = 'failed'; record.error = 'Recording interrupted by service restart';
          await persist(record);
        }
        records.set(record.id, record);
      } catch { /* An incomplete metadata file must not prevent startup. */ }
    }
  })();
  function enqueue(operation, allowClosed = false) {
    const result = serial.then(async () => {
      await ready;
      if (closed && !allowClosed) throw fail(503, 'Recording service is closed');
      return operation();
    });
    serial = result.catch(() => {});
    return result;
  }
  function find(id) {
    if (!ID.test(id) || !records.has(id)) throw fail(404, 'Recording not found');
    return records.get(id);
  }
  function convert(record) {
    return new Promise((resolveJob, rejectJob) => {
      let errors = '';
      child = spawn(ffmpegPath, ['-nostdin', '-v', 'error', '-y', '-i', sourcePath(record),
        '-vf', `scale=${record.width}:${record.height}:force_original_aspect_ratio=decrease,pad=${record.width}:${record.height}:(ow-iw)/2:(oh-ih)/2,fps=${record.fps}`,
        '-an', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-movflags', '+faststart', exportPath(record)],
      { stdio: ['ignore', 'ignore', 'pipe'] });
      const timeout = setTimeout(() => child?.kill('SIGTERM'), 10 * 60 * 1000);
      timeout.unref();
      child.stderr.on('data', data => { errors = (errors + data.toString()).slice(-8192); });
      child.once('error', error => { clearTimeout(timeout); rejectJob(error); });
      child.once('close', code => {
        clearTimeout(timeout);
        if (code === 0) resolveJob();
        else rejectJob(new Error(errors.trim() || `FFmpeg exited with code ${code}`));
      });
    });
  }
  return {
    create(meta) { return enqueue(async () => {
      if (active) throw fail(409, 'Another recording or export is active');
      if (!meta || meta.source !== 'minecraft' || typeof meta.mimeType !== 'string'
        || !/^video\/(webm|mp4)(;codecs=[a-zA-Z0-9., -]+)?$/.test(meta.mimeType)
        || !Number.isInteger(meta.width) || meta.width < 2 || meta.width > 1920 || meta.width % 2
        || !Number.isInteger(meta.height) || meta.height < 2 || meta.height > 1080 || meta.height % 2
        || !Number.isInteger(meta.fps) || meta.fps < 1 || meta.fps > 60) throw fail(400, 'Invalid recording metadata');
      const record = { id: randomUUID(), status: 'recording', mimeType: meta.mimeType,
        width: meta.width, height: meta.height, fps: meta.fps, source: meta.source,
        sourceExtension: meta.mimeType.startsWith('video/mp4') ? 'mp4' : 'webm',
        bytes: 0, nextSequence: 0, createdAt: new Date().toISOString() };
      record.handle = await open(sourcePath(record), 'wx', 0o600);
      try { await persist(record); } catch (error) { await record.handle.close(); throw error; }
      records.set(record.id, record); active = record;
      return view(record);
    }); },
    append(id, sequence, buffer) {
      if (uploading) return Promise.reject(fail(409, 'Another chunk upload is active'));
      uploading = true;
      return enqueue(async () => {
      const record = find(id);
      if (record.status !== 'recording') throw fail(409, 'Recording is no longer accepting chunks');
      if (!Number.isSafeInteger(sequence) || sequence < 0) throw fail(400, 'Invalid chunk sequence');
      if (sequence !== record.nextSequence) throw fail(409, `Expected chunk sequence ${record.nextSequence}`);
      if (!Buffer.isBuffer(buffer) || !buffer.length) throw fail(400, 'Empty or invalid chunk');
      if (buffer.length > MAX_CHUNK || record.bytes + buffer.length > MAX_TOTAL) throw fail(413, 'Recording upload size limit exceeded');
      await record.handle.writeFile(buffer);
      record.bytes += buffer.length; record.nextSequence++;
      await persist(record);
      return view(record);
      }).finally(() => { uploading = false; });
    },
    finish(id) { return enqueue(async () => {
      const record = find(id);
      if (record.status !== 'recording') return view(record);
      if (!record.bytes) {
        await record.handle.close(); record.handle = null;
        record.status = 'failed'; record.error = 'Recording contains no video data';
        active = null; await persist(record);
        throw fail(400, record.error);
      }
      await record.handle.close(); record.handle = null;
      record.status = 'processing'; await persist(record);
      job = convert(record).then(async () => {
        if (!(await stat(exportPath(record))).size) throw new Error('FFmpeg produced an empty file');
        record.status = 'complete';
      }).catch(error => { record.status = 'failed'; record.error = error.message; })
        .then(() => enqueue(async () => {
          record.finishedAt = new Date().toISOString(); child = null; active = null;
          await persist(record);
        }, true));
      return view(record);
    }); },
    get(id) { return enqueue(() => view(find(id))); },
    list() { return enqueue(() => [...records.values()].map(view).sort((a,b) => b.createdAt.localeCompare(a.createdAt))); },
    download(id) { return enqueue(async () => {
      const record = find(id);
      if (!['complete', 'failed'].includes(record.status) || !record.bytes) throw fail(409, 'Recording is not ready for download');
      const complete = record.status === 'complete';
      const path = complete ? exportPath(record) : sourcePath(record);
      await stat(path);
      return { path, filename: `arena-${record.id}.${complete ? 'mp4' : record.sourceExtension}`,
        mimeType: complete ? 'video/mp4' : record.mimeType.split(';')[0] };
    }); },
    async close() {
      await enqueue(async () => {
        closed = true;
        if (active?.status === 'recording') {
          await active.handle.close(); active.handle = null;
          active.status = 'failed'; active.error = 'Recording interrupted by service shutdown';
          await persist(active); active = null;
        }
        child?.kill('SIGTERM');
      }, true);
      if (job) await job;
    },
  };
}
