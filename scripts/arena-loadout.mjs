export function duelPlan(env = process.env) {
  const mode = env.NBMCBOT_DUEL_MODE ?? 'mace-vs-spear';
  if (!['mace-vs-mace', 'mace-vs-spear', 'mace-vs-player'].includes(mode)) throw new Error('invalid NBMCBOT_DUEL_MODE');
  const explicitTotal = env.NBMCBOT_DUEL_TOTAL !== undefined;
  const count = Number(env.NBMCBOT_DUEL_COUNT ?? 5);
  if (!explicitTotal && (!Number.isInteger(count) || count < 1 || count > 50)) throw new Error('NBMCBOT_DUEL_COUNT must be an integer from 1 to 50');
  const total = explicitTotal ? Number(env.NBMCBOT_DUEL_TOTAL) : count * 2;
  if (!Number.isInteger(total) || total < 1 || total > 100) throw new Error('NBMCBOT_DUEL_TOTAL must be an integer from 1 to 100');
  const player = env.NBMCBOT_DUEL_PLAYER;
  if (mode === 'mace-vs-player' && !/^[A-Za-z0-9_]{1,16}$/.test(player ?? '')) throw new Error('NBMCBOT_DUEL_PLAYER must be a Minecraft username');
  const groups = [
    { kind: 'mace-team', prefix: 'Mace', team: 'nbmc_mace', style: 'mace', enemy: 'Spear', x: -12, yaw: -90, count: Math.ceil(total / 2) },
    { kind: 'spear-team', prefix: 'Spear', team: 'nbmc_spear', style: mode === 'mace-vs-mace' ? 'mace' : 'spear', enemy: 'Mace', x: 12, yaw: 90, count: Math.floor(total / 2) },
  ];
  if (mode === 'mace-vs-player') for (const group of groups) Object.assign(group, { team: 'nbmc_attackers', enemy: `=${player}` });
  return { mode, player, total, explicitTotal, groups };
}

export function validateGroupNames(group, names, explicitTotal) {
  const pattern = new RegExp(`^${group.prefix}\\d{3}$`);
  if (!Array.isArray(names) || names.length < 1 || names.length > 50 || names.some(name => typeof name !== 'string' || !pattern.test(name)) || new Set(names.map(name => name.toLowerCase())).size !== names.length) throw new Error(`${group.kind}: invalid test-account list in process record`);
  if (explicitTotal && names.length !== group.count) throw new Error(`${group.kind}: expected ${group.count} accounts, found ${names.length}`);
  return names;
}

export function loadoutCommands(plan, groups) {
  const unbreakable = 'minecraft:unbreakable={}';
  const armor = `${unbreakable},minecraft:enchantments={"minecraft:protection":4}`;
  const boots = `${unbreakable},minecraft:enchantments={"minecraft:protection":4,"minecraft:feather_falling":4}`;
  const rocket = 'minecraft:firework_rocket[minecraft:fireworks={flight_duration:1,explosions:[]}] 64';
  const mace = `minecraft:mace[${unbreakable},minecraft:enchantments={"minecraft:wind_burst":3}]`;
  const commands = [];
  function equip(name, style, player = false) {
    commands.push(
      `item replace entity ${name} armor.head with minecraft:netherite_helmet[${armor}]`,
      `item replace entity ${name} armor.legs with minecraft:netherite_leggings[${armor}]`,
      `item replace entity ${name} armor.feet with minecraft:netherite_boots[${boots}]`,
      `item replace entity ${name} armor.chest with minecraft:elytra[${unbreakable}]`,
      `item replace entity ${name} hotbar.0 with ${style === 'mace' ? mace : `minecraft:netherite_spear[${unbreakable}]`}`,
      `item replace entity ${name} weapon.offhand with ${rocket}`,
    );
    if (style === 'mace' || player) commands.push(
      `item replace entity ${name} hotbar.1 with minecraft:wind_charge 64`,
      `item replace entity ${name} hotbar.2 with minecraft:ender_pearl 16`,
      `item replace entity ${name} hotbar.3 with minecraft:netherite_chestplate[${armor}]`,
    );
    if (player) commands.push(`item replace entity ${name} hotbar.4 with minecraft:golden_apple 64`, ...Array.from({ length: 16 }, (_, slot) => `item replace entity ${name} inventory.${slot} with ${rocket}`));
  }
  function heal(name) { commands.push(`effect give ${name} minecraft:instant_health 1 10 true`, `effect give ${name} minecraft:saturation 1 10 true`); }
  const teams = new Set();
  const total = groups.reduce((sum, group) => sum + group.names.length, 0);
  let index = 0;
  for (const group of groups) {
    if (!teams.has(group.team)) {
      teams.add(group.team);
      commands.push(`team add ${group.team}`, `team modify ${group.team} friendlyFire false`, `team modify ${group.team} color ${group.prefix === 'Mace' ? 'red' : 'blue'}`);
    }
    for (const [localIndex, name] of group.names.entries()) {
      let x = group.x, z = (localIndex - (group.names.length - 1) / 2) * 3, yaw = group.yaw;
      if (plan.mode === 'mace-vs-player') {
        const angle = index * 2 * Math.PI / total, radius = Math.max(12, total * 2 / (2 * Math.PI));
        x = Number((Math.cos(angle) * radius).toFixed(3)); z = Number((Math.sin(angle) * radius).toFixed(3)); yaw = Number((angle * 180 / Math.PI + 90).toFixed(3));
      }
      index++;
      commands.push(`tag ${name} add nbmc_duel`, `team join ${group.team} ${name}`, `gamemode survival ${name}`, `clear ${name}`, `effect clear ${name}`);
      equip(name, group.style);
      commands.push(`spawnpoint ${name} ${Math.floor(x)} 64 ${Math.floor(z)} ${yaw} 0`, `tp ${name} ${x} 64 ${z} ${yaw} 0`);
      heal(name);
    }
  }
  if (plan.mode === 'mace-vs-player') {
    const name = plan.player;
    commands.push(`gamemode spectator ${name}`, `execute as ${name} run spectate`, `team leave ${name}`, `tag ${name} remove nbmc_duel`, `gamemode survival ${name}`);
    equip(name, 'spear', true);
    commands.push(`tp ${name} 0 64 0`);
    heal(name);
  }
  return commands;
}
