//! The only module adapting event context. Handlers subscribe when a rule needs them, ask the policy
//! before any host call they can avoid, and never enumerate entities or worlds.
use crate::regions::{self, Bounds, Flag, MAX_SELECTIONS, Policy, Verdict};
use crate::{BYPASS, Shared, WAND, holds_wand, player_id, tell};
use std::sync::Arc;
use strawberry_plugin_api::events::extension::{
    BLOCK_DESTROY, ExtensionEvent, ExtensionEventHandler,
};
use strawberry_plugin_api::events::*;
use strawberry_plugin_api::events_wit::*;
use strawberry_plugin_api::wit::strawberry::plugin::common::BlockPos;
use strawberry_plugin_api::{BuildAction, Context, Player, Result, Server, World};

/// The wand's two clicks, beside the nine flags in [`crate::Shared::activate`]'s mask.
pub const WAND_BIT: u16 = 1 << 9;
const INTERACT_EVENT_BIT: u16 = 1 << 10;
/// The build check other plugins ask (`player.can_build`), beside the place, break and use flags.
const BUILD_CHECK_BIT: u16 = 1 << 11;
/// The block the world destroys with no player behind it ("block-destroy"), beside the break flag.
const DESTROY_EVENT_BIT: u16 = 1 << 12;
/// A target is never further from the player than this; the server checks the reach itself.
const REACH: i32 = 16;

fn options() -> HandlerOptions {
    HandlerOptions {
        // After plugins that rule on their own state, before those that read the final result.
        priority: HandlerPriority::High,
        blocking: true,
        ignore_cancelled: true,
        on_failure: FailureVerdict::Deny,
    }
}
/// Joining and leaving keep the bounded player table, and cost one call per join or leave.
pub fn register_always(context: &Context, shared: &Arc<Shared>) -> Result<()> {
    let guard = || Guard(shared.clone());
    context.register_event_handler_with_options::<PlayerJoinEvent, _>(guard(), options())?;
    context.register_event_handler_with_options::<PlayerLeaveEvent, _>(
        guard(),
        HandlerOptions {
            ignore_cancelled: false,
            on_failure: FailureVerdict::Allow,
            ..options()
        },
    )?;
    Ok(())
}
/// Adds the handlers for `missing`; returns the bits now done. There is no way to remove a
/// handler, so one added stays until the next start.
pub fn subscribe(context: &Context, shared: &Arc<Shared>, missing: u16, done: u16) -> Result<u16> {
    let mut added = 0;
    macro_rules! on {
        ($bit:expr => $($event:ty),+ $(,)?) => {
            if missing & $bit != 0 {
                $(context.register_event_handler_with_options::<$event, _>(
                    Guard(shared.clone()), options())?;)+
                added |= $bit;
            }
        };
    }
    on!(Flag::Build.bit() => BlockPlaceEvent, BlockMultiPlaceEvent);
    on!(Flag::Break.bit() => BlockBreakEvent, PlayerBucketFillEvent);
    on!(Flag::Interact.bit() => PlayerInteractEntityEvent);
    on!(Flag::Pvp.bit() => EntityDamageByEntityEvent);
    on!(Flag::Entry.bit() => PlayerMoveEvent, PlayerTeleportEvent, PlayerChangeWorldEvent);
    on!(Flag::MobSpawn.bit() => CreatureSpawnEvent, EntitySpawnEvent);
    on!(Flag::Explosions.bit() => EntityExplodeEvent, BlockExplodeEvent);
    on!(Flag::Fire.bit() => BlockBurnEvent, BlockSpreadEvent, BlockIgniteEvent);
    on!(Flag::LiquidFlow.bit() => BlockFromToEvent);
    on!(WAND_BIT => BlockDamageEvent);
    // Block use and the wand's second corner share one handler.
    if (missing | done) & (Flag::Interact.bit() | WAND_BIT) != 0 && done & INTERACT_EVENT_BIT == 0 {
        context.register_event_handler_with_options::<PlayerInteractEvent, _>(
            Guard(shared.clone()),
            options(),
        )?;
        added |= INTERACT_EVENT_BIT;
    }
    // A block the world destroys (a plant that pops, a fluid that replaces it) is a break with no
    // player: the break flag rules on it through the block-destroy event, not the block break one.
    if (missing | done) & Flag::Break.bit() != 0 && done & DESTROY_EVENT_BIT == 0 {
        context.register_extension_event_handler(
            BLOCK_DESTROY,
            Guard(shared.clone()),
            options(),
        )?;
        added |= DESTROY_EVENT_BIT;
    }
    // Things other plugins put into the world (furniture, its barriers and light) ask the same
    // build, break and interact rules through the server's build check.
    let checked = Flag::Build.bit() | Flag::Break.bit() | Flag::Interact.bit();
    if (missing | done) & checked != 0 && done & BUILD_CHECK_BIT == 0 {
        let guard = Guard(shared.clone());
        context.register_build_check_handler(options().priority, move |_server, check| {
            let pos = coords(check.event.pos);
            !guard.at(
                &check.event.world.get_name(),
                pos,
                check_flag(check.event.action),
                Some(&check.player),
            )
        })?;
        added |= BUILD_CHECK_BIT;
    }
    Ok(done | added)
}

/// The flag a build check asks: the one the block place, block break and block or entity use
/// handlers ask for the same action.
pub const fn check_flag(action: BuildAction) -> Flag {
    match action {
        BuildAction::Place => Flag::Build,
        BuildAction::Break => Flag::Break,
        BuildAction::Use => Flag::Interact,
    }
}

fn coords(p: BlockPos) -> [i32; 3] {
    [p.x, p.y, p.z]
}
/// The event's own world, which costs less than asking the player for theirs.
fn world_of(p: &Player) -> String {
    event_world().map_or_else(|| p.get_world().get_name(), |w| w.get_name())
}

#[derive(Clone)]
struct Guard(Arc<Shared>);
impl Guard {
    /// Should this action be refused? A world with no region answers after one lookup. The player's
    /// identity is fetched only for a refusal, because no identity can be allowed more than one.
    fn refuses(
        &self,
        world: &str,
        p: Option<&Player>,
        ask: impl Fn(&Policy, Option<&str>) -> Verdict,
    ) -> bool {
        let Ok(state) = self.0.state.lock() else {
            return true;
        };
        if !state.in_scope(world) {
            return false;
        }
        let first = state.evaluate(world, |policy| ask(policy, None));
        drop(state);
        if first == Verdict::Allow {
            return false;
        }
        let Some(p) = p else {
            return true;
        };
        let id = player_id(p);
        let Ok(state) = self.0.state.lock() else {
            return true;
        };
        let second = state.evaluate(world, |policy| ask(policy, Some(&id)));
        drop(state);
        match second {
            Verdict::Allow => false,
            Verdict::Deny => !p.has_permission(BYPASS),
            Verdict::Unavailable => true,
        }
    }
    /// The decision of every block handler and of the build check: whether `flag` is refused at
    /// `pos` for `p`. It only answers; `player_at` adds the refusal line for an event.
    fn at(&self, world: &str, pos: [i32; 3], flag: Flag, p: Option<&Player>) -> bool {
        self.refuses(world, p, |policy, id| policy.decide(world, pos, flag, id))
    }
    /// Whether the world may destroy the block at `pos` with no player behind it: the break flag
    /// with no player (a protected cuboid keeps its plants, and no fluid replaces its blocks).
    fn destroy_refused(&self, world: &str, pos: [i32; 3]) -> bool {
        self.at(world, pos, Flag::Break, None)
    }
    fn player_at(&self, world: &str, pos: [i32; 3], flag: Flag, p: &Player) -> bool {
        let refused = self.at(world, pos, flag, Some(p));
        if refused {
            self.0.refused(p);
        }
        refused
    }
    fn entry(&self, world: &str, from: Option<[i32; 3]>, to: Option<[i32; 3]>, p: &Player) -> bool {
        let Some(to) = to else {
            return false;
        };
        let refused = self.refuses(world, Some(p), |policy, id| {
            policy.entry(world, from, to, id)
        });
        if refused {
            self.0.refused(p);
        }
        refused
    }
    fn explosion(&self) {
        // The server names the world of every explosion.
        let Some(world) = event_world().map(|w| w.get_name()) else {
            return;
        };
        if self
            .0
            .state
            .lock()
            .is_ok_and(|state| !state.in_scope(&world))
        {
            return;
        }
        let blocks: Vec<[i32; 3]> = explosion_blocks().into_iter().map(coords).collect();
        let keep = self
            .0
            .state
            .lock()
            .map_or_else(|_| Vec::new(), |s| s.explosion_keeps(&world, &blocks));
        if keep.len() != blocks.len() {
            let list: Vec<BlockPos> = keep
                .into_iter()
                .map(|[x, y, z]| BlockPos { x, y, z })
                .collect();
            set_explosion_blocks(&list);
        }
    }
    fn select(&self, p: &Player, first: bool, pos: [i32; 3]) {
        if !regions::valid_pos(pos) {
            return;
        }
        let world = p.get_world().get_name();
        let id = player_id(p);
        let saved = self.0.selections.lock().is_ok_and(|mut selections| {
            let room = selections.len() < MAX_SELECTIONS || selections.contains_key(&id);
            if room {
                selections.entry(id).or_default().set(&world, first, pos);
            }
            room
        });
        if saved {
            let (corner, at) = (
                if first { "1" } else { "2" },
                format!("{} {} {}", pos[0], pos[1], pos[2]),
            );
            tell(p, "corner", &[corner, &at]);
        } else {
            tell(p, "limit", &[]);
        }
    }
    fn wand_active(&self) -> bool {
        self.0.wand_active()
    }
    /// For an event that names no world: refuse when a loaded world has a cuboid refusing it here,
    /// and `happens_in` finds the action could be taking place in that world.
    fn without_world(
        &self,
        server: &Server,
        pos: BlockPos,
        flag: Flag,
        happens_in: impl Fn(&World) -> bool,
    ) -> bool {
        let worlds = self
            .0
            .state
            .lock()
            .ok()
            .and_then(|state| state.refusing_worlds(coords(pos), flag));
        worlds.is_none_or(|worlds| {
            worlds.iter().any(|name| {
                server
                    .get_world_by_name(name)
                    .is_some_and(|world| happens_in(&world))
            })
        })
    }
    /// Use of an entity: its exact place when the server can name it, otherwise every region the
    /// player could have reached it in. Only a real use counts, an attack is not a use.
    fn entity_use(&self, p: &Player, entity_id: i32) -> bool {
        let world = p.get_world().get_name();
        if self.0.state.lock().is_ok_and(|s| !s.in_scope(&world)) {
            return false;
        }
        let position = p
            .as_entity()
            .get_target_entity(f64::from(REACH))
            .filter(|target| target.get_id() as i32 == entity_id)
            .map(|target| target.get_position());
        let refused = if let Some(pos) = position.and_then(regions::block_position) {
            self.at(&world, pos, Flag::Interact, Some(p))
        } else {
            let Some(at) = regions::block_position(p.get_position()) else {
                return true;
            };
            let area = Bounds {
                min: at.map(|v| v - REACH),
                max: at.map(|v| v + REACH),
            };
            self.refuses(&world, Some(p), |policy, id| {
                policy.decide_within(&world, area, Flag::Interact, id)
            })
        };
        if refused {
            self.0.refused(p);
        }
        refused
    }
}
impl EventHandler<PlayerJoinEvent> for Guard {
    fn handle(&self, _s: Server, e: PlayerJoinEventData) -> PlayerJoinEventData {
        self.0.track(&e.player);
        e
    }
}
impl EventHandler<PlayerLeaveEvent> for Guard {
    fn handle(&self, _s: Server, e: PlayerLeaveEventData) -> PlayerLeaveEventData {
        self.0.forget(&e.player);
        e
    }
}
impl EventHandler<BlockBreakEvent> for Guard {
    fn handle(&self, _s: Server, mut e: BlockBreakEventData) -> BlockBreakEventData {
        if let Some(p) = e.player.as_ref() {
            let flag = check_flag(BuildAction::Break);
            e.cancelled |= self.player_at(&world_of(p), coords(e.block_pos), flag, p);
        } else if let Some(world) = event_world() {
            e.cancelled |= self.destroy_refused(&world.get_name(), coords(e.block_pos));
        }
        e
    }
}
impl ExtensionEventHandler for Guard {
    /// The world destroys a block with no player behind it ("block-destroy"). Fails closed: an
    /// event that cannot be read is refused.
    fn handle(&self, _s: Server, event: &ExtensionEvent) {
        let refused = event
            .block_destroy()
            .is_none_or(|d| self.destroy_refused(&d.world.get_name(), coords(d.pos)));
        if refused {
            event.set_cancelled(true);
        }
    }
}
impl EventHandler<BlockDamageEvent> for Guard {
    fn handle(&self, _s: Server, mut e: BlockDamageEventData) -> BlockDamageEventData {
        // The damage event comes first and its cancel stops the break, in creative mode too.
        if holds_wand(&e.player) {
            e.cancelled = true;
            if e.player.has_permission(WAND) {
                self.select(&e.player, true, coords(e.block_pos));
            }
        }
        e
    }
}
impl EventHandler<BlockPlaceEvent> for Guard {
    fn handle(&self, _s: Server, mut e: BlockPlaceEventData) -> BlockPlaceEventData {
        e.cancelled |= self.player_at(
            &world_of(&e.player),
            coords(e.block_pos),
            check_flag(BuildAction::Place),
            &e.player,
        );
        e
    }
}
impl EventHandler<BlockMultiPlaceEvent> for Guard {
    fn handle(&self, _s: Server, mut e: BlockMultiPlaceEventData) -> BlockMultiPlaceEventData {
        let world = e.target_world.get_name();
        let blocks: Vec<[i32; 3]> = e.placed_blocks.iter().map(|(p, _)| coords(*p)).collect();
        let refused = self.refuses(&world, Some(&e.player), |policy, id| {
            if blocks.len() > crate::state::MAX_BULK {
                return Verdict::Unavailable;
            }
            let (mut verdict, mut visits) = (Verdict::Allow, 0);
            for pos in &blocks {
                let (v, cost) = policy.decide_counted(&world, *pos, Flag::Build, id);
                visits += cost;
                if visits > crate::state::MAX_BULK_VISITS || v == Verdict::Unavailable {
                    return Verdict::Unavailable;
                }
                if v == Verdict::Deny {
                    verdict = v;
                }
            }
            verdict
        });
        if refused {
            e.cancelled = true;
            self.0.refused(&e.player);
        }
        e
    }
}
impl EventHandler<PlayerInteractEvent> for Guard {
    fn handle(&self, _s: Server, mut e: PlayerInteractEventData) -> PlayerInteractEventData {
        // A click in the air, or a hit on a block, is not the use of a block.
        if e.action != InteractAction::RightClickBlock {
            return e;
        }
        let Some(pos) = e.clicked_pos.map(coords) else {
            return e;
        };
        if self.wand_active() && holds_wand(&e.player) {
            e.cancelled = true;
            if e.player.has_permission(WAND) {
                self.select(&e.player, false, pos);
            }
            return e;
        }
        let flag = check_flag(BuildAction::Use);
        e.cancelled |= self.player_at(&world_of(&e.player), pos, flag, &e.player);
        e
    }
}
impl EventHandler<PlayerInteractEntityEvent> for Guard {
    fn handle(
        &self,
        _s: Server,
        mut e: PlayerInteractEntityEventData,
    ) -> PlayerInteractEntityEventData {
        if !matches!(e.action, EntityInteractionAction::Attack) {
            e.cancelled |= self.entity_use(&e.player, e.entity_id);
        }
        e
    }
}
impl EventHandler<EntityDamageByEntityEvent> for Guard {
    fn handle(
        &self,
        s: Server,
        mut e: EntityDamageByEntityEventData,
    ) -> EntityDamageByEntityEventData {
        let Some(victim) = self.0.player(&s, e.entity_id) else {
            return e;
        };
        let attacker_id = damage_projectile_shooter().unwrap_or(e.damager_id);
        // Cannot tell an unresolved player projectile from a non-player attacker.
        let Some(attacker) = self.0.player(&s, attacker_id) else {
            return e;
        };
        let endpoints = [&victim, &attacker];
        let refused = endpoints.into_iter().any(|p| {
            regions::block_position(p.get_position()).is_some_and(|pos| {
                self.at(&p.get_world().get_name(), pos, Flag::Pvp, Some(&attacker))
            })
        });
        if refused {
            e.cancelled = true;
            self.0.refused(&attacker);
        }
        e
    }
}
impl EventHandler<EntityExplodeEvent> for Guard {
    fn handle(&self, _s: Server, e: EntityExplodeEventData) -> EntityExplodeEventData {
        self.explosion();
        e
    }
}
impl EventHandler<BlockExplodeEvent> for Guard {
    fn handle(&self, _s: Server, e: BlockExplodeEventData) -> BlockExplodeEventData {
        self.explosion();
        e
    }
}
impl EventHandler<CreatureSpawnEvent> for Guard {
    fn handle(&self, _s: Server, mut e: CreatureSpawnEventData) -> CreatureSpawnEventData {
        if let Some(pos) = regions::block_position(e.position) {
            e.cancelled |= self.at(&e.target_world.get_name(), pos, Flag::MobSpawn, None);
        }
        e
    }
}
impl EventHandler<EntitySpawnEvent> for Guard {
    fn handle(&self, _s: Server, mut e: EntitySpawnEventData) -> EntitySpawnEventData {
        // The place first: it answers most spawns without looking at what is spawning.
        if let Some(pos) = regions::block_position(e.position)
            && self.at(&e.target_world.get_name(), pos, Flag::MobSpawn, None)
        {
            e.cancelled |= regions::is_mob(&e.entity_type);
        }
        e
    }
}
impl EventHandler<BlockBurnEvent> for Guard {
    fn handle(&self, _s: Server, mut e: BlockBurnEventData) -> BlockBurnEventData {
        e.cancelled |= self.at(
            &e.target_world.get_name(),
            coords(e.block_pos),
            Flag::Fire,
            None,
        );
        e
    }
}
impl EventHandler<BlockSpreadEvent> for Guard {
    fn handle(&self, _s: Server, mut e: BlockSpreadEventData) -> BlockSpreadEventData {
        let world = e.target_world.get_name();
        if self.at(&world, coords(e.target_pos), Flag::Fire, None)
            || self.at(&world, coords(e.source_pos), Flag::Fire, None)
        {
            // Grass and vines spread here too. A state we cannot name is treated as fire.
            e.cancelled |= strawberry_plugin_api::world::get_block_from_state_id(e.new_state_id)
                .is_none_or(|block| {
                    matches!(
                        block.name.trim_start_matches("minecraft:"),
                        "fire" | "soul_fire"
                    )
                });
        }
        e
    }
}
impl EventHandler<BlockIgniteEvent> for Guard {
    fn handle(&self, s: Server, mut e: BlockIgniteEventData) -> BlockIgniteEventData {
        e.cancelled |= self.without_world(&s, e.block_pos, Flag::Fire, |_| true);
        e
    }
}
impl EventHandler<BlockFromToEvent> for Guard {
    fn handle(&self, s: Server, mut e: BlockFromToEventData) -> BlockFromToEventData {
        let to = e.to_pos;
        e.cancelled |= self.without_world(&s, to, Flag::LiquidFlow, |world| {
            // Liquid comes from above or the side. It can also sit inside a block that is not a
            // full cube, so only air and full cubes rule a neighbour out.
            [(0, 1, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)]
                .into_iter()
                .any(|(x, y, z)| {
                    let state = world.get_block_state(BlockPos {
                        x: to.x + x,
                        y: to.y + y,
                        z: to.z + z,
                    });
                    state.is_liquid || !(state.is_air || state.is_full_cube)
                })
        });
        e
    }
}
impl EventHandler<PlayerMoveEvent> for Guard {
    fn handle(&self, _s: Server, mut e: PlayerMoveEventData) -> PlayerMoveEventData {
        let world = e.player.get_world().get_name();
        let (from, to) = (e.from_position, e.to_position);
        if self.refuses(&world, Some(&e.player), |policy, id| {
            policy.entry_path(&world, from, to, id)
        }) {
            e.cancelled = true;
            self.0.refused(&e.player);
        }
        e
    }
}
impl EventHandler<PlayerTeleportEvent> for Guard {
    fn handle(&self, _s: Server, mut e: PlayerTeleportEventData) -> PlayerTeleportEventData {
        e.cancelled |= self.entry(
            &e.player.get_world().get_name(),
            regions::block_position(e.from_position),
            regions::block_position(e.to_position),
            &e.player,
        );
        e
    }
}
impl EventHandler<PlayerChangeWorldEvent> for Guard {
    fn handle(&self, _s: Server, mut e: PlayerChangeWorldEventData) -> PlayerChangeWorldEventData {
        e.cancelled |= self.entry(
            &e.new_world.get_name(),
            None,
            regions::block_position(e.position),
            &e.player,
        );
        e
    }
}
impl EventHandler<PlayerBucketFillEvent> for Guard {
    fn handle(&self, _s: Server, mut e: PlayerBucketFillEventData) -> PlayerBucketFillEventData {
        e.cancelled |= self.player_at(
            &e.player.get_world().get_name(),
            coords(e.block_pos),
            Flag::Break,
            &e.player,
        );
        e
    }
}

#[cfg(test)]
mod build_check_tests {
    use super::*;
    use crate::regions::Region;

    #[test]
    fn a_build_check_asks_the_flag_its_block_event_asks() {
        assert_eq!(check_flag(BuildAction::Place), Flag::Build);
        assert_eq!(check_flag(BuildAction::Break), Flag::Break);
        assert_eq!(check_flag(BuildAction::Use), Flag::Interact);
    }

    #[test]
    fn a_protected_cuboid_refuses_a_block_destroy_as_a_break_with_no_player() {
        let region = Region::new(
            "spawn",
            "world",
            Bounds {
                min: [0, 60, 0],
                max: [10, 80, 10],
            },
            Some("00000000000000000000000000000001".into()),
        );
        let policy = Policy::build(vec![region]).unwrap();
        let guard = Guard(Shared::new(Some(policy), std::path::PathBuf::new()));
        // Inside, the break flag refuses the world's destroy: the plant stays, the block stays.
        assert!(guard.destroy_refused("world", [5, 70, 5]));
        // It is the very question the block break handler asks of a break with no player.
        assert_eq!(
            guard.destroy_refused("world", [5, 70, 5]),
            guard.at("world", [5, 70, 5], Flag::Break, None)
        );
        // Outside, or in another world: allowed.
        assert!(!guard.destroy_refused("world", [50, 70, 5]));
        assert!(!guard.destroy_refused("nether", [5, 70, 5]));
    }

    #[test]
    fn a_protected_cuboid_refuses_the_check_as_it_refuses_the_event() {
        let region = Region::new(
            "spawn",
            "world",
            Bounds {
                min: [0, 60, 0],
                max: [10, 80, 10],
            },
            Some("00000000000000000000000000000001".into()),
        );
        let policy = Policy::build(vec![region]).unwrap();
        let guard = Guard(Shared::new(Some(policy), std::path::PathBuf::new()));
        for action in [BuildAction::Place, BuildAction::Break, BuildAction::Use] {
            // Inside: refused without a player who may (no identity, no bypass).
            assert!(
                guard.at("world", [5, 70, 5], check_flag(action), None),
                "{action:?}"
            );
            // Outside, or in another world: allowed.
            assert!(
                !guard.at("world", [50, 70, 5], check_flag(action), None),
                "{action:?}"
            );
            assert!(
                !guard.at("nether", [5, 70, 5], check_flag(action), None),
                "{action:?}"
            );
        }
    }
}
