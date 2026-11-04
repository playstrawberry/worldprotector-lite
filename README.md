# World Protector Lite

Region protection for Play Strawberry servers. Mark a box in a world, give it owners and members,
and choose per region who may build, break, use blocks, fight, enter and more. Worlds without
regions play normally.

## Regions

- A region is a box between two corners in one world.
- Owners and members are stored by UUID, so a name change keeps their rights.
- Where regions overlap, the one with the highest priority that sets a flag decides; at equal
  priority a deny wins.
- Each world can also have a `__global__` region, a fallback below every other region. It does
  nothing until you set a flag on it.
- Operators and players with `worldprotector-lite:bypass` skip normal denials.

## Flags

Each flag is `allow` (everyone), `deny` (nobody, owners included), `members` (owners and members)
or `unset`.

| Flag | Default in a region | What it guards |
| --- | --- | --- |
| `build` | `members` | placing blocks |
| `break` | `members` | breaking blocks and filling buckets |
| `interact` | `members` | using blocks, containers and entities |
| `pvp` | `allow` | players hitting players, also with projectiles |
| `entry` | `allow` | walking, teleporting or changing world into the region |
| `mob-spawn` | `allow` | creatures spawning |
| `explosions` | `deny` | protected blocks are left out of explosions |
| `fire` | `allow` | burning, fire spread and ignition |
| `liquid-flow` | `allow` | water and lava flowing in |

Some actions cannot be checked yet and are not protected: emptying buckets, pistons, item frames,
paintings and armour stands, pressure plates and hoppers. Do not rely on a flag for those.

## Commands

`/rg`, `/region` and `/worldprotector` are the same command. They act on your current world; use
`/rg world <world> <subcommand>` for another world or from the console.

| Command | What it does |
| --- | --- |
| `/rg wand` | Gives the selection stick: left and right click set the two corners |
| `/rg pos1`, `/rg pos2` | Sets a corner at your feet |
| `/rg define <name>` | Makes a region from your corners; you become its owner |
| `/rg redefine <region>` | Moves a region to your current corners |
| `/rg global` | Makes this world's `__global__` region |
| `/rg setpriority <region> <number>` | Sets the priority |
| `/rg flag <region> <flag> <allow\|deny\|members\|unset>` | Changes one flag |
| `/rg addmember\|removemember <region> <player\|uuid>` | Changes members |
| `/rg addowner\|removeowner <region> <player\|uuid>` | Changes owners |
| `/rg info <region>`, `/rg list [page]` | Shows a region; lists this world's regions |
| `/rg delete <region>` | Deletes a region |
| `/rg status`, `/rg reload` | Shows the save state; reads the regions file again |

Region names are 1 to 32 lowercase letters, digits, `_` or `-`.

## Permissions

| Permission | Default | Use |
| --- | --- | --- |
| `worldprotector-lite:use` | operators (level 2) | the command, list and info; give it to let owners manage their own regions |
| `worldprotector-lite:wand` | operators (level 2) | the wand and corner selection |
| `worldprotector-lite:create` | operators (level 2) | defining new regions |
| `worldprotector-lite:admin` | operators (level 2) | every region, global regions, priority, status, reload |
| `worldprotector-lite:bypass` | operators (level 2) | skips normal denials |

Owners can edit their own regions without `admin`; members cannot.

## Settings

One setting, in `plugins/data/worldprotector-lite/worldprotector-settings.txt`, created on the first
start and read at each start and `/rg reload`:

| Setting | Default | Effect |
| --- | --- | --- |
| `refusal-message` | `off` | `on` tells a refused player so in chat, at most once a second |

Regions are saved in `plugins/data/worldprotector-lite/regions.tsv`. A change is saved in the
background; until it is saved, an action that either the old or the new rules deny stays denied.
Limits: 4096 regions, 64 owners and 64 members per region.

## Languages

English, French, Spanish and Turkish for players (`lang/`); the console uses English.

## Install on a Play Strawberry server

1. Add World Protector Lite to your server in the Play Strawberry panel.
2. Restart the server. The plugin creates `plugins/data/worldprotector-lite/` on the first start.
3. Run `/rg wand` once after each restart before you use a wand you already hold.

Play Strawberry servers load only plugins signed by Play Strawberry, so a file you build yourself
from this source does not load on them.

## Build from source

You need Rust with the `wasm32-wasip2` target and the Strawberry plugin SDK
(`strawberry-plugin-api`), which Play Strawberry gives to its plugin developers.
`Cargo.toml` takes it from the SDK registry named `strawberry`; add that registry to
your Cargo configuration (for example `~/.cargo/config.toml`):

```toml
[registries.strawberry]
index = "sparse+https://sdk.playstrawberry.net/index/"
```

```text
cargo test
cargo build --release --target wasm32-wasip2
```

The plugin file is `target/wasm32-wasip2/release/worldprotector_lite.wasm`.

## License

Copyright Strawberry Interactive LLC. The license text is not published yet; until it is, all
rights are reserved.
