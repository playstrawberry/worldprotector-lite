use super::*;

const ALICE: &str = "00000000000000000000000000000001";
const BOB: &str = "00000000000000000000000000000002";
fn region(name: &str) -> Region {
    Region::new(
        name,
        "world",
        Bounds {
            min: [0, 60, 0],
            max: [10, 80, 10],
        },
        Some(ALICE.into()),
    )
}
fn policy(regions: Vec<Region>) -> Policy {
    Policy::build(regions).unwrap()
}

#[test]
fn bounds_and_worlds_are_exact() {
    let p = policy(vec![region("spawn")]);
    for pos in [[0, 60, 0], [10, 80, 10]] {
        assert_eq!(p.decide("world", pos, Flag::Break, None), Verdict::Deny);
    }
    assert_eq!(
        p.decide("world", [11, 70, 5], Flag::Break, None),
        Verdict::Allow
    );
    assert_eq!(
        p.decide("nether", [5, 70, 5], Flag::Break, None),
        Verdict::Allow
    );
}
#[test]
fn all_flags_can_allow_deny_or_require_members() {
    for flag in Flag::ALL {
        for rule in [Rule::Allow, Rule::Deny, Rule::Members] {
            let mut r = region("r");
            r.flags[flag as usize] = rule;
            r.members.insert(BOB.into());
            let p = policy(vec![r]);
            for actor in [None, Some(ALICE), Some(BOB), Some("outsider")] {
                assert_eq!(
                    p.decide("world", [1, 70, 1], flag, actor),
                    if rule.allows(actor == Some(ALICE) || actor == Some(BOB)) {
                        Verdict::Allow
                    } else {
                        Verdict::Deny
                    }
                );
            }
        }
    }
}
#[test]
fn defaults_protect_builds_and_leave_the_world_alone() {
    for (flag, rule) in [
        (Flag::Build, Rule::Members),
        (Flag::Break, Rule::Members),
        (Flag::Interact, Rule::Members),
        (Flag::Explosions, Rule::Deny),
        (Flag::Pvp, Rule::Allow),
        (Flag::Entry, Rule::Allow),
        (Flag::MobSpawn, Rule::Allow),
        (Flag::Fire, Rule::Allow),
        (Flag::LiquidFlow, Rule::Allow),
    ] {
        assert_eq!(flag.default(), rule, "{}", flag.name());
    }
    let p = policy(vec![region("r")]);
    for f in Flag::ALL {
        assert_eq!(
            p.decide("world", [1, 70, 1], f, None).denied(),
            !f.default().allows(false)
        );
    }
    assert_eq!(
        p.decide("world", [1, 70, 1], Flag::Break, Some(ALICE)),
        Verdict::Allow
    );
}
#[test]
fn highest_priority_and_tied_deny_win_in_any_order() {
    let mut a = region("a");
    a.priority = 10;
    a.flags[Flag::Pvp as usize] = Rule::Allow;
    let mut b = region("b");
    b.flags[Flag::Pvp as usize] = Rule::Deny;
    assert_eq!(
        policy(vec![a.clone(), b.clone()]).decide("world", [1, 70, 1], Flag::Pvp, None),
        Verdict::Allow
    );
    b.priority = 10;
    for rs in [vec![a.clone(), b.clone()], vec![b, a]] {
        assert_eq!(
            policy(rs).decide("world", [1, 70, 1], Flag::Pvp, Some(ALICE)),
            Verdict::Deny
        );
    }
}
#[test]
fn unset_inherits_and_global_is_per_world_fallback() {
    let a = region("a");
    let mut b = region("b");
    b.priority = -1;
    b.flags[Flag::Entry as usize] = Rule::Deny;
    let mut g = region(GLOBAL);
    g.flags[Flag::Pvp as usize] = Rule::Deny;
    let p = policy(vec![a, b, g]);
    assert_eq!(
        p.decide("world", [1, 70, 1], Flag::Entry, None),
        Verdict::Deny
    );
    assert_eq!(
        p.decide("world", [1000, 70, 1], Flag::Pvp, None),
        Verdict::Deny
    );
    assert_eq!(
        p.decide("nether", [1000, 70, 1], Flag::Pvp, None),
        Verdict::Allow
    );
}
#[test]
fn local_explicit_overrides_global_regardless_of_global_priority() {
    let mut g = region(GLOBAL);
    g.priority = 1_000_000;
    g.flags[Flag::Build as usize] = Rule::Deny;
    let mut r = region("r");
    r.priority = -1_000_000;
    r.flags[Flag::Build as usize] = Rule::Allow;
    assert_eq!(
        policy(vec![r, g]).decide("world", [1, 70, 1], Flag::Build, None),
        Verdict::Allow
    );
}
#[test]
fn duplicate_region_ids_are_refused() {
    assert!(Policy::build(vec![region("r"), region("r")]).is_err());
    assert!(decode(&encode(&[region("r"), region("r")])).1 > 0);
    let mut nether = region("r");
    nether.world = "nether".into();
    assert!(Policy::build(vec![region("r"), nether]).is_ok());
}
#[test]
fn out_of_world_coordinates_are_refused() {
    let mut r = region("r");
    r.bounds.max[0] = 30_000_001;
    assert!(Policy::build(vec![r.clone()]).is_err());
    assert!(decode(&encode(&[r])).1 > 0);
    for pos in [
        (f64::NAN, 1.0, 1.0),
        (f64::INFINITY, 1.0, 1.0),
        (0.0, 2048.0, 0.0),
    ] {
        assert!(block_position(pos).is_none());
    }
    assert_eq!(block_position((-0.01, -1.01, 0.0)), Some([-1, -2, 0]));
}
#[test]
fn uuid_membership_and_limits_are_strict() {
    assert_eq!(
        identity("00000000-0000-0000-0000-000000000001").as_deref(),
        Some(ALICE)
    );
    for bad in [
        "alice",
        "é000000000000000000000000000000",
        "--------00000000000000000000000000000001",
    ] {
        assert!(identity(bad).is_none());
    }
    assert!(
        Policy::build(
            (0..=MAX_REGIONS)
                .map(|i| region(&format!("r{i}")))
                .collect()
        )
        .is_err()
    );
}
#[test]
fn round_trip_all_fields_and_bad_input_never_loads_partially() {
    let mut r = region("r");
    r.members.insert(BOB.into());
    r.priority = -42;
    r.flags = [Rule::Members; 9];
    assert_eq!(decode(&encode(&[r.clone()])), (vec![r], 0));
    for text in [
        "garbage".into(),
        format!("{}garbage\n", encode(&[region("r")])),
        format!("{HEADER}{}", "x".repeat(MAX_FILE_BYTES)),
        format!("{HEADER}unknown\n"),
    ] {
        let (rs, bad) = decode(&text);
        assert!(rs.is_empty() && bad > 0);
    }
}
#[test]
fn a_region_without_owners_protects_from_everyone_and_survives_storage() {
    let mut r = Region::new("box", "world", Bounds::point([1, 2, 3]), None);
    r.flags[Flag::Interact as usize] = Rule::Deny;
    assert_eq!(decode(&encode(&[r.clone()])), (vec![r.clone()], 0));
    let p = policy(vec![r]);
    for flag in [Flag::Build, Flag::Break, Flag::Interact] {
        assert_eq!(
            p.decide("world", [1, 2, 3], flag, Some(ALICE)),
            Verdict::Deny
        );
    }
}
#[test]
fn previous_format_still_loads_and_keeps_its_rule_that_regions_have_owners() {
    let row = "r\tworld\t0,0,0\t1,1,1\t0\tOWNER\t\tunset,unset,unset,unset,unset,unset,unset,unset,unset\n";
    let with = format!("{HEADER_V2}{}", row.replace("OWNER", ALICE));
    assert_eq!(decode(&with).1, 0);
    assert_eq!(decode(&with).0[0].owners.len(), 1);
    assert!(decode(&format!("{HEADER_V2}{}", row.replace("OWNER", ""))).1 > 0);
    assert_eq!(
        decode(&format!("{HEADER}{}", row.replace("OWNER", ""))).1,
        0
    );
}
#[test]
fn needs_name_only_the_flags_that_can_refuse() {
    assert_eq!(policy(vec![]).needs(), 0);
    let all_allow = |name: &str| {
        let mut r = region(name);
        r.flags = [Rule::Allow; 9];
        r
    };
    assert_eq!(policy(vec![all_allow("r")]).needs(), 0);
    assert_eq!(
        policy(vec![region("r")]).needs(),
        Flag::Build.bit() | Flag::Break.bit() | Flag::Interact.bit() | Flag::Explosions.bit()
    );
    // A global region does nothing until a flag is set on it.
    assert_eq!(policy(vec![region(GLOBAL)]).needs(), 0);
    let mut g = all_allow(GLOBAL);
    g.flags[Flag::Entry as usize] = Rule::Members;
    assert_eq!(policy(vec![g]).needs(), Flag::Entry.bit());
    let mut r = all_allow("r");
    r.flags[Flag::LiquidFlow as usize] = Rule::Deny;
    assert_eq!(policy(vec![r]).needs(), Flag::LiquidFlow.bit());
}
#[test]
fn a_nameless_actor_is_never_allowed_more_than_a_named_one() {
    // The event path asks without an identity first, and looks the player up only on a refusal.
    for rule in [Rule::Unset, Rule::Allow, Rule::Deny, Rule::Members] {
        for flag in Flag::ALL {
            let mut r = region("r");
            r.flags[flag as usize] = rule;
            let p = policy(vec![r]);
            if p.decide("world", [1, 70, 1], flag, None) == Verdict::Allow {
                for actor in [ALICE, BOB] {
                    assert_eq!(
                        p.decide("world", [1, 70, 1], flag, Some(actor)),
                        Verdict::Allow
                    );
                }
            }
        }
    }
}
#[test]
fn unknown_world_uses_cuboids_at_those_coordinates_and_never_a_global() {
    let mut r = region("r");
    r.flags[Flag::LiquidFlow as usize] = Rule::Deny;
    let mut g = region(GLOBAL);
    g.world = "nether".into();
    g.flags[Flag::LiquidFlow as usize] = Rule::Deny;
    let p = policy(vec![r, g]);
    let refusing = |pos| {
        let mut out = Vec::new();
        assert!(p.refusing_worlds(pos, Flag::LiquidFlow, &mut out));
        out
    };
    assert_eq!(refusing([1, 70, 1]), vec!["world".to_owned()]);
    assert!(refusing([500, 70, 1]).is_empty());
}
#[test]
fn unknown_target_in_reach_refuses_only_when_something_there_could() {
    let area = |x: i32| Bounds {
        min: [x, 60, 0],
        max: [x + 2, 62, 2],
    };
    let mut open = region("open");
    open.flags[Flag::Interact as usize] = Rule::Allow;
    let p = policy(vec![region("closed")]);
    let within =
        |p: &Policy, world, x, actor| p.decide_within(world, area(x), Flag::Interact, actor);
    assert_eq!(within(&p, "world", 0, None), Verdict::Deny);
    assert_eq!(within(&p, "world", 0, Some(ALICE)), Verdict::Allow);
    assert_eq!(within(&p, "world", 50, None), Verdict::Allow);
    assert_eq!(within(&p, "nether", 0, None), Verdict::Allow);
    assert_eq!(
        within(&policy(vec![open]), "world", 0, None),
        Verdict::Allow
    );
    let mut g = region(GLOBAL);
    g.flags[Flag::Interact as usize] = Rule::Deny;
    assert_eq!(
        within(&policy(vec![g]), "world", 50, Some(ALICE)),
        Verdict::Deny
    );
}
#[test]
fn only_creatures_count_as_mobs() {
    for mob in [
        "minecraft:zombie",
        "cow",
        "minecraft:wither",
        "villager",
        "minecraft:unknown_new",
    ] {
        assert!(is_mob(mob), "{mob}");
    }
    for thing in [
        "minecraft:item",
        "experience_orb",
        "minecraft:arrow",
        "spectral_arrow",
        "minecraft:oak_boat",
        "bamboo_chest_raft",
        "minecart",
        "hopper_minecart",
        "block_display",
        "text_display",
        "interaction",
        "armor_stand",
        "minecraft:falling_block",
        "tnt",
        "lightning_bolt",
        "small_fireball",
        "splash_potion",
        "glow_item_frame",
        "player",
    ] {
        assert!(!is_mob(thing), "{thing}");
    }
}
#[test]
fn refusal_lines_are_throttled_per_player_and_bounded() {
    let mut t = Throttle::default();
    assert!(t.allow(1, 0));
    assert!(!t.allow(1, Throttle::INTERVAL_MS - 1));
    assert!(t.allow(2, 1));
    assert!(t.allow(1, Throttle::INTERVAL_MS));
    t.forget(1);
    assert!(t.allow(1, Throttle::INTERVAL_MS + 1));
    let mut full = Throttle::default();
    for player in 0..MAX_SELECTIONS as u128 {
        assert!(full.allow(player, 0));
    }
    assert!(!full.allow(u128::MAX, 1));
    assert_eq!(full.len(), MAX_SELECTIONS);
    // Old entries make room: nothing grows and nobody stays silent for long.
    assert!(full.allow(u128::MAX, Throttle::INTERVAL_MS));
    assert_eq!(full.len(), 1);
}
#[test]
fn selection_sorts_and_drops_cross_world_corner() {
    let mut s = Selection::default();
    s.set("world", true, [5, 70, 9]);
    assert!(s.bounds().is_none());
    s.set("world", false, [1, 80, 2]);
    assert_eq!(
        s.bounds(),
        Some(Bounds {
            min: [1, 70, 2],
            max: [5, 80, 9]
        })
    );
    s.set("nether", true, [0, 0, 0]);
    assert!(s.pos2.is_none());
}
#[test]
fn index_1000_sparse_regions_visits_only_a_small_path() {
    let p = policy(
        (0..1000)
            .map(|i| {
                let mut r = region(&format!("r{i}"));
                r.bounds = Bounds {
                    min: [i * 32, 0, 0],
                    max: [i * 32 + 15, 255, 15],
                };
                r
            })
            .collect(),
    );
    for i in 0..1000 {
        let (v, visits) = p.decide_counted("world", [i * 32 + 1, 64, 1], Flag::Break, None);
        assert_eq!(v, Verdict::Deny);
        assert!(visits < 40, "{visits}");
    }
}
#[test]
fn overlap_exhaustion_never_allows_even_a_member() {
    let p = policy((0..1000).map(|i| region(&format!("r{i}"))).collect());
    assert_eq!(
        p.decide("world", [1, 70, 1], Flag::Break, Some(ALICE)),
        Verdict::Unavailable
    );
}
#[test]
fn entry_blocks_crossing_not_movement_inside_or_escape() {
    let mut r = region("r");
    r.flags[Flag::Entry as usize] = Rule::Deny;
    let p = policy(vec![r]);
    assert!(
        p.entry("world", Some([-1, 70, 1]), [1, 70, 1], None)
            .denied()
    );
    assert!(
        !p.entry("world", Some([1, 70, 1]), [2, 70, 1], None)
            .denied()
    );
    assert!(
        !p.entry("world", Some([1, 70, 1]), [11, 70, 1], None)
            .denied()
    );
    assert!(
        p.entry_path("world", (-1.5, 70.0, 1.5), (11.5, 70.0, 1.5), None)
            .denied()
    );
    assert!(
        !p.entry_path("world", (-1.5, 90.0, 1.5), (11.5, 90.0, 1.5), None)
            .denied()
    );
}
#[test]
fn index_matches_reference_for_seeded_overlap_and_boundaries() {
    let regions: Vec<_> = (0..200)
        .map(|i| {
            let mut r = region(&format!("r{i}"));
            r.bounds = Bounds {
                min: [i * 9 - 1000, -10, i % 3],
                max: [i * 9 - 980, 10, 10],
            };
            r.priority = i % 5;
            r.flags[0] = if i % 3 == 0 { Rule::Allow } else { Rule::Deny };
            r
        })
        .collect();
    let p = policy(regions.clone());
    for x in -1020..850 {
        let mut v = Vote::default();
        for r in &regions {
            if r.bounds.contains([x, 0, 5]) {
                v.add(r.priority, r.flags[0] == Rule::Allow);
            }
        }
        assert_eq!(
            p.decide("world", [x, 0, 5], Flag::Build, None).denied(),
            v.denied
        );
    }
}
