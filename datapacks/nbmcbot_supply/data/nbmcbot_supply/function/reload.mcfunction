$item replace entity @s weapon.offhand from entity @s inventory.$(slot)
$execute if items entity @s weapon.offhand minecraft:firework_rocket run item replace entity @s inventory.$(slot) with minecraft:air
