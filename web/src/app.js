import { Recorder } from './recording.js';
import { drawMap } from './map.js';

const $ = id => document.getElementById(id);
let state = { players: [], events: [], recordings: [], round: {} };
let selected = null;
let video = null;
let recordingState = 'idle';
let online = false;
let noticeTimer;
const rosterNodes = new Map();
const teamNodes = new Map();
const pendingActions = new Set();
const canvas = $('game-canvas');
const context = canvas.getContext('2d');
const statusNames = { idle: '待机', capturing: '选择窗口中', ready: '可录制', recording: '录制中', stopping: '上传中', processing: '导出中', complete: '导出完成', error: '录像错误' };

function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = String(text);
  if (className) node.className = className;
  return node;
}
function notify(message, error = false) {
  $('notice').textContent = message; $('notice').classList.toggle('error', error); $('notice').hidden = false;
  clearTimeout(noticeTimer); noticeTimer = setTimeout(() => { $('notice').hidden = true; }, 6000);
}
function recordingChanged(change) {
  recordingState = typeof change === 'string' ? change : change.state ?? recordingState;
  $('record-status').textContent = statusNames[recordingState] ?? recordingState;
  $('record-status').classList.toggle('recording', recordingState === 'recording');
  $('record-message').textContent = change.error ?? (recordingState === 'recording' ? `正在录制 ${Math.floor(Number(change.elapsed ?? 0))} 秒` : statusNames[recordingState] ?? '');
  if (change.error) notify(change.error, true);
  if (change.recording) refreshState();
  updateButtons();
}
const recorder = new Recorder({ getCanvas: () => canvas, getState: () => ({ ...state, selectedPlayer: selected }), onChange: recordingChanged });

async function mutate(path, payload) {
  if (!state.token) throw new Error('尚未取得服务端会话，请等待连接恢复。');
  const response = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-NBMC-Token': state.token }, body: JSON.stringify(payload) });
  const result = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(result.error?.message ?? result.error ?? `操作失败（${response.status}）`);
  return result;
}
async function action(button, task) {
  if (pendingActions.has(button)) return;
  pendingActions.add(button);
  button.disabled = true;
  try { await task(); } catch (error) { notify(error.message, true); }
  finally { pendingActions.delete(button); updateButtons(); }
}
function updateButtons() {
  const busy = ['recording', 'stopping', 'processing', 'capturing'].includes(recordingState);
  $('record-start').disabled = !video || !online || busy;
  $('record-stop').disabled = recordingState !== 'recording';
  $('release').disabled = !video || busy;
  $('capture').disabled = busy;
  const target = state.players.find(player => player.name === selected);
  const viewerAvailable = state.players.some(player => player.name === $('viewer').value && player.team === 'observer' && player.connected && !player.stale);
  $('spectate').disabled = !online || !viewerAvailable || !target || !target.connected || target.stale || target.alive === false || target.name === $('viewer').value;
  $('free-view').disabled = !online || !viewerAvailable;
  $('viewer').disabled = !viewerAvailable;
  $('round-start').disabled = !online || ['starting', 'countdown', 'preparing'].includes(state.round?.state);
  for (const button of pendingActions) button.disabled = true;
}
function freshText(player) {
  if (!player.connected) return '离线';
  if (player.stale) return '过期';
  if (player.alive === false) return '已死亡';
  return '实时';
}
function coordinates(player) {
  return Array.isArray(player.position) && player.position.every(Number.isFinite) ? player.position.map(value => value.toFixed(1)).join(' / ') : '— / — / —';
}
function renderRoster() {
  const filter = $('search').value.trim().toLowerCase();
  const names = new Set(state.players.map(player => player.name));
  for (const name of rosterNodes.keys()) if (!names.has(name)) rosterNodes.delete(name);
  for (const [team, label] of [['red', '红队'], ['blue', '蓝队'], ['observer', '观察者']]) {
    const members = state.players.filter(player => (player.team ?? 'observer') === team && player.name.toLowerCase().includes(filter));
    if (!members.length) continue;
    let section = teamNodes.get(team);
    if (!section) { section = element('section', undefined, `team team-${team}`); section.append(element('h2', label)); teamNodes.set(team, section); }
    const header = section.firstChild;
    header.replaceChildren(document.createTextNode(label), element('span', `${members.filter(player => player.connected && !player.stale).length} / ${members.length}`));
    const ordered = [header];
    for (const player of members.sort((a, b) => a.name.localeCompare(b.name))) {
      let button = rosterNodes.get(player.name);
      if (!button) { button = element('button', undefined, 'player'); button.type = 'button'; button.addEventListener('click', () => { selected = player.name; render(); }); rosterNodes.set(player.name, button); }
      button.classList.toggle('selected', selected === player.name); button.classList.toggle('stale', player.stale || !player.connected); button.setAttribute('aria-pressed', String(selected === player.name));
      const row = element('div', undefined, 'player-top'); row.append(element('strong', player.name), element('span', freshText(player), 'freshness')); button.replaceChildren(row);
      const health = Number.isFinite(player.health) ? player.health : null;
      const healthRow = element('div', undefined, 'health-row'); const track = element('span', undefined, 'health-track'); const fill = element('span', undefined, 'health-fill'); fill.style.width = `${Math.max(0, Math.min(100, (health ?? 0) / 20 * 100))}%`; track.append(fill); healthRow.append(track, element('span', health === null ? '— HP' : `${health.toFixed(1)} HP`)); button.append(healthRow);
      button.append(element('span', coordinates(player), 'coordinates'), element('span', player.task ?? '任务未知', 'player-task'));
      ordered.push(button);
    }
    reconcile(section, ordered);
  }
  const visibleTeams = [...teamNodes.entries()].filter(([team]) => state.players.some(player => (player.team ?? 'observer') === team && player.name.toLowerCase().includes(filter))).map(([, node]) => node);
  reconcile($('roster'), visibleTeams.length ? visibleTeams : [element('p', state.players.length ? '没有匹配的玩家' : '等待真实玩家遥测', 'empty-small')]);
  $('population').textContent = `${state.players.filter(player => player.connected && !player.stale).length} 位在线`;
}
function reconcile(parent, nodes) {
  nodes.forEach((node, index) => { if (parent.children[index] !== node) parent.insertBefore(node, parent.children[index] ?? null); });
  while (parent.children.length > nodes.length) parent.lastElementChild.remove();
}
function renderRecordings() {
  const fragment = document.createDocumentFragment();
  for (const record of [...state.recordings].sort((a, b) => String(b.createdAt ?? '').localeCompare(String(a.createdAt ?? ''))).slice(0, 12)) {
    const row = element('div', undefined, 'recording-item'); const title = element('strong', record.filename ?? record.id);
    row.append(title, element('span', record.error ?? record.status ?? record.state ?? '处理中', 'muted'));
    if (record.status === 'complete' || record.state === 'complete' || record.status === 'ready' || record.downloadUrl) {
      const isMp4 = record.downloadMimeType === 'video/mp4' || record.status === 'complete';
      const link = element('a', isMp4 ? '下载 MP4' : '下载原始录像', 'download');
      link.href = `/api/recordings/${encodeURIComponent(record.id)}/download`; link.setAttribute('download', ''); row.append(link);
    }
    fragment.append(row);
  }
  if (!fragment.childNodes.length) fragment.append(element('p', '暂无录像', 'empty-small'));
  $('recordings').replaceChildren(fragment);
}
function renderEvents() {
  const fragment = document.createDocumentFragment();
  for (const event of state.events.slice(-15).reverse()) {
    const row = element('li');
    const when = event.at ?? event.timestamp ?? event.time ?? event.createdAt ?? event.ts;
    const date = when ? new Date(when) : null;
    row.append(element('time', date && Number.isFinite(date.getTime()) ? date.toLocaleTimeString('zh-CN', { hour12: false }) : '—'));
    row.append(element('span', event.message ?? [event.bot ?? event.player, event.event ?? event.type, event.target].filter(Boolean).join(' · ')));
    fragment.append(row);
  }
  if (!fragment.childNodes.length) fragment.append(element('li', '等待竞技场事件', 'muted'));
  $('events').replaceChildren(fragment);
}
function render() {
  renderRoster(); renderRecordings(); renderEvents();
  const player = state.players.find(player => player.name === selected);
  const viewerNames = [...new Set(state.players.filter(player => player.team === 'observer' && player.connected && !player.stale).map(player => player.name))].sort();
  const previousViewer = $('viewer').value;
  if ([...$('viewer').options].map(option => option.value).join(',') !== viewerNames.join(',') || (!viewerNames.length && $('viewer').options[0]?.textContent !== '等待观战账号登录')) {
    const options = viewerNames.map(name => { const option = element('option', name); option.value = name; return option; });
    if (!options.length) { const placeholder = element('option', '等待观战账号登录'); placeholder.value = ''; options.push(placeholder); }
    $('viewer').replaceChildren(...options);
    $('viewer').value = viewerNames.includes(previousViewer) ? previousViewer : viewerNames[0] ?? '';
  }
  $('selected-name').textContent = player?.name ?? '尚未选择玩家';
  $('selected-details').textContent = player ? `${freshText(player)} · XYZ ${coordinates(player)} · ${player.source === 'self' ? '自身遥测' : '外部观测'}` : '从左侧名单选择跟拍目标';
  $('view-title').textContent = player ? `原版画面 · 已选 ${player.name}` : '原版游戏画面';
  $('version').textContent = state.server?.version ?? '1.21.11';
  $('clock').textContent = new Date(state.now ?? Date.now()).toLocaleTimeString('zh-CN', { hour12: false });
  const layout = drawMap($('map'), state.players, selected);
  $('map-note').textContent = layout.points.length ? `${layout.points.length} 位有坐标 · ${layout.dimension ?? '维度未知'} · 范围约 ${Math.round(layout.span)} 格` : '暂无可用位置观测';
  const round = state.round ?? {};
  $('round-status').textContent = round.state ?? '待机';
  $('round-note').textContent = round.error ?? (round.countdown > 0 ? `${round.countdown} 秒后开局` : '开启新局会重新开始竞技场对局。');
  updateButtons();
}
function acceptState(next) {
  if (!next || !Array.isArray(next.players)) throw new Error('服务器状态格式不正确');
  state = { ...next, events: next.events ?? [], recordings: next.recordings ?? [], round: next.round ?? {} };
  online = true; $('connection').textContent = '遥测已连接'; $('connection').classList.add('live'); render();
}
async function refreshState() {
  try { const response = await fetch('/api/state', { cache: 'no-store' }); if (!response.ok) throw new Error(`状态请求失败（${response.status}）`); acceptState(await response.json()); }
  catch (error) { online = false; $('connection').textContent = '连接中断'; $('connection').classList.remove('live'); updateButtons(); notify(error.message, true); }
}

$('search').addEventListener('input', renderRoster);
$('viewer').addEventListener('change', updateButtons);
$('spectate').addEventListener('click', () => action($('spectate'), async () => { await mutate('/api/spectate', { viewer: $('viewer').value, target: selected }); notify(`已请求 ${$('viewer').value} 跟拍 ${selected}`); }));
$('free-view').addEventListener('click', () => action($('free-view'), async () => { await mutate('/api/spectate', { viewer: $('viewer').value, target: null }); notify('已请求返回自由视角'); }));
$('round-start').addEventListener('click', () => action($('round-start'), async () => {
  const countdown = Number($('countdown').value);
  if (!Number.isInteger(countdown) || countdown < 0 || countdown > 30) throw new Error('倒计时必须为 0 到 30 的整数');
  await mutate('/api/rounds', { mode: $('round-mode').value, countdown }); notify('新一局已请求'); await refreshState();
}));
$('capture').addEventListener('click', () => action($('capture'), async () => {
  video = await recorder.captureGame(); await video.play();
  $('capture-empty').hidden = true; $('capture-badge').textContent = '窗口已连接'; $('capture-badge').classList.add('live'); $('capture-info').textContent = '正在显示所选窗口 · 原版画面';
  video.srcObject?.getVideoTracks()[0]?.addEventListener('ended', clearCapture, { once: true }); updateButtons();
}));
function clearCapture() {
  video = null; context.clearRect(0, 0, canvas.width, canvas.height); $('capture-empty').hidden = false; $('capture-badge').textContent = '未连接窗口'; $('capture-badge').classList.remove('live'); $('capture-info').textContent = '没有采集画面'; updateButtons();
}
$('release').addEventListener('click', () => { recorder.releaseCapture(); clearCapture(); });
$('record-start').addEventListener('click', () => action($('record-start'), async () => { await recorder.start({ source: 'minecraft', video }); }));
$('record-stop').addEventListener('click', () => action($('record-stop'), async () => { await recorder.stop(); await refreshState(); }));
function drawVideo() {
  if (video && video.readyState >= 2) {
    context.fillStyle = '#05080c'; context.fillRect(0, 0, canvas.width, canvas.height);
    const scale = Math.min(canvas.width / video.videoWidth, canvas.height / video.videoHeight);
    const width = video.videoWidth * scale; const height = video.videoHeight * scale;
    context.drawImage(video, (canvas.width - width) / 2, (canvas.height - height) / 2, width, height);
  }
  requestAnimationFrame(drawVideo);
}
drawVideo(); refreshState();
const stream = new EventSource('/api/events');
stream.addEventListener('state', event => { try { acceptState(JSON.parse(event.data)); } catch (error) { notify(error.message, true); } });
stream.onerror = () => {
  online = false; state.players = state.players.map(player => ({ ...player, stale: true }));
  $('connection').textContent = '正在重连'; $('connection').classList.remove('live'); render();
};
window.addEventListener('pagehide', () => { stream.close(); recorder.releaseCapture(); });
