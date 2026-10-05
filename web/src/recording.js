const WIDTH = 1280;
const HEIGHT = 720;
const FPS = 30;
const MAX_CHUNK = 16 * 1024 * 1024;
const MAX_PENDING = 32 * 1024 * 1024;

export class Recorder {
  constructor({ getCanvas, getState, onChange }) {
    this.getCanvas = getCanvas;
    this.getState = getState;
    this.onChange = onChange ?? (() => {});
    this.video = null;
    this.capture = null;
    this.recorder = null;
    this.output = null;
    this.canvas = document.createElement('canvas');
    this.canvas.width = WIDTH; this.canvas.height = HEIGHT;
    this.context = this.canvas.getContext('2d', { alpha: false });
    this.state = 'idle'; this.id = null; this.error = null; this.recording = null;
    this.elapsed = 0; this.startedAt = 0; this.sequence = 0;
    this.pendingBytes = 0; this.uploads = Promise.resolve();
    this.stopPromise = null; this.frame = null; this.poll = null;
    this.starting = false; this.uploadError = null;
  }

  notify(state = this.state) {
    this.state = state;
    this.onChange({ state, id: this.id, elapsed: this.elapsed, error: this.error, recording: this.recording });
  }

  async request(path, { method = 'GET', body, raw = false } = {}) {
    const headers = { 'X-NBMC-Token': this.getState()?.token ?? '' };
    if (body && !raw) headers['Content-Type'] = 'application/json';
    if (raw) headers['Content-Type'] = 'application/octet-stream';
    const response = await fetch(path, { method, headers, body: body && !raw ? JSON.stringify(body) : body });
    let value;
    try { value = await response.json(); } catch { value = {}; }
    if (!response.ok) throw new Error(value.error ?? `Recording request failed (${response.status})`);
    return value.recording ?? value.record ?? value;
  }

  async captureGame() {
    if (['recording', 'stopping'].includes(this.state)) throw new Error('Stop recording before changing capture');
    this.releaseCapture(); this.error = null; this.notify('capturing');
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: { displaySurface: 'window', frameRate: { ideal: FPS, max: FPS } },
        audio: false, systemAudio: 'exclude', selfBrowserSurface: 'exclude',
      });
      const track = stream.getVideoTracks()[0];
      if (!track || (track.getSettings().displaySurface && track.getSettings().displaySurface !== 'window')) {
        stream.getTracks().forEach(track => track.stop());
        throw new Error('Select the original Minecraft window to capture');
      }
      this.capture = stream;
      const video = document.createElement('video');
      video.muted = true; video.playsInline = true; video.srcObject = stream;
      await video.play();
      this.video = video;
      track.addEventListener('ended', () => {
        if (this.capture !== stream) return;
        if (this.state === 'recording') void this.stop();
        else this.notify('idle');
      });
      this.notify('ready');
      return video;
    } catch (error) {
      this.capture?.getTracks().forEach(track => track.stop()); this.capture = null;
      const reported = error.name === 'InvalidStateError'
        ? new Error('请先将此网页切到前台，再点击连接窗口', { cause: error }) : error;
      this.error = reported.message; this.notify('error'); throw reported;
    }
  }

  releaseCapture() {
    if (this.state === 'recording') void this.stop();
    const stream = this.capture; this.capture = null;
    stream?.getTracks().forEach(track => track.stop());
    if (this.video) { this.video.pause(); this.video.srcObject = null; }
    this.video = null;
    if (!['recording', 'stopping', 'processing'].includes(this.state)) this.notify('idle');
  }

  compose() {
    const video = this.video;
    if (!video || !video.videoWidth || video.readyState < 2) return false;
    const context = this.context;
    context.fillStyle = '#080b10'; context.fillRect(0, 0, WIDTH, HEIGHT);
    const scale = Math.min(WIDTH / video.videoWidth, HEIGHT / video.videoHeight);
    const width = video.videoWidth * scale, height = video.videoHeight * scale;
    try { context.drawImage(video, (WIDTH - width) / 2, (HEIGHT - height) / 2, width, height); }
    catch {
      const canvas = this.getCanvas?.();
      if (!canvas?.width || !canvas?.height) return false;
      context.drawImage(canvas, 0, 0, WIDTH, HEIGHT);
    }
    const state = this.getState() ?? {};
    const selected = state.selectedPlayer ?? state.selected;
    const selectedName = typeof selected === 'string' ? selected : selected?.name;
    const player = state.players?.find(player => player.name === selectedName);
    const position = player?.position?.map(value => Number(value).toFixed(1)).join(', ');
    context.fillStyle = 'rgba(8,11,16,.78)'; context.fillRect(0, HEIGHT - 52, WIDTH, 52);
    context.fillStyle = '#eef4ff'; context.font = '20px sans-serif';
    context.fillText(`NBMCBot Arena | ${selectedName ?? 'Original Minecraft window'}${position ? ` | ${position}` : ''}`, 20, HEIGHT - 20, WIDTH - 220);
    context.fillStyle = '#fb6571';
    context.fillText(`REC ${Math.floor(this.elapsed)}s`, WIDTH - 150, HEIGHT - 20);
    return true;
  }

  async start({ source = 'minecraft', video = this.video } = {}) {
    if (this.starting || ['recording', 'stopping', 'processing'].includes(this.state)) throw new Error('A recording or export is already active');
    if (source !== 'minecraft' || !video?.srcObject?.getVideoTracks().some(track => track.readyState === 'live')) {
      throw new Error('Capture the original Minecraft window before recording');
    }
    if (!globalThis.MediaRecorder || !this.canvas.captureStream) throw new Error('This browser does not support video recording');
    this.video = video;
    if (!this.compose()) throw new Error('Wait for the first Minecraft capture frame');
    const mimeType = ['video/webm;codecs=vp8', 'video/webm;codecs=vp9', 'video/webm', 'video/mp4']
      .find(type => MediaRecorder.isTypeSupported(type));
    if (!mimeType) throw new Error('This browser has no supported video recording format');
    this.output = this.canvas.captureStream(FPS);
    const recorder = new MediaRecorder(this.output, { mimeType, videoBitsPerSecond: 4_000_000 });
    this.error = null; this.elapsed = 0; this.sequence = 0; this.pendingBytes = 0;
    this.uploads = Promise.resolve(); this.stopPromise = null; this.uploadError = null;
    this.lastElapsedSecond = -1;
    this.starting = true;
    let created = false;
    try {
      this.recording = await this.request('/api/recordings', { method: 'POST', body: { mimeType, width: WIDTH, height: HEIGHT, fps: FPS, source } });
      this.id = this.recording.id;
      if (!this.id) throw new Error('Recording service did not return an id');
      created = true;
      this.recorder = recorder;
      this.stopped = new Promise(resolve => { recorder.addEventListener('stop', resolve, { once: true }); });
      recorder.addEventListener('dataavailable', event => this.upload(event.data));
      recorder.addEventListener('error', event => {
        this.error = event.error?.message ?? 'Browser recording failed'; void this.stop();
      });
      this.startedAt = performance.now(); recorder.start(1000); this.notify('recording');
      const draw = () => {
        if (this.state !== 'recording') return;
        this.elapsed = (performance.now() - this.startedAt) / 1000;
        if (!this.compose()) { this.error = 'Minecraft capture ended'; void this.stop(); return; }
        if (Math.floor(this.elapsed) !== this.lastElapsedSecond) {
          this.lastElapsedSecond = Math.floor(this.elapsed); this.notify();
        }
        this.frame = requestAnimationFrame(draw);
      };
      this.frame = requestAnimationFrame(draw);
      return this.recording;
    } catch (error) {
      this.output.getTracks().forEach(track => track.stop());
      this.recorder = null;
      if (created) {
        try { await this.request(`/api/recordings/${this.id}/finish`, { method: 'POST' }); } catch { /* Release an empty server recording after browser failure. */ }
      }
      this.error = error.message; this.notify('error'); throw error;
    } finally { this.starting = false; }
  }

  upload(blob) {
    if (!blob.size || this.error) return;
    if (blob.size > MAX_CHUNK || this.pendingBytes + blob.size > MAX_PENDING) {
      this.error = 'Recording upload cannot keep up; the saved prefix can still be exported';
      void this.stop(); return;
    }
    const sequence = this.sequence++; this.pendingBytes += blob.size;
    this.uploads = this.uploads.then(async () => {
      try {
        if (this.uploadError) return;
        await this.request(`/api/recordings/${this.id}/chunks/${sequence}`, { method: 'PUT', body: blob, raw: true });
      } catch (error) {
        this.uploadError = error; this.error = error.message;
        if (this.state === 'recording') void this.stop();
      } finally { this.pendingBytes -= blob.size; }
    });
  }

  stop() {
    if (this.stopPromise) return this.stopPromise;
    if (!this.recorder) return Promise.resolve(this.recording);
    this.stopPromise = (async () => {
      this.notify('stopping'); cancelAnimationFrame(this.frame); this.frame = null;
      if (this.recorder.state !== 'inactive') this.recorder.stop();
      await this.stopped; await this.uploads;
      this.output?.getTracks().forEach(track => track.stop()); this.recorder = null;
      try {
        this.recording = await this.request(`/api/recordings/${this.id}/finish`, { method: 'POST' });
        this.notify(this.recording.status === 'processing' ? 'processing' : this.recording.status === 'complete' ? 'complete' : 'error');
        if (this.recording.status === 'processing') this.pollExport();
        return this.recording;
      } catch (error) { this.error = error.message; this.notify('error'); return null; }
    })();
    return this.stopPromise;
  }

  pollExport() {
    clearTimeout(this.poll);
    this.poll = setTimeout(async () => {
      try {
        this.recording = await this.request(`/api/recordings/${this.id}`);
        if (this.recording.status === 'processing') this.pollExport();
        else {
          this.error = this.recording.error ?? this.error;
          this.notify(this.recording.status === 'complete' ? 'complete' : 'error');
        }
      } catch (error) { this.error = error.message; this.notify('error'); }
    }, 1000);
  }
}
