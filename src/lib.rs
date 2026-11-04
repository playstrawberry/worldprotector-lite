//! worldprotector-lite: bounded indexed regions, fail-closed decisions and durable policy snapshots.
#![forbid(unsafe_code)]
mod adapter;
mod commands;
mod lang;
pub mod regions;
mod settings;
mod state;
mod store;

use crate::regions::{MAX_SELECTIONS, Policy, Selections, Throttle};
use crate::state::State;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use strawberry_plugin_api::{
    Context, Player, Plugin, PluginMetadata, Result, logging, register_plugin,
};

pub const WAND_ITEM: &str = "minecraft:stick";
pub const WAND_NAMESPACE: &str = "worldprotector-lite";
pub const WAND_KEY: &str = "wand";
pub const WAND: &str = "worldprotector-lite:wand";
pub const BYPASS: &str = "worldprotector-lite:bypass";

pub struct Shared {
    state: Mutex<State>,
    selections: Mutex<Selections>,
    players: Mutex<BTreeMap<i32, String>>,
    throttle: Mutex<Throttle>,
    refusal_message: AtomicBool,
    subscriptions: Mutex<Subscriptions>,
    started: Instant,
    path: PathBuf,
}
/// What this server has subscribed to so far, and the handle that can add more.
#[derive(Default)]
struct Subscriptions {
    context: Option<Context>,
    done: u16,
}
impl Shared {
    fn new(policy: Option<Policy>, path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::new(policy)),
            selections: Mutex::new(Selections::new()),
            players: Mutex::new(BTreeMap::new()),
            throttle: Mutex::new(Throttle::default()),
            refusal_message: AtomicBool::new(false),
            subscriptions: Mutex::new(Subscriptions::default()),
            started: Instant::now(),
            path,
        })
    }
    /// Makes sure the handlers in `mask` exist. Called before a policy can apply, so that no rule
    /// is ever in force without its event; on failure the caller keeps what it had.
    fn activate(self: &Arc<Self>, mask: u16) -> bool {
        let Ok(mut subs) = self.subscriptions.lock() else {
            return false;
        };
        let missing = mask & !subs.done;
        if missing == 0 {
            return true;
        }
        let done = subs.done;
        let Some(context) = subs.context.as_ref() else {
            return false;
        };
        match adapter::subscribe(context, self, missing, done) {
            Ok(done) => {
                subs.done = done;
                true
            }
            Err(_) => false,
        }
    }
    fn wand_active(&self) -> bool {
        self.subscriptions
            .lock()
            .is_ok_and(|subs| subs.done & adapter::WAND_BIT != 0)
    }
    /// Settings never touch protection: a bad file keeps the line off and says why in the log.
    fn read_settings(&self) {
        let Some(dir) = self.path.parent() else {
            return;
        };
        let refusal_message = settings::load(dir)
            .inspect_err(|problem| logging::log(logging::LogLevel::Warn, problem))
            .is_ok_and(|settings| settings.refusal_message);
        self.refusal_message
            .store(refusal_message, Ordering::Relaxed);
    }
    /// With the setting on: one short line, at most once a second, that never names the region.
    fn refused(&self, p: &Player) {
        if !self.refusal_message.load(Ordering::Relaxed) {
            return;
        }
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if self
            .throttle
            .lock()
            .is_ok_and(|mut t| t.allow(player_key(p), now))
        {
            tell(p, "denied", &[]);
        }
    }
    fn forget(&self, p: &Player) {
        let id = player_id(p);
        let entity = p.as_entity().get_id() as i32;
        if let Ok(mut selections) = self.selections.lock() {
            selections.remove(&id);
        }
        if let Ok(mut players) = self.players.lock() {
            players.remove(&entity);
        }
        if let Ok(mut throttle) = self.throttle.lock() {
            throttle.forget(player_key(p));
        }
    }
    fn flush(self: &Arc<Self>) {
        let (pending, revision, reload) = {
            let Ok(s) = self.state.lock() else {
                return;
            };
            (s.pending.clone(), s.revision, s.reload)
        };
        if let Some(p) = pending {
            let ok = store::write(&self.path, &p).is_ok();
            if let Ok(mut s) = self.state.lock() {
                s.finish_write(revision, ok);
            }
        } else if reload {
            self.read_settings();
            // A loaded policy applies only once its events are subscribed.
            let loaded = store::read(&self.path)
                .ok()
                .filter(|policy| self.activate(policy.needs()))
                .map(Arc::new);
            if let Ok(mut s) = self.state.lock() {
                if let Some(loaded) = loaded {
                    s.active = Some(loaded);
                    s.write_failed = false;
                } else {
                    s.write_failed = true;
                }
                s.reload = false;
                s.scheduled = false;
            }
        }
    }
    fn track(&self, p: &Player) -> bool {
        let id = p.as_entity().get_id() as i32;
        let uuid = player_id(p);
        let Ok(mut players) = self.players.lock() else {
            return false;
        };
        if players.len() >= MAX_SELECTIONS && !players.contains_key(&id) {
            return false;
        }
        players.insert(id, uuid);
        true
    }
    fn player(&self, server: &strawberry_plugin_api::Server, id: i32) -> Option<Player> {
        let uuid = self.players.lock().ok()?.get(&id)?.clone();
        let parsed = strawberry_plugin_api::uuid::parse(&uuid)?;
        server
            .get_player_by_uuid(parsed)
            .filter(|p| p.as_entity().get_id() as i32 == id)
    }
}
fn player_id(p: &Player) -> String {
    let id = p.get_id();
    format!("{:016x}{:016x}", id.high, id.low)
}
fn player_key(p: &Player) -> u128 {
    let id = p.get_id();
    u128::from(id.high) << 64 | u128::from(id.low)
}
fn tell(p: &Player, key: &str, args: &[&str]) {
    p.send_system_message(
        strawberry_plugin_api::text::TextComponent::text(&lang::text(&p.get_locale(), key, args)),
        false,
    );
}
fn holds_wand(p: &Player) -> bool {
    use strawberry_plugin_api::wit::strawberry::plugin::common::Hand;
    p.get_item_in_hand(Hand::Right).is_some_and(|item| {
        item.get_registry_key().trim_start_matches("minecraft:") == "stick"
            && item.has_custom_data(WAND_NAMESPACE, WAND_KEY)
    })
}
struct WorldProtector {
    shared: Mutex<Option<Arc<Shared>>>,
}
impl Plugin for WorldProtector {
    fn new() -> Self {
        Self {
            shared: Mutex::new(None),
        }
    }
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "worldprotector-lite".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Strawberry Interactive LLC".into()],
            description: lang::text("en", "description", &[]),
            dependencies: vec![],
            permissions: vec![
                strawberry_plugin_api::permissions::FS_READ_DATA.into(),
                strawberry_plugin_api::permissions::FS_WRITE_DATA.into(),
            ],
        }
    }
    fn on_load(&self, context: Context) -> Result<()> {
        let path = PathBuf::from(context.get_data_folder()).join("regions.tsv");
        // Invalid storage keeps registered security guards active, rather than unloading protection.
        let policy = store::read(&path).ok();
        let shared = Shared::new(policy, path);
        shared.read_settings();
        let server = context.get_server();
        let players = server.get_all_players();
        for p in players.into_iter().take(MAX_SELECTIONS) {
            shared.track(&p);
        }
        adapter::register_always(&context, &shared)?;
        commands::register(&context, &shared)?;
        let needs = shared.state.lock().map_err(|_| "unavailable")?.needs();
        shared
            .subscriptions
            .lock()
            .map_err(|_| "unavailable")?
            .context = Some(context);
        if !shared.activate(needs) {
            return Err("unavailable".into());
        }
        *self.shared.lock().map_err(|_| "unavailable")? = Some(shared);
        Ok(())
    }
    fn on_unload(&self, _context: Context) -> Result<()> {
        if let Some(shared) = self.shared.lock().map_err(|_| "unavailable")?.take() {
            shared.flush();
        }
        Ok(())
    }
}
register_plugin!(WorldProtector);

#[cfg(test)]
mod context_tests {
    use super::*;
    use crate::regions::Flag;
    #[test]
    fn a_rule_cannot_apply_when_its_event_cannot_be_subscribed() {
        // No server handle in a unit test: the subscription fails, and so must the activation.
        let shared = Shared::new(Some(Policy::default()), PathBuf::new());
        assert!(shared.activate(0));
        assert!(!shared.activate(Flag::Entry.bit()));
    }
}
