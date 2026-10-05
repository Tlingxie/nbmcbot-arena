export function mapPositions(players, selected, width, height) {
  const points = players.filter(player => Array.isArray(player.position) && player.position.length === 3 && player.position.every(Number.isFinite));
  const focus = points.find(player => player.name === selected);
  const dimension = focus?.dimension ?? points[0]?.dimension;
  const visible = points.filter(player => player.dimension === dimension);
  if (!visible.length) return { points: [], dimension, span: 0 };
  const xs = visible.map(player => player.position[0]);
  const zs = visible.map(player => player.position[2]);
  const centerX = (Math.min(...xs) + Math.max(...xs)) / 2;
  const centerZ = (Math.min(...zs) + Math.max(...zs)) / 2;
  const span = Math.max(24, Math.max(...xs) - Math.min(...xs), Math.max(...zs) - Math.min(...zs)) + 12;
  const scale = Math.min(width, height) / span;
  return { points: visible.map(player => ({ player, x: width / 2 + (player.position[0] - centerX) * scale, y: height / 2 + (player.position[2] - centerZ) * scale })), dimension, span };
}

export function drawMap(canvas, players, selected) {
  const context = canvas.getContext('2d');
  const { width, height } = canvas;
  const layout = mapPositions(players, selected, width, height);
  context.fillStyle = '#0b141e'; context.fillRect(0, 0, width, height);
  context.strokeStyle = '#1a2a39'; context.lineWidth = 1;
  for (let x = 0; x < width; x += 40) { context.beginPath(); context.moveTo(x, 0); context.lineTo(x, height); context.stroke(); }
  for (let y = 0; y < height; y += 40) { context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke(); }
  context.font = '14px system-ui'; context.fillStyle = '#728495'; context.fillText('N  ↑  −Z', 18, 25);
  for (const { player, x, y } of layout.points) {
    context.globalAlpha = player.stale || !player.connected ? 0.35 : 1;
    const color = player.team === 'red' ? '#fb6b78' : player.team === 'blue' ? '#69aaff' : '#a8b7c5';
    if (player.name === selected) { context.strokeStyle = '#ffffff'; context.lineWidth = 2; context.beginPath(); context.arc(x, y, 11, 0, Math.PI * 2); context.stroke(); }
    context.fillStyle = color; context.beginPath(); context.arc(x, y, 5, 0, Math.PI * 2); context.fill();
    if (player.name === selected) { context.fillStyle = '#eef4fa'; context.fillText(player.name, x + 15, y + 4); }
  }
  context.globalAlpha = 1;
  if (!layout.points.length) { context.textAlign = 'center'; context.fillStyle = '#728495'; context.fillText('暂无可用的真实位置', width / 2, height / 2); context.textAlign = 'left'; }
  return layout;
}
