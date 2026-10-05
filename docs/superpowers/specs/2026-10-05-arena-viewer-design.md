# Arena viewer design

## Goal

User selected original Minecraft imagery and the public repository
`Tlingxie/nbmcbot-arena`. Do not substitute reconstructed 3D imagery.

Provide a local browser control room for the running Java 1.21.11 arena. Track
every bot independently of its current task, follow a selected player, display
live battle views, record a clip, and export a playable MP4. Publish the source
to a new public GitHub repository after live validation.

## User experience

- Dark, legible control room with red/blue teams, a large live viewport, player
  roster, coordinates, health, task state, selected-player camera controls,
  battle events, and recording controls.
- Original-game view uses browser window capture. Selecting the
  Minecraft window is an explicit browser interaction. The user can request
  that their connected client spectate the selected player.
- Record the selected view with a HUD at 1280x720 / 30 FPS. Stream chunks to
  disk, finalize, and export MP4 via local FFmpeg. If FFmpeg is unavailable,
  offer the original browser recording with an accurate format label.
- Include a deliberate start-round action with mace-vs-mace and mace-vs-spear
  choices, a countdown, and the existing finite 1,088-rocket loadout.

## Architecture

Rust emits optional bounded UDP telemetry to 127.0.0.1:4211, enabled by
NBMCBOT_TELEMETRY_ADDR. Every process samples all its sessions at 10 Hz,
including idle, dead, and disconnected bots. Own-player inventory and
observation data are authoritative client observations, not invented state.
Visible remote players can be included with source and freshness information.

A Node.js service on 127.0.0.1:4210 ingests the frames, merges players, tails
arena logs for battle events, and publishes state using Server-Sent Events.
The local server console may supply online human-player positions outside
bot visibility; stale or unavailable observations are explicitly marked.
Read-only status and participant discovery do not change the current match.

The browser shows the original captured Minecraft window, plus a 2D tactical
map from actual telemetry. Selecting a player sends a server spectator command
to the user's explicitly selected client account. A free-view action exits
spectating. It does not claim to render multiple original camera views at once.
Window capture and FFmpeg are optional observer costs, outside the standalone
bot process. No Three.js dependency is required.

## Contracts

UDP frame: {event:"telemetry_frame",version:1,process_id:number,
sampled_at_ms:number,sequence:number,players:[Player]}.
Player: {name,uuid,source:"self"|"observed",position:[x,y,z]|null,
yaw:number|null,pitch:number|null,velocity:[x,y,z]|null,health:number|null,
alive:boolean|null,connected:boolean,dimension:string|null,task:string|null,
style:string|null,phase:string|null,target:string|null,gliding:boolean|null,
equipment:{mainhand:string|null,chest:string|null,offhand:string|null}}.
Additional fields may be ignored. Packet size is bounded below 60 KB;
sequence and process ID identify freshness. Sampling never blocks combat on
network, serialization, disk, or an unavailable viewer.

HTTP: GET /api/state, GET /api/events (SSE), GET /api/recordings;
POST /api/rounds {mode:"mace-vs-mace"|"mace-vs-spear",countdown:0..30};
POST /api/spectate {viewer:string,target:string|null};
POST /api/recordings {mimeType,width,height,fps,source};
PUT /api/recordings/:id/chunks/:sequence (raw bytes);
POST /api/recordings/:id/finish; GET /api/recordings/:id;
GET /api/recordings/:id/download.
Mutations require a same-origin request plus the session token supplied by
/api/state; the server binds loopback and does not expose a remote console.

State: {now,token,server:{address,version,telemetryHz},players:Player[],
events:[],round:{state,mode,countdown,error},recordings:[]}.
Server player entries add team:"red"|"blue"|"observer",updatedAt,stale.

## Publication

Publish source, pinned dependency manifests, license notices, setup and run
instructions. Exclude .runtime, worlds, account caches, local paths, server
configuration, logs, recordings, binaries, and node_modules. Do not publish
Minecraft client or server assets. Create the public repository through the
authenticated GitHub browser and publish the reviewed source through the
connected GitHub API if CLI authentication remains unavailable. The user
explicitly confirmed repository name `nbmcbot-arena` and the current account.

## Validation

Verify telemetry for idle/dead/disconnected sessions and stable identity;
validate UDP input, freshness, source priority, upload sequencing and size
limits, export errors, same-origin mutation checks, and command input
allowlists. Test the real browser UI, camera switching, a fresh 5v5 round,
recording upload, MP4 decoding, and the public repository contents. A synthetic
fixture is a separate demo mode and must never be presented as live evidence.
