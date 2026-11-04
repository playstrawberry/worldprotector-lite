use crate::regions::{Flag, Policy, Region, Verdict};
use std::sync::Arc;

pub const MAX_BULK: usize = 4096;
pub const MAX_BULK_VISITS: usize = 8192;

pub struct State {
    pub active: Option<Arc<Policy>>,
    pub pending: Option<Arc<Policy>>,
    pub revision: u64,
    pub reload: bool,
    pub write_failed: bool,
    pub scheduled: bool,
}
impl State {
    pub fn new(policy: Option<Policy>) -> Self {
        Self {
            active: policy.map(Arc::new),
            pending: None,
            revision: 0,
            reload: false,
            write_failed: false,
            scheduled: false,
        }
    }
    pub fn ready(&self) -> bool {
        self.active.is_some() && self.pending.is_none() && !self.reload && !self.write_failed
    }
    #[cfg(test)]
    pub fn decide(&self, world: &str, p: [i32; 3], flag: Flag, actor: Option<&str>) -> Verdict {
        self.evaluate(world, |policy| policy.decide(world, p, flag, actor))
    }
    /// False only when no policy, published or waiting to be saved, has a region in this world.
    pub fn in_scope(&self, world: &str) -> bool {
        self.active.as_ref().is_none_or(|p| p.has_regions(world))
            || self.pending.as_ref().is_some_and(|p| p.has_regions(world))
    }
    /// Every flag either policy can refuse; with no valid policy, all of them stay guarded.
    pub fn needs(&self) -> u16 {
        let Some(active) = self.active.as_ref() else {
            return u16::MAX;
        };
        active.needs() | self.pending.as_ref().map_or(0, |p| p.needs())
    }
    pub fn evaluate(&self, world: &str, decide: impl Fn(&Policy) -> Verdict) -> Verdict {
        self.union(|policy| {
            if policy.has_regions(world) {
                decide(policy)
            } else {
                Verdict::Allow
            }
        })
    }
    /// For an event that names no world: the worlds that may refuse it, under either policy. `None`
    /// when that cannot be decided, which refuses the action.
    pub fn refusing_worlds(&self, p: [i32; 3], flag: Flag) -> Option<Vec<String>> {
        let active = self.active.as_ref()?;
        let mut out = Vec::new();
        let complete = active.refusing_worlds(p, flag, &mut out)
            && self
                .pending
                .as_ref()
                .is_none_or(|policy| policy.refusing_worlds(p, flag, &mut out));
        complete.then_some(out)
    }
    /// The blocks an explosion may still destroy, in their given order. Whatever cannot be decided
    /// within the work limit stays protected; a block an earlier listener removed is not in the list.
    pub fn explosion_keeps(&self, world: &str, blocks: &[[i32; 3]]) -> Vec<[i32; 3]> {
        if blocks.len() > MAX_BULK {
            return Vec::new();
        }
        let visits = std::cell::Cell::new(0usize);
        let mut keep = Vec::with_capacity(blocks.len());
        for pos in blocks {
            let v = self.evaluate(world, |policy| {
                let (v, cost) = policy.decide_counted(world, *pos, Flag::Explosions, None);
                visits.set(visits.get() + cost);
                v
            });
            if visits.get() > MAX_BULK_VISITS {
                break;
            }
            if v == Verdict::Allow {
                keep.push(*pos);
            }
        }
        keep
    }
    fn union(&self, decide: impl Fn(&Policy) -> Verdict) -> Verdict {
        let Some(active) = self.active.as_ref() else {
            return Verdict::Unavailable;
        };
        let published = decide(active);
        if self.ready() {
            return published;
        }
        let candidate = self.pending.as_ref().map_or(Verdict::Allow, |p| decide(p));
        if published == Verdict::Allow && candidate == Verdict::Allow {
            Verdict::Allow
        } else {
            // Only an action protected by either snapshot waits for durable publication.
            Verdict::Unavailable
        }
    }
    pub fn stage(
        &mut self,
        change: impl FnOnce(&mut Vec<Region>) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if !self.ready() {
            return Err("unavailable");
        }
        let mut regions = self
            .active
            .as_ref()
            .ok_or("unavailable")?
            .regions()
            .to_vec();
        change(&mut regions)?;
        let candidate = Policy::build(regions)?;
        self.revision = self.revision.checked_add(1).ok_or("limit")?;
        self.pending = Some(Arc::new(candidate));
        self.write_failed = false;
        self.scheduled = true;
        Ok(())
    }
    pub fn finish_write(&mut self, revision: u64, success: bool) {
        if revision != self.revision || self.pending.is_none() {
            return;
        }
        self.scheduled = false;
        if success {
            self.active = self.pending.take();
            self.write_failed = false;
        } else {
            self.write_failed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::Bounds;
    #[test]
    fn pending_or_failed_save_does_not_protect_unrelated_worlds_or_locations() {
        let r = Region::new(
            "r",
            "protected",
            Bounds::point([0; 3]),
            Some("0".repeat(32)),
        );
        let mut s = State::new(Some(Policy::build(vec![r]).unwrap()));
        s.stage(|_| Ok(())).unwrap();
        for failed in [false, true] {
            if failed {
                s.finish_write(s.revision, false);
            }
            for flag in Flag::ALL {
                assert_eq!(s.decide("elsewhere", [0; 3], flag, None), Verdict::Allow);
                assert_eq!(s.decide("protected", [100; 3], flag, None), Verdict::Allow);
            }
            assert!(s.decide("protected", [0; 3], Flag::Break, None).denied());
        }
    }
    #[test]
    fn scope_covers_the_waiting_policy_and_an_unreadable_one() {
        assert!(State::new(None).in_scope("anywhere"));
        assert_eq!(State::new(None).needs(), u16::MAX);
        let mut s = State::new(Some(Policy::default()));
        assert!(!s.in_scope("world"));
        assert_eq!(s.needs(), 0);
        s.stage(|rs| {
            rs.push(Region::new("r", "world", Bounds::point([0; 3]), None));
            Ok(())
        })
        .unwrap();
        assert!(s.in_scope("world") && !s.in_scope("other"));
        assert_ne!(s.needs() & Flag::Break.bit(), 0);
    }
    #[test]
    fn explosion_takes_out_only_protected_blocks() {
        let r = Region::new("r", "world", Bounds::point([0; 3]), None);
        let s = State::new(Some(Policy::build(vec![r]).unwrap()));
        let list = [[1, 0, 0], [0, 0, 0], [0, 1, 0]];
        assert_eq!(
            s.explosion_keeps("world", &list),
            vec![[1, 0, 0], [0, 1, 0]]
        );
        assert_eq!(s.explosion_keeps("other", &list), list.to_vec());
    }
    #[test]
    fn explosion_keeps_nothing_it_could_not_decide() {
        assert!(
            State::new(None)
                .explosion_keeps("world", &[[0; 3]])
                .is_empty()
        );
        let s = State::new(Some(Policy::default()));
        assert!(
            s.explosion_keeps("world", &vec![[0; 3]; MAX_BULK + 1])
                .is_empty()
        );
        // Overlap so deep that the work limit runs out: nothing past that point may go.
        let many: Vec<_> = (0..1000)
            .map(|i| {
                let mut r = Region::new(&format!("r{i}"), "world", Bounds::point([0; 3]), None);
                r.flags[Flag::Explosions as usize] = crate::regions::Rule::Allow;
                r
            })
            .collect();
        let s = State::new(Some(Policy::build(many).unwrap()));
        assert!(s.explosion_keeps("world", &[[0; 3]; 32]).is_empty());
    }
    #[test]
    fn unknown_world_decisions_follow_both_policies_during_a_save() {
        let mut r = Region::new("r", "world", Bounds::point([0; 3]), None);
        r.flags[Flag::Fire as usize] = crate::regions::Rule::Deny;
        let mut s = State::new(Some(Policy::build(vec![r]).unwrap()));
        let protected = Some(vec!["world".to_owned()]);
        assert_eq!(s.refusing_worlds([0; 3], Flag::Fire), protected);
        s.stage(|rs| {
            rs.clear();
            Ok(())
        })
        .unwrap();
        assert_eq!(s.refusing_worlds([0; 3], Flag::Fire), protected);
        assert_eq!(s.refusing_worlds([9; 3], Flag::Fire), Some(vec![]));
        s.finish_write(s.revision, true);
        assert_eq!(s.refusing_worlds([0; 3], Flag::Fire), Some(vec![]));
        assert_eq!(State::new(None).refusing_worlds([0; 3], Flag::Fire), None);
    }
    #[test]
    fn reloading_an_empty_policy_does_not_change_vanilla_actions() {
        let mut s = State::new(Some(Policy::default()));
        s.reload = true;
        for flag in Flag::ALL {
            assert_eq!(s.decide("world", [0; 3], flag, None), Verdict::Allow);
        }
    }
    #[test]
    fn entry_scope_is_independent_across_worlds_during_a_save() {
        let mut r = Region::new(
            "r",
            "protected",
            Bounds::point([0; 3]),
            Some("0".repeat(32)),
        );
        r.flags[Flag::Entry as usize] = crate::regions::Rule::Deny;
        let mut s = State::new(Some(Policy::build(vec![r]).unwrap()));
        s.stage(|_| Ok(())).unwrap();
        for world in ["protected", "elsewhere"] {
            let v = s.evaluate(world, |policy| policy.entry(world, None, [0; 3], None));
            assert_eq!(v.denied(), world == "protected");
        }
        assert_eq!(
            s.evaluate("elsewhere", |policy| policy.entry_path(
                "elsewhere",
                (-1.0, 0.0, 0.0),
                (1.0, 0.0, 0.0),
                None
            )),
            Verdict::Allow
        );
    }
    #[test]
    fn pending_add_remove_and_member_changes_preserve_the_union_of_denials() {
        let r = Region::new("r", "world", Bounds::point([0; 3]), Some("0".repeat(32)));
        let mut s = State::new(Some(Policy::build(vec![r]).unwrap()));
        s.stage(|rs| {
            rs[0].members.insert("1".repeat(32));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            s.decide("world", [0; 3], Flag::Break, Some(&"1".repeat(32))),
            Verdict::Unavailable
        );
        assert_eq!(
            s.decide("world", [0; 3], Flag::Break, Some(&"0".repeat(32))),
            Verdict::Allow
        );
        s.finish_write(s.revision, true);
        assert_eq!(
            s.decide("world", [0; 3], Flag::Break, Some(&"1".repeat(32))),
            Verdict::Allow
        );
        s.stage(|rs| {
            rs[0].members.clear();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            s.decide("world", [0; 3], Flag::Break, Some(&"1".repeat(32))),
            Verdict::Unavailable
        );
    }
    #[test]
    fn no_answer_pending_or_failed_write_denies_until_durable() {
        let mut s = State::new(None);
        assert!(s.decide("world", [0; 3], Flag::Build, None).denied());
        s = State::new(Some(Policy::default()));
        s.stage(|rs| {
            rs.push(Region::new(
                "r",
                "world",
                Bounds::point([0; 3]),
                Some("0".repeat(32)),
            ));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            s.decide("unrelated", [0; 3], Flag::Build, None),
            Verdict::Allow
        );
        assert!(s.stage(|_| Ok(())).is_err());
        s.finish_write(s.revision, false);
        assert!(s.write_failed && !s.ready());
        s.finish_write(s.revision + 1, true);
        assert!(!s.ready());
        s.finish_write(s.revision, true);
        assert!(s.ready());
        assert_eq!(s.decide("world", [0; 3], Flag::Build, None), Verdict::Deny);
    }
    #[test]
    fn removal_and_revocation_do_not_publish_before_save() {
        let r = Region::new("r", "world", Bounds::point([0; 3]), Some("0".repeat(32)));
        let mut s = State::new(Some(Policy::build(vec![r]).unwrap()));
        s.stage(|rs| {
            rs.clear();
            Ok(())
        })
        .unwrap();
        assert!(s.decide("world", [0; 3], Flag::Build, None).denied());
        assert_eq!(s.active.as_ref().unwrap().regions().len(), 1);
        s.finish_write(s.revision, true);
        assert_eq!(s.decide("world", [0; 3], Flag::Build, None), Verdict::Allow);
    }
}
