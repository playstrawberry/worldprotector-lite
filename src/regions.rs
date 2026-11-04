//! Bounded, host-independent policy and spatial lookup. No I/O or host handles.

use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const MAX_REGIONS: usize = 4096;
pub const MAX_WORLDS: usize = 128;
pub const MAX_PEOPLE: usize = 64;
pub const MAX_SELECTIONS: usize = 1024;
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_VISITS: usize = 512;
pub const GLOBAL: &str = "__global__";
pub const HEADER: &str = "# worldprotector-lite regions v3\n";
/// The previous format: the same rows, but every region had to have an owner.
const HEADER_V2: &str = "# worldprotector-lite regions v2\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Flag {
    Build,
    Break,
    Interact,
    Pvp,
    Entry,
    MobSpawn,
    Explosions,
    Fire,
    LiquidFlow,
}
impl Flag {
    pub const ALL: [Self; 9] = [
        Self::Build,
        Self::Break,
        Self::Interact,
        Self::Pvp,
        Self::Entry,
        Self::MobSpawn,
        Self::Explosions,
        Self::Fire,
        Self::LiquidFlow,
    ];
    pub fn name(self) -> &'static str {
        [
            "build",
            "break",
            "interact",
            "pvp",
            "entry",
            "mob-spawn",
            "explosions",
            "fire",
            "liquid-flow",
        ][self as usize]
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == s)
    }
    fn default(self) -> Rule {
        match self {
            Self::Build | Self::Break | Self::Interact => Rule::Members,
            // Blocks only: an explosion keeps its effects, damage and knockback.
            Self::Explosions => Rule::Deny,
            Self::Pvp | Self::Entry | Self::MobSpawn | Self::Fire | Self::LiquidFlow => Rule::Allow,
        }
    }
    pub const fn bit(self) -> u16 {
        1 << self as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rule {
    #[default]
    Unset,
    Allow,
    Deny,
    Members,
}
impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Members => "members",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "unset" => Some(Self::Unset),
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "members" => Some(Self::Members),
            _ => None,
        }
    }
    fn allows(self, member: bool) -> bool {
        matches!(self, Self::Allow) || self == Self::Members && member
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}
impl Bounds {
    pub fn point(p: [i32; 3]) -> Self {
        Self { min: p, max: p }
    }
    pub fn contains(self, p: [i32; 3]) -> bool {
        (0..3).all(|a| self.min[a] <= p[a] && p[a] <= self.max[a])
    }
    pub fn intersects(self, other: Self) -> bool {
        (0..3).all(|a| self.min[a] <= other.max[a] && other.min[a] <= self.max[a])
    }
    fn union(self, b: Self) -> Self {
        Self {
            min: std::array::from_fn(|a| self.min[a].min(b.min[a])),
            max: std::array::from_fn(|a| self.max[a].max(b.max[a])),
        }
    }
    fn valid(self) -> bool {
        valid_pos(self.min) && valid_pos(self.max) && (0..3).all(|a| self.min[a] <= self.max[a])
    }
}
pub fn valid_pos(p: [i32; 3]) -> bool {
    p[0].abs_diff(0) <= 30_000_000
        && p[2].abs_diff(0) <= 30_000_000
        && (-2048..=2047).contains(&p[1])
}
pub fn block_position(p: (f64, f64, f64)) -> Option<[i32; 3]> {
    let [x, y, z] = [p.0, p.1, p.2];
    if ![x, y, z].into_iter().all(f64::is_finite)
        || !(-30_000_000.0..30_000_001.0).contains(&x)
        || !(-30_000_000.0..30_000_001.0).contains(&z)
        || !(-2048.0..2048.0).contains(&y)
    {
        return None;
    }
    Some([x.floor() as i32, y.floor() as i32, z.floor() as i32])
}
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s != GLOBAL
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c))
}
pub fn valid_world(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
pub fn identity(s: &str) -> Option<String> {
    let hex: String = s.chars().filter(|&c| c != '-').collect();
    if hex.len() != 32
        || !hex.bytes().all(|c| c.is_ascii_hexdigit())
        || !(s.len() == 32
            || s.len() == 36 && [8, 13, 18, 23].into_iter().all(|i| s.as_bytes()[i] == b'-'))
    {
        return None;
    }
    Some(hex.to_ascii_lowercase())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub name: String,
    pub world: String,
    pub bounds: Bounds,
    pub priority: i32,
    pub owners: BTreeSet<String>,
    pub members: BTreeSet<String>,
    pub flags: [Rule; 9],
}
impl Region {
    pub fn new(name: &str, world: &str, bounds: Bounds, owner: Option<String>) -> Self {
        Self {
            name: name.into(),
            world: world.into(),
            bounds,
            priority: 0,
            owners: owner.into_iter().collect(),
            members: BTreeSet::new(),
            flags: [Rule::Unset; 9],
        }
    }
    pub fn is_member(&self, id: &str) -> bool {
        self.owners.contains(id) || self.members.contains(id)
    }
    fn valid(&self) -> bool {
        (valid_name(&self.name) || self.name == GLOBAL)
            && valid_world(&self.world)
            && self.bounds.valid()
            && (-1_000_000..=1_000_000).contains(&self.priority)
            && self.owners.len() <= MAX_PEOPLE
            && self.members.len() <= MAX_PEOPLE
            && self
                .owners
                .iter()
                .chain(&self.members)
                .all(|id| identity(id).as_deref() == Some(id.as_str()))
            && self.owners.is_disjoint(&self.members)
    }
}

#[derive(Clone, Debug)]
struct Node {
    bounds: Bounds,
    kind: NodeKind,
}
#[derive(Clone, Debug)]
enum NodeKind {
    Leaf(usize),
    Branch(usize, usize),
}
#[derive(Clone, Debug, Default)]
struct WorldIndex {
    nodes: Vec<Node>,
    root: Option<usize>,
    global: Option<usize>,
}
impl WorldIndex {
    fn build(&mut self, ids: &mut [usize], regions: &[Region]) -> usize {
        let bounds = ids
            .iter()
            .map(|&i| regions[i].bounds)
            .reduce(Bounds::union)
            .expect("nonempty");
        let kind = if ids.len() == 1 {
            NodeKind::Leaf(ids[0])
        } else {
            let axis = (0..3)
                .max_by_key(|&a| i64::from(bounds.max[a]) - i64::from(bounds.min[a]))
                .unwrap_or(0);
            let mid = ids.len() / 2;
            ids.select_nth_unstable_by_key(mid, |&i| {
                (
                    i64::from(regions[i].bounds.min[axis]) + i64::from(regions[i].bounds.max[axis]),
                    i,
                )
            });
            let (left, right) = ids.split_at_mut(mid);
            NodeKind::Branch(self.build(left, regions), self.build(right, regions))
        };
        let index = self.nodes.len();
        self.nodes.push(Node { bounds, kind });
        index
    }
    fn visit(&self, bounds: Bounds, budget: &mut usize, f: &mut impl FnMut(usize)) -> bool {
        let Some(root) = self.root else {
            return true;
        };
        // Balanced tree: at most 13 pending nodes for 4096 leaves; no event-time allocation.
        let mut stack = [0usize; 32];
        stack[0] = root;
        let mut len = 1;
        while len > 0 {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            len -= 1;
            let node = &self.nodes[stack[len]];
            if !node.bounds.intersects(bounds) {
                continue;
            }
            match node.kind {
                NodeKind::Leaf(i) => f(i),
                NodeKind::Branch(a, b) => {
                    stack[len] = a;
                    stack[len + 1] = b;
                    len += 2;
                }
            }
        }
        true
    }
}

#[derive(Clone, Debug, Default)]
pub struct Policy {
    regions: Vec<Region>,
    worlds: BTreeMap<String, WorldIndex>,
    by_name: BTreeMap<(String, String), usize>,
    needs: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
    Unavailable,
}
impl Verdict {
    pub fn denied(self) -> bool {
        self != Self::Allow
    }
}
#[derive(Default)]
struct Vote {
    priority: Option<i32>,
    denied: bool,
}
impl Vote {
    fn add(&mut self, priority: i32, allowed: bool) {
        match self.priority {
            Some(old) if old > priority => {}
            Some(old) if old == priority => self.denied |= !allowed,
            _ => {
                self.priority = Some(priority);
                self.denied = !allowed;
            }
        }
    }
}
impl Policy {
    pub fn build(regions: Vec<Region>) -> Result<Self, &'static str> {
        if regions.len() > MAX_REGIONS {
            return Err("limit");
        }
        let mut out = Self {
            regions,
            ..Self::default()
        };
        let mut stored_bytes = HEADER.len();
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, r) in out.regions.iter().enumerate() {
            if !r.valid() {
                return Err("invalid");
            }
            // Bound both durable bytes and the simultaneously retained old/candidate snapshots.
            stored_bytes += r.name.len()
                + r.world.len()
                + 8
                + 4
                + r.bounds
                    .min
                    .iter()
                    .chain(&r.bounds.max)
                    .map(|v| v.to_string().len())
                    .sum::<usize>()
                + r.priority.to_string().len()
                + r.owners.iter().map(|s| s.len()).sum::<usize>()
                + r.owners.len().saturating_sub(1)
                + r.members.iter().map(|s| s.len()).sum::<usize>()
                + r.members.len().saturating_sub(1)
                + r.flags.iter().map(|v| v.name().len()).sum::<usize>()
                + 8;
            if stored_bytes > MAX_FILE_BYTES {
                return Err("limit");
            }
            if out
                .by_name
                .insert((r.world.clone(), r.name.clone()), i)
                .is_some()
            {
                return Err("duplicate");
            }
            for flag in Flag::ALL {
                // A cuboid's unset flag applies its default; a global's unset flag applies nothing.
                let effective = match r.flags[flag as usize] {
                    Rule::Unset if r.name == GLOBAL => Rule::Allow,
                    Rule::Unset => flag.default(),
                    rule => rule,
                };
                if effective != Rule::Allow {
                    out.needs |= flag.bit();
                }
            }
            let world = out.worlds.entry(r.world.clone()).or_default();
            if r.name == GLOBAL {
                world.global = Some(i);
            } else {
                groups.entry(r.world.clone()).or_default().push(i);
            }
        }
        if out.worlds.len() > MAX_WORLDS {
            return Err("limit");
        }
        for (world, mut ids) in groups {
            let index = out.worlds.get_mut(&world).expect("world inserted");
            index.nodes.reserve(ids.len() * 2 - 1);
            index.root = Some(index.build(&mut ids, &out.regions));
        }
        Ok(out)
    }
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }
    pub fn region(&self, world: &str, name: &str) -> Option<&Region> {
        self.by_name
            .get(&(world.into(), name.into()))
            .map(|&i| &self.regions[i])
    }
    pub fn has_regions(&self, world: &str) -> bool {
        self.worlds.contains_key(world)
    }
    /// Which flags can refuse anything under this policy: only their events need a subscription.
    pub fn needs(&self) -> u16 {
        self.needs
    }
    pub fn decide(&self, world: &str, pos: [i32; 3], flag: Flag, actor: Option<&str>) -> Verdict {
        self.decide_counted(world, pos, flag, actor).0
    }
    pub fn decide_counted(
        &self,
        world: &str,
        pos: [i32; 3],
        flag: Flag,
        actor: Option<&str>,
    ) -> (Verdict, usize) {
        self.query(world, pos, flag, actor, None, false)
    }
    /// For an action whose event does not name its world: the worlds whose cuboids refuse it at these
    /// coordinates. False when the work limit ran out. A world-global rule is not applied here, it
    /// would stop every other world.
    pub fn refusing_worlds(&self, pos: [i32; 3], flag: Flag, out: &mut Vec<String>) -> bool {
        for (world, index) in &self.worlds {
            if index.root.is_some() {
                match self.resolve(world, pos, flag, None, None, false, false).0 {
                    Verdict::Allow => {}
                    Verdict::Deny => {
                        if !out.contains(world) {
                            out.push(world.clone());
                        }
                    }
                    Verdict::Unavailable => return false,
                }
            }
        }
        true
    }
    /// For a target known to be within `area` but at no exact position: refuse when any cuboid
    /// there could refuse this actor, or when the world-global rule does.
    pub fn decide_within(
        &self,
        world: &str,
        area: Bounds,
        flag: Flag,
        actor: Option<&str>,
    ) -> Verdict {
        let Some(index) = self.worlds.get(world) else {
            return Verdict::Allow;
        };
        let mut denied = false;
        let mut budget = MAX_VISITS;
        let complete = index.visit(area, &mut budget, &mut |i| {
            let r = &self.regions[i];
            let rule = match r.flags[flag as usize] {
                Rule::Unset => flag.default(),
                rule => rule,
            };
            denied |= !rule.allows(actor.is_some_and(|id| r.is_member(id)));
        });
        if !complete {
            return Verdict::Unavailable;
        }
        denied |= index.global.is_some_and(|i| {
            let r = &self.regions[i];
            let rule = r.flags[flag as usize];
            rule != Rule::Unset && !rule.allows(actor.is_some_and(|id| r.is_member(id)))
        });
        if denied {
            Verdict::Deny
        } else {
            Verdict::Allow
        }
    }
    pub fn entry(
        &self,
        world: &str,
        from: Option<[i32; 3]>,
        to: [i32; 3],
        actor: Option<&str>,
    ) -> Verdict {
        self.query(world, to, Flag::Entry, actor, from, true).0
    }
    pub fn entry_path(
        &self,
        world: &str,
        from: (f64, f64, f64),
        to: (f64, f64, f64),
        actor: Option<&str>,
    ) -> Verdict {
        let (Some(a), Some(b)) = (block_position(from), block_position(to)) else {
            return Verdict::Unavailable;
        };
        let end = self.entry(world, Some(a), b, actor);
        if end.denied() || a == b {
            return end;
        }
        let Some(index) = self.worlds.get(world) else {
            return Verdict::Allow;
        };
        let broad = Bounds {
            min: std::array::from_fn(|i| a[i].min(b[i])),
            max: std::array::from_fn(|i| a[i].max(b[i])),
        };
        let (origin, target) = ([from.0, from.1, from.2], [to.0, to.1, to.2]);
        let mut cuts = [0.0f64; 1026];
        cuts[1] = 1.0;
        let mut count = 2;
        let mut budget = MAX_VISITS;
        let mut overflow = false;
        let complete = index.visit(broad, &mut budget, &mut |i| {
            let bounds = self.regions[i].bounds;
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for axis in 0..3 {
                let delta = target[axis] - origin[axis];
                let (min, max) = (
                    f64::from(bounds.min[axis]),
                    f64::from(bounds.max[axis]) + 1.0,
                );
                if delta == 0.0 {
                    if origin[axis] < min || origin[axis] >= max {
                        return;
                    }
                } else {
                    let (x, y) = ((min - origin[axis]) / delta, (max - origin[axis]) / delta);
                    lo = lo.max(x.min(y));
                    hi = hi.min(x.max(y));
                }
            }
            if lo > hi {
                return;
            }
            if count + 2 > cuts.len() {
                overflow = true;
                return;
            }
            cuts[count] = lo;
            cuts[count + 1] = hi;
            count += 2;
        });
        if !complete || overflow {
            return Verdict::Unavailable;
        }
        cuts[..count].sort_unstable_by(f64::total_cmp);
        let mut visits = MAX_VISITS - budget;
        // Membership can change only at a cuboid boundary. Check each boundary and interval.
        for i in 0..count {
            for t in [
                cuts[i],
                (cuts[i] + cuts.get(i + 1).copied().unwrap_or(cuts[i])) / 2.0,
            ] {
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let p: [f64; 3] =
                    std::array::from_fn(|axis| origin[axis] + (target[axis] - origin[axis]) * t);
                let Some(pos) = block_position((p[0], p[1], p[2])) else {
                    return Verdict::Unavailable;
                };
                let (v, cost) = self.query(world, pos, Flag::Entry, actor, Some(a), true);
                visits += cost;
                if visits > 8192 {
                    return Verdict::Unavailable;
                }
                if v.denied() {
                    return v;
                }
            }
        }
        Verdict::Allow
    }
    fn query(
        &self,
        world: &str,
        pos: [i32; 3],
        flag: Flag,
        actor: Option<&str>,
        from: Option<[i32; 3]>,
        transition: bool,
    ) -> (Verdict, usize) {
        self.resolve(world, pos, flag, actor, from, transition, true)
    }
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        &self,
        world: &str,
        pos: [i32; 3],
        flag: Flag,
        actor: Option<&str>,
        from: Option<[i32; 3]>,
        transition: bool,
        with_global: bool,
    ) -> (Verdict, usize) {
        if !valid_pos(pos) || !valid_world(world) {
            return (Verdict::Unavailable, 0);
        }
        let Some(index) = self.worlds.get(world) else {
            return (Verdict::Allow, 0);
        };
        let (mut explicit, mut fallback) = (Vote::default(), Vote::default());
        let mut budget = MAX_VISITS;
        let complete = index.visit(Bounds::point(pos), &mut budget, &mut |i| {
            let r = &self.regions[i];
            let member = actor.is_some_and(|id| r.is_member(id));
            let rule = r.flags[flag as usize];
            if rule != Rule::Unset {
                let already_inside = transition && from.is_some_and(|p| r.bounds.contains(p));
                explicit.add(r.priority, already_inside || rule.allows(member));
            }
            fallback.add(r.priority, flag.default().allows(member));
        });
        if !complete {
            return (Verdict::Unavailable, MAX_VISITS);
        }
        // Global is a per-flag fallback, not a giant cuboid or arbitrary priority.
        let global = index
            .global
            .filter(|_| with_global)
            .map(|i| &self.regions[i]);
        let denied = if explicit.priority.is_some() {
            explicit.denied
        } else if let Some(r) = global.filter(|r| r.flags[flag as usize] != Rule::Unset) {
            !(transition && from.is_some()
                || r.flags[flag as usize].allows(actor.is_some_and(|id| r.is_member(id))))
        } else {
            fallback.denied
        };
        (
            if denied {
                Verdict::Deny
            } else {
                Verdict::Allow
            },
            MAX_VISITS - budget,
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub world: String,
    pub pos1: Option<[i32; 3]>,
    pub pos2: Option<[i32; 3]>,
}
impl Selection {
    pub fn set(&mut self, world: &str, first: bool, pos: [i32; 3]) {
        if self.world != world {
            self.world = world.into();
            self.pos1 = None;
            self.pos2 = None;
        }
        if first {
            self.pos1 = Some(pos);
        } else {
            self.pos2 = Some(pos);
        }
    }
    pub fn bounds(&self) -> Option<Bounds> {
        let (a, b) = (self.pos1?, self.pos2?);
        Some(Bounds {
            min: std::array::from_fn(|i| a[i].min(b[i])),
            max: std::array::from_fn(|i| a[i].max(b[i])),
        })
    }
}
pub type Selections = HashMap<String, Selection>;

/// Things that spawn but are not creatures: `mob-spawn` must not stop drops, projectiles or displays.
pub fn is_mob(entity_type: &str) -> bool {
    let name = entity_type.trim_start_matches("minecraft:");
    !(name.ends_with("_boat")
        || name.ends_with("_raft")
        || name.ends_with("minecart")
        || name.ends_with("_display")
        || name.ends_with("_arrow")
        || name.ends_with("_fireball")
        || name.ends_with("_potion")
        || name.ends_with("_item_frame")
        || matches!(
            name,
            "area_effect_cloud"
                | "armor_stand"
                | "arrow"
                | "boat"
                | "breeze_wind_charge"
                | "egg"
                | "end_crystal"
                | "ender_pearl"
                | "evoker_fangs"
                | "experience_bottle"
                | "experience_orb"
                | "eye_of_ender"
                | "falling_block"
                | "fireball"
                | "firework_rocket"
                | "fishing_bobber"
                | "interaction"
                | "item"
                | "item_frame"
                | "leash_knot"
                | "lightning_bolt"
                | "llama_spit"
                | "marker"
                | "ominous_item_spawner"
                | "painting"
                | "player"
                | "potion"
                | "shulker_bullet"
                | "snowball"
                | "tnt"
                | "trident"
                | "wind_charge"
                | "wither_skull"
        ))
}

/// Refusal lines per player: at most one a second, for a bounded number of players.
#[derive(Debug, Default)]
pub struct Throttle(HashMap<u128, u64>);
impl Throttle {
    pub const INTERVAL_MS: u64 = 1000;
    pub fn allow(&mut self, player: u128, now_ms: u64) -> bool {
        if let Some(last) = self.0.get(&player)
            && now_ms.saturating_sub(*last) < Self::INTERVAL_MS
        {
            return false;
        }
        if self.0.len() >= MAX_SELECTIONS && !self.0.contains_key(&player) {
            self.0
                .retain(|_, last| now_ms.saturating_sub(*last) < Self::INTERVAL_MS);
            if self.0.len() >= MAX_SELECTIONS {
                return false;
            }
        }
        self.0.insert(player, now_ms);
        true
    }
    pub fn forget(&mut self, player: u128) {
        self.0.remove(&player);
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

pub fn encode(regions: &[Region]) -> String {
    let mut out = String::from(HEADER);
    for r in regions {
        out.push_str(&format!(
            "{}\t{}\t{},{},{}\t{},{},{}\t{}\t{}\t{}\t{}\n",
            r.name,
            r.world,
            r.bounds.min[0],
            r.bounds.min[1],
            r.bounds.min[2],
            r.bounds.max[0],
            r.bounds.max[1],
            r.bounds.max[2],
            r.priority,
            r.owners.iter().cloned().collect::<Vec<_>>().join(","),
            r.members.iter().cloned().collect::<Vec<_>>().join(","),
            r.flags
                .iter()
                .map(|r| r.name())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    out
}
pub fn decode(text: &str) -> (Vec<Region>, usize) {
    let v2 = text.starts_with(HEADER_V2);
    if text.len() > MAX_FILE_BYTES || !(v2 || text.starts_with(HEADER)) {
        return (vec![], 1);
    }
    let mut regions = Vec::new();
    for line in text.lines().skip(1) {
        if regions.len() >= MAX_REGIONS {
            return (vec![], 1);
        }
        let Some(region) = parse_line(line).filter(|r| !v2 || !r.owners.is_empty()) else {
            return (vec![], 1);
        };
        regions.push(region);
    }
    if Policy::build(regions.clone()).is_err() {
        return (vec![], 1);
    }
    (regions, 0)
}
fn parse_line(line: &str) -> Option<Region> {
    if line.len() > 6000 {
        return None;
    }
    let fields: Vec<_> = line.split('\t').collect();
    if fields.len() != 8 {
        return None;
    }
    let people = |s: &str| -> Option<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        if !s.is_empty() {
            for raw in s.split(',') {
                if out.len() >= MAX_PEOPLE || !out.insert(identity(raw)?) {
                    return None;
                }
            }
        }
        Some(out)
    };
    let mut flags = [Rule::Unset; 9];
    let mut parts = fields[7].split(',');
    for flag in &mut flags {
        *flag = Rule::parse(parts.next()?)?;
    }
    if parts.next().is_some() {
        return None;
    }
    let r = Region {
        name: fields[0].into(),
        world: fields[1].into(),
        bounds: Bounds {
            min: triple(fields[2])?,
            max: triple(fields[3])?,
        },
        priority: fields[4].parse().ok()?,
        owners: people(fields[5])?,
        members: people(fields[6])?,
        flags,
    };
    r.valid().then_some(r)
}
fn triple(s: &str) -> Option<[i32; 3]> {
    let mut p = s.split(',');
    let out = [
        p.next()?.parse().ok()?,
        p.next()?.parse().ok()?,
        p.next()?.parse().ok()?,
    ];
    p.next().is_none().then_some(out)
}

#[cfg(test)]
#[path = "region_tests.rs"]
mod tests;
