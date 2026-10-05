import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createRecordingStore } from '../lib/recordings.mjs';
import { Recorder } from '../src/recording.js';

const meta = { mimeType: 'video/webm;codecs=vp8', width: 1280, height: 720, fps: 30, source: 'minecraft' };
async function setup(t, options = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'nbmc-recording-'));
  const store = createRecordingStore({ directory, ffmpegPath: '/not-installed/ffmpeg', ...options });
  t.after(async () => { await store.close(); await rm(directory, { recursive: true, force: true }); });
  return store;
}
async function finalized(store, id) {
  for (let attempt = 0; attempt < 100; attempt++) {
    const record = await store.get(id);
    if (record.status !== 'processing') return record;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  assert.fail('processing did not finish');
}

test('ordered segments persist exactly and failed export keeps the source', async t => {
  const store = await setup(t);
  const record = await store.create(meta);
  assert.equal(record.status, 'recording');
  await assert.rejects(store.append(record.id, 1, Buffer.from('later')), { statusCode: 409 });
  await store.append(record.id, 0, Buffer.from('first'));
  await assert.rejects(store.append(record.id, 0, Buffer.from('duplicate')), { statusCode: 409 });
  await store.append(record.id, 1, Buffer.from('second'));
  assert.equal((await store.finish(record.id)).status, 'processing');
  const done = await finalized(store, record.id);
  assert.equal(done.status, 'failed');
  assert.ok(done.error);
  const download = await store.download(record.id);
  assert.equal(download.mimeType, 'video/webm');
  assert.match(download.filename, /\.webm$/);
  assert.equal((await readFile(download.path)).toString(), 'firstsecond');
  assert.equal((await store.list()).length, 1);
  await assert.rejects(store.append(record.id, 2, Buffer.from('late')), { statusCode: 409 });
});

test('only one capture or export can occupy the store', async t => {
  const store = await setup(t);
  const record = await store.create(meta);
  await assert.rejects(store.create(meta), { statusCode: 409 });
  await store.append(record.id, 0, Buffer.from('not-video'));
  const finishing = store.finish(record.id);
  await assert.rejects(store.create(meta), { statusCode: 409 });
  await finishing;
  await finalized(store, record.id);
  assert.equal((await store.create(meta)).status, 'recording');
});

test('rejects unsupported metadata, unsafe ids and oversized chunks', async t => {
  const store = await setup(t);
  for (const invalid of [{...meta,source:'fake'}, {...meta,width:8000}, {...meta,mimeType:'text/plain'}, {...meta,fps:0}]) {
    await assert.rejects(store.create(invalid), { statusCode: 400 });
  }
  await assert.rejects(store.get('../outside'), { statusCode: 404 });
  const record = await store.create(meta);
  await assert.rejects(store.append(record.id, -1, Buffer.from('a')), { statusCode: 400 });
  await assert.rejects(store.append(record.id, 0, Buffer.alloc(16 * 1024 * 1024 + 1)), { statusCode: 413 });
  assert.equal((await store.get(record.id)).bytes, 0);
  await assert.rejects(store.finish(record.id), { statusCode: 400 });
  assert.equal((await store.create(meta)).status, 'recording');
});

test('browser stop flushes the final chunk and waits for ordered authenticated uploads', async t => {
  const originals = new Map(['document','fetch','MediaRecorder','requestAnimationFrame','cancelAnimationFrame'].map(key => [key, Object.getOwnPropertyDescriptor(globalThis,key)]));
  t.after(() => { for (const [key, descriptor] of originals) {
    if (descriptor) Object.defineProperty(globalThis,key,descriptor); else delete globalThis[key];
  }});
  const context = { fillRect() {}, fillText() {}, drawImage() {} };
  const output = { getTracks: () => [{stop() {}}] };
  globalThis.document = { createElement: () => ({ getContext: () => context, captureStream: () => output }) };
  globalThis.requestAnimationFrame = () => 1; globalThis.cancelAnimationFrame = () => {};
  class BrowserRecorder extends EventTarget {
    static isTypeSupported() { return true; }
    start(timeslice) { assert.equal(timeslice,1000); this.state = 'recording'; }
    stop() {
      this.state = 'inactive';
      this.dispatchEvent(Object.assign(new Event('dataavailable'), { data: new Blob(['last']) }));
      this.dispatchEvent(new Event('stop'));
    }
  }
  globalThis.MediaRecorder = BrowserRecorder;
  const calls = [];
  let release;
  const firstUpload = new Promise(resolve => { release = resolve; });
  globalThis.fetch = async (url, options) => {
    assert.equal(options.headers['X-NBMC-Token'],'session-token');
    calls.push(url);
    if (url.endsWith('/chunks/0')) await firstUpload;
    return { ok: true, json: async () => ({ id:'record-1',status:url.endsWith('/finish')?'complete':'recording' }) };
  };
  const changes = [];
  const recorder = new Recorder({ getCanvas: () => null, getState: () => ({ token:'session-token' }), onChange: value => changes.push(value) });
  const video = { videoWidth:1280,videoHeight:720,readyState:2,srcObject:{getVideoTracks:()=>[{readyState:'live'}]} };
  await recorder.start({source:'minecraft',video});
  recorder.upload(new Blob(['first'])); recorder.upload(new Blob(['second']));
  const stopping = recorder.stop();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls,['/api/recordings','/api/recordings/record-1/chunks/0']);
  release();
  await stopping;
  assert.deepEqual(calls,['/api/recordings','/api/recordings/record-1/chunks/0','/api/recordings/record-1/chunks/1','/api/recordings/record-1/chunks/2','/api/recordings/record-1/finish']);
  assert.equal(changes.at(-1).state,'complete');
  assert.equal(recorder.pendingBytes,0);
});

test('capture requests a silent window and refuses a whole-monitor capture', async t => {
  const originals = new Map(['document','navigator'].map(key => [key,Object.getOwnPropertyDescriptor(globalThis,key)]));
  t.after(() => { for (const [key, descriptor] of originals) {
    if (descriptor) Object.defineProperty(globalThis,key,descriptor); else delete globalThis[key];
  }});
  const video = {play:async()=>{},pause() {}};
  globalThis.document = {createElement: type => type === 'video' ? video : { getContext:()=>({}) }};
  let stopped = 0;
  let surface = 'window';
  const track = {getSettings:()=>({displaySurface:surface}),addEventListener() {},stop() {stopped++;}};
  const stream = {getVideoTracks:()=>[track],getTracks:()=>[track]};
  Object.defineProperty(globalThis,'navigator',{configurable:true,value:{mediaDevices:{getDisplayMedia:async constraints=> {
    assert.equal(constraints.audio,false);
    assert.equal(constraints.video.displaySurface,'window');
    return stream;
  }}}});
  const recorder = new Recorder({getState:()=>({}),onChange() {}});
  assert.equal(await recorder.captureGame(),video);
  assert.equal(video.muted,true);
  recorder.releaseCapture(); assert.equal(stopped,1);
  surface = 'monitor';
  await assert.rejects(recorder.captureGame(),/Select the original Minecraft window/);
  assert.equal(stopped,2);
  navigator.mediaDevices.getDisplayMedia = async () => { throw new DOMException('Invalid state','InvalidStateError'); };
  await assert.rejects(recorder.captureGame(), error => error.message === '请先将此网页切到前台，再点击连接窗口' && error.cause?.name === 'InvalidStateError');
});

test('real FFmpeg export produces a decodable MP4 and survives restart', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'nbmc-export-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const input = join(directory, 'input.webm');
  await promisify(execFile)('ffmpeg', ['-nostdin','-v','error','-f','lavfi','-i','color=c=black:s=1280x720:r=30','-t','0.2','-an','-c:v','libvpx','-y',input]);
  const store = createRecordingStore({ directory });
  t.after(() => store.close());
  const record = await store.create(meta);
  await store.append(record.id, 0, await readFile(input));
  await store.finish(record.id);
  const done = await finalized(store, record.id);
  assert.equal(done.status, 'complete', done.error);
  const exported = await store.download(record.id);
  assert.equal(exported.mimeType, 'video/mp4');
  await promisify(execFile)('ffmpeg', ['-nostdin','-v','error','-i',exported.path,'-f','null','-']);
  await store.close();
  const recovered = createRecordingStore({ directory });
  t.after(() => recovered.close());
  assert.equal((await recovered.get(record.id)).status, 'complete');
  assert.equal((await recovered.list()).length, 1);
});
