//! One command implementation shared by current-world and explicit-world forms.
use crate::regions::{self, Bounds, Flag, GLOBAL, MAX_PEOPLE, MAX_SELECTIONS, Region, Rule};
use crate::{BYPASS, Shared, WAND, WAND_ITEM, WAND_KEY, WAND_NAMESPACE, adapter, lang, player_id};
use std::sync::Arc;
use strawberry_plugin_api::command::{
    Arg, ArgumentType, Command, CommandError, CommandNode, CommandSender, ConsumedArgs, StringType,
};
use strawberry_plugin_api::commands::CommandHandler;
use strawberry_plugin_api::permission::PermissionLevel;
use strawberry_plugin_api::permission::{Permission, PermissionDefault};
use strawberry_plugin_api::scheduler::SchedulerExt;
use strawberry_plugin_api::text::TextComponent;
use strawberry_plugin_api::wit::strawberry::plugin::common::{NbtEntry, NbtTag, NbtTree};
use strawberry_plugin_api::{Context, ItemStack, Player, Result, Server};

const USE: &str = "worldprotector-lite:use";
const ADMIN: &str = "worldprotector-lite:admin";
const CREATE: &str = "worldprotector-lite:create";

pub fn register(context: &Context, shared: &Arc<Shared>) -> Result<()> {
    for (node, default) in [
        // Hidden from players unless the server grants it, for example to region owners.
        (USE, PermissionDefault::Op(PermissionLevel::Two)),
        (ADMIN, PermissionDefault::Op(PermissionLevel::Two)),
        (CREATE, PermissionDefault::Op(PermissionLevel::Two)),
        (WAND, PermissionDefault::Op(PermissionLevel::Two)),
        (BYPASS, PermissionDefault::Op(PermissionLevel::Two)),
    ] {
        context.register_permission(&Permission {
            node: node.into(),
            description: lang::text("en", "permission", &[]),
            default,
            children: vec![],
        })?;
    }
    let mut root = Command::new(
        &["rg".into(), "region".into(), "worldprotector".into()],
        &lang::text("en", "description", &[]),
    )
    .execute(Cmd(shared.clone(), "help"));
    for node in nodes(shared) {
        root = root.then(node);
    }
    let mut explicit = CommandNode::argument("world", &word());
    for node in nodes(shared) {
        explicit = explicit.then(node);
    }
    root = root.then(CommandNode::literal("world").then(explicit));
    context.register_command(root, USE);
    Ok(())
}
fn word() -> ArgumentType {
    ArgumentType::String(StringType::SingleWord)
}
fn nodes(s: &Arc<Shared>) -> Vec<CommandNode> {
    let mut out = Vec::new();
    for action in ["wand", "pos1", "pos2", "global", "status", "reload"] {
        out.push(CommandNode::literal(action).execute(Cmd(s.clone(), action)));
    }
    let list = CommandNode::literal("list")
        .execute(Cmd(s.clone(), "list"))
        .then(CommandNode::argument("page", &word()).execute(Cmd(s.clone(), "list")));
    out.push(list);
    for action in [
        "define",
        "delete",
        "info",
        "redefine",
        "setpriority",
        "flag",
        "addmember",
        "removemember",
        "addowner",
        "removeowner",
    ] {
        let mut node = CommandNode::argument("region", &word());
        node = match action {
            "flag" => node.then(
                CommandNode::argument("flag", &word())
                    .then(CommandNode::argument("value", &word()).execute(Cmd(s.clone(), action))),
            ),
            "setpriority" => {
                node.then(CommandNode::argument("value", &word()).execute(Cmd(s.clone(), action)))
            }
            "addmember" | "removemember" | "addowner" | "removeowner" => {
                node.then(CommandNode::argument("player", &word()).execute(Cmd(s.clone(), action)))
            }
            _ => node.execute(Cmd(s.clone(), action)),
        };
        out.push(CommandNode::literal(action).then(node));
    }
    out
}
fn locale(s: &CommandSender) -> String {
    s.as_player()
        .map_or_else(|| "en".into(), |p| p.get_locale())
}
fn say(s: &CommandSender, key: &str, args: &[&str]) {
    s.send_message(TextComponent::text(&lang::text(&locale(s), key, args)));
}
fn fail(language: &str, key: &str) -> CommandError {
    CommandError::CommandFailed(TextComponent::text(&lang::text(language, key, &[])))
}
fn arg(args: &ConsumedArgs, key: &str) -> Option<String> {
    match args.get_value(key) {
        Arg::Simple(v) if !v.is_empty() && v.len() <= 128 => Some(v),
        _ => None,
    }
}
fn can_manage(admin: bool, actor: Option<&str>, r: &Region) -> bool {
    admin || actor.is_some_and(|id| r.owners.contains(id))
}
fn may_overlap(admin: bool, actor: Option<&str>, candidate: &Region, regions: &[Region]) -> bool {
    admin
        || regions.iter().all(|r| {
            r.world != candidate.world
                || r.name == candidate.name
                || r.name != GLOBAL && !r.bounds.intersects(candidate.bounds)
                || actor.is_some_and(|id| r.owners.contains(id))
        })
}
fn player_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
fn give_wand(p: &Player) -> bool {
    for slot in 0..36u8 {
        if p.get_inventory_item(slot).is_none() {
            let wand = ItemStack::new(WAND_ITEM, 1);
            wand.set_custom_name(Some(TextComponent::text(&lang::text(
                &p.get_locale(),
                "wand_name",
                &[],
            ))));
            wand.set_custom_data(
                WAND_NAMESPACE,
                WAND_KEY,
                &NbtTree {
                    root: 0,
                    tags: vec![
                        NbtTag::Compound(vec![NbtEntry {
                            key: WAND_KEY.into(),
                            value: 1,
                        }]),
                        NbtTag::Byte(1),
                    ],
                },
            );
            p.set_inventory_item(slot, Some(wand));
            return true;
        }
    }
    false
}
struct Cmd(Arc<Shared>, &'static str);
impl CommandHandler for Cmd {
    fn handle(
        &self,
        sender: CommandSender,
        server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let language = locale(&sender);
        if self.1 == "help" {
            say(&sender, "usage", &[]);
            return Ok(1);
        }
        let player = sender.as_player();
        let actor = player.as_ref().map(player_id);
        let admin = sender.has_permission(&server, ADMIN);
        if matches!(self.1, "status" | "reload") {
            if !admin {
                return Err(fail(&language, "forbidden"));
            }
            if self.1 == "status" {
                let (key, count, rev) = {
                    let s = self
                        .0
                        .state
                        .lock()
                        .map_err(|_| fail(&language, "unavailable"))?;
                    (
                        if s.write_failed {
                            "storage"
                        } else if s.pending.is_some() {
                            "pending"
                        } else if s.ready() {
                            "ready"
                        } else {
                            "unavailable"
                        },
                        s.active.as_ref().map_or(0, |p| p.regions().len()),
                        s.revision,
                    )
                };
                say(
                    &sender,
                    "status",
                    &[
                        &lang::text(&locale(&sender), key, &[]),
                        &count.to_string(),
                        &rev.to_string(),
                    ],
                );
            } else {
                {
                    let mut s = self
                        .0
                        .state
                        .lock()
                        .map_err(|_| fail(&language, "unavailable"))?;
                    if s.scheduled {
                        return Err(fail(&language, "unavailable"));
                    }
                    s.scheduled = true;
                    if s.pending.is_none() {
                        s.reload = true;
                    }
                }
                let shared = self.0.clone();
                server.schedule_delayed_async_task(1, move |_| shared.flush());
                say(&sender, "reload", &[]);
            }
            return Ok(1);
        }
        let explicit_world = arg(&args, "world");
        let world = explicit_world
            .clone()
            .or_else(|| player.as_ref().map(|p| p.get_world().get_name()))
            .ok_or_else(|| fail(&language, "world"))?;
        if !regions::valid_world(&world)
            || explicit_world.is_some() && server.get_world_by_name(&world).is_none()
        {
            return Err(fail(&language, "world"));
        }
        if matches!(self.1, "wand" | "pos1" | "pos2") {
            let p = player
                .as_ref()
                .ok_or_else(|| fail(&language, "player_only"))?;
            if !p.has_permission(WAND) {
                return Err(fail(&language, "forbidden"));
            }
            if self.1 == "wand" {
                // The clicks of a wand need handlers nobody else should pay for.
                if !self.0.activate(adapter::WAND_BIT) {
                    return Err(fail(&language, "unavailable"));
                }
                if !give_wand(p) {
                    return Err(fail(&language, "full"));
                }
                say(&sender, "wand", &[]);
            } else {
                if world != p.get_world().get_name() {
                    return Err(fail(&language, "world"));
                }
                let pos = regions::block_position(p.get_position())
                    .ok_or_else(|| fail(&language, "invalid"))?;
                let id = actor
                    .as_ref()
                    .ok_or_else(|| fail(&language, "player_only"))?;
                {
                    let mut sels = self
                        .0
                        .selections
                        .lock()
                        .map_err(|_| fail(&language, "unavailable"))?;
                    if sels.len() >= MAX_SELECTIONS && !sels.contains_key(id) {
                        return Err(fail(&language, "limit"));
                    }
                    sels.entry(id.clone())
                        .or_default()
                        .set(&world, self.1 == "pos1", pos);
                }
                say(
                    &sender,
                    "corner",
                    &[if self.1 == "pos1" { "1" } else { "2" }, &fmt(pos)],
                );
            }
            return Ok(1);
        }
        if matches!(self.1, "list" | "info") {
            let policy = {
                let s = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| fail(&language, "unavailable"))?;
                if !s.ready() {
                    return Err(fail(&language, "unavailable"));
                }
                s.active
                    .clone()
                    .ok_or_else(|| fail(&language, "unavailable"))?
            };
            if self.1 == "info" {
                let name = arg(&args, "region")
                    .ok_or_else(|| fail(&language, "invalid"))?
                    .to_ascii_lowercase();
                let Some(r) = policy.region(&world, &name) else {
                    say(&sender, "not_found", &[&name]);
                    return Ok(0);
                };
                say(
                    &sender,
                    "info",
                    &[
                        &r.name,
                        &r.world,
                        &fmt(r.bounds.min),
                        &fmt(r.bounds.max),
                        &r.priority.to_string(),
                        &r.owners.iter().cloned().collect::<Vec<_>>().join(","),
                        &r.members.iter().cloned().collect::<Vec<_>>().join(","),
                    ],
                );
                for f in Flag::ALL {
                    say(&sender, "flag_row", &[f.name(), r.flags[f as usize].name()]);
                }
            } else {
                let page = arg(&args, "page")
                    .map_or(Ok(1usize), |p| p.parse())
                    .map_err(|_| fail(&language, "invalid"))?;
                let all: Vec<_> = policy
                    .regions()
                    .iter()
                    .filter(|r| r.world == world)
                    .collect();
                let pages = all.len().div_ceil(10).max(1);
                if page == 0 || page > pages {
                    return Err(fail(&language, "invalid"));
                }
                say(
                    &sender,
                    "list",
                    &[&world, &page.to_string(), &pages.to_string()],
                );
                for r in all.iter().skip((page - 1) * 10).take(10) {
                    say(&sender, "list_row", &[&r.name, &r.priority.to_string()]);
                }
            }
            return Ok(1);
        }
        let name = if self.1 == "global" {
            GLOBAL.into()
        } else {
            arg(&args, "region")
                .ok_or_else(|| fail(&language, "invalid"))?
                .to_ascii_lowercase()
        };
        if !regions::valid_name(&name) && name != GLOBAL {
            return Err(fail(&language, "invalid"));
        }
        if (matches!(self.1, "global" | "setpriority") || name == GLOBAL) && !admin {
            return Err(fail(&language, "forbidden"));
        }
        if self.1 == "define" && !sender.has_permission(&server, CREATE) {
            return Err(fail(&language, "forbidden"));
        }
        let selection = if matches!(self.1, "define" | "redefine") {
            let id = actor
                .as_ref()
                .ok_or_else(|| fail(&language, "player_only"))?;
            let sels = self
                .0
                .selections
                .lock()
                .map_err(|_| fail(&language, "unavailable"))?;
            Some(
                sels.get(id)
                    .filter(|s| s.world == world)
                    .and_then(|s| s.bounds())
                    .ok_or_else(|| fail(&language, "selection"))?,
            )
        } else {
            None
        };
        let flag = arg(&args, "flag").and_then(|s| Flag::parse(&s));
        let value = arg(&args, "value");
        let target = if matches!(
            self.1,
            "addmember" | "removemember" | "addowner" | "removeowner"
        ) {
            let raw = arg(&args, "player").ok_or_else(|| fail(&language, "invalid"))?;
            Some(
                regions::identity(&raw)
                    .or_else(|| {
                        player_name(&raw)
                            .then(|| server.get_player_by_name(&raw))
                            .flatten()
                            .map(|p| player_id(&p))
                    })
                    .ok_or_else(|| fail(&language, "unknown_player"))?,
            )
        } else {
            None
        };
        let result = self
            .0
            .state
            .lock()
            .map_err(|_| fail(&language, "unavailable"))?
            .stage(|rs| {
                if matches!(self.1, "define" | "global") {
                    if rs.iter().any(|r| r.name == name && r.world == world) {
                        return Err("duplicate");
                    }
                    // A world's global region belongs to the administrators.
                    let owner = actor.clone().filter(|_| self.1 == "define");
                    let candidate = Region::new(
                        &name,
                        &world,
                        selection.unwrap_or(Bounds::point([0; 3])),
                        owner,
                    );
                    if !may_overlap(admin, actor.as_deref(), &candidate, rs) {
                        return Err("forbidden");
                    }
                    rs.push(candidate);
                    return Ok(());
                }
                let index = rs
                    .iter()
                    .position(|r| r.name == name && r.world == world)
                    .ok_or("not_found")?;
                if !can_manage(admin, actor.as_deref(), &rs[index]) {
                    return Err("forbidden");
                }
                if matches!(self.1, "flag" | "redefine") {
                    let mut candidate = rs[index].clone();
                    if let Some(bounds) = selection {
                        candidate.bounds = bounds;
                    }
                    if !may_overlap(admin, actor.as_deref(), &candidate, rs) {
                        return Err("forbidden");
                    }
                }
                if self.1 == "delete" {
                    rs.remove(index);
                    return Ok(());
                }
                let r = &mut rs[index];
                match self.1 {
                    "redefine" => {
                        if r.name == GLOBAL {
                            return Err("invalid");
                        }
                        r.bounds = selection.ok_or("selection")?;
                    }
                    "setpriority" => {
                        r.priority = value
                            .as_ref()
                            .ok_or("invalid")?
                            .parse()
                            .map_err(|_| "invalid")?
                    }
                    "flag" => {
                        r.flags[flag.ok_or("invalid")? as usize] =
                            Rule::parse(value.as_deref().ok_or("invalid")?).ok_or("invalid")?
                    }
                    "addmember" => {
                        let id = target.as_ref().ok_or("invalid")?;
                        if !r.owners.contains(id) && !r.members.contains(id) {
                            if r.members.len() >= MAX_PEOPLE {
                                return Err("limit");
                            }
                            r.members.insert(id.clone());
                        }
                    }
                    "removemember" => {
                        r.members.remove(target.as_ref().ok_or("invalid")?);
                    }
                    "addowner" => {
                        let id = target.as_ref().ok_or("invalid")?;
                        if !r.owners.contains(id) && r.owners.len() >= MAX_PEOPLE {
                            return Err("limit");
                        }
                        r.members.remove(id);
                        r.owners.insert(id.clone());
                    }
                    "removeowner" => {
                        let id = target.as_ref().ok_or("invalid")?;
                        // Only an administrator can leave a region to the administrators.
                        if !admin && r.owners.contains(id) && r.owners.len() == 1 {
                            return Err("last_owner");
                        }
                        r.owners.remove(id);
                    }
                    _ => return Err("invalid"),
                }
                Ok(())
            });
        if let Err(key) = result {
            say(&sender, key, &[&name]);
            return Ok(0);
        }
        // The rule must have its events before it can apply. Until the save succeeds the change
        // waits, so a failure here leaves the old protection in force and /rg status says so.
        let needs = self.0.state.lock().map_or(u16::MAX, |s| s.needs());
        if !self.0.activate(needs) {
            say(&sender, "unavailable", &[]);
            return Ok(0);
        }
        if self.1 == "define"
            && let Some(id) = actor.as_ref()
            && let Ok(mut selections) = self.0.selections.lock()
        {
            selections.remove(id);
        }
        let shared = self.0.clone();
        server.schedule_delayed_async_task(1, move |_| shared.flush());
        say(&sender, "queued", &[&name]);
        if name == GLOBAL && matches!(flag, Some(Flag::Fire | Flag::LiquidFlow)) {
            say(&sender, "global_limit", &[]);
        }
        Ok(1)
    }
}
fn fmt(p: [i32; 3]) -> String {
    format!("{} {} {}", p[0], p[1], p[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_owner_or_explicit_admin_can_manage() {
        let r = Region::new("r", "world", Bounds::point([0; 3]), Some("0".repeat(32)));
        assert!(!can_manage(false, None, &r));
        assert!(!can_manage(false, Some("outsider"), &r));
        assert!(can_manage(false, Some(&"0".repeat(32)), &r));
        assert!(can_manage(true, None, &r));
    }
    #[test]
    fn a_region_without_owners_is_managed_by_administrators_only() {
        let r = Region::new("r", "world", Bounds::point([0; 3]), None);
        assert!(can_manage(true, None, &r));
        assert!(!can_manage(false, Some(&"0".repeat(32)), &r));
    }
    #[test]
    fn create_permission_cannot_override_someone_elses_region_or_global() {
        let r = Region::new("r", "world", Bounds::point([0; 3]), Some("0".repeat(32)));
        let candidate = Region::new("mine", "world", Bounds::point([0; 3]), Some("1".repeat(32)));
        assert!(!may_overlap(
            false,
            Some(&"1".repeat(32)),
            &candidate,
            std::slice::from_ref(&r)
        ));
        assert!(may_overlap(
            true,
            None,
            &candidate,
            std::slice::from_ref(&r)
        ));
        assert!(may_overlap(
            false,
            Some(&"0".repeat(32)),
            &candidate,
            std::slice::from_ref(&r)
        ));
        let mut global = r;
        global.name = GLOBAL.into();
        let mut remote = candidate;
        remote.bounds = Bounds::point([100; 3]);
        assert!(!may_overlap(
            false,
            Some(&"1".repeat(32)),
            &remote,
            &[global]
        ));
    }
}
