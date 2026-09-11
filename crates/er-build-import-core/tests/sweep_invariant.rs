//! The import's inventory invariant, over generated prior inventories rather than chosen ones.
//!
//! # What this is for
//!
//! The sweep that sends the previous build's gear back to storage was fixed three times in one
//! day, each time against the case that had just been reported: a shield that survived a stale
//! shelf count, a shield that survived an uncounted kept copy, a shield that survived its own Ash
//! of War changing its item id. Three fixes, three cases, no rule -- and a fourth live run found
//! five more survivors.
//!
//! So the rule is asserted here instead, against pairs nobody chose. Each case generates a prior
//! inventory and a target build, runs [`plan_sweep`], applies it, and checks the invariant on the
//! result. A sweep that handles the three reported cases and misses the fourth shape fails one of
//! these long before it reaches a player.
//!
//! # The invariant, in three parts
//!
//! * **Totality.** Every entry has exactly one disposition. An entry that leaves the
//!   classification without one is how a survivor becomes invisible rather than reported.
//! * **Exactness.** After the plan is applied, no identity is held in more items than the build
//!   asks for, and nothing outside the gear categories has been touched at all.
//! * **Maximality.** An entry is surplus only when no arrangement of the allowance covers it.
//!   Without this half the invariant is satisfiable by evicting everything, which would pass a
//!   test and empty a player's pockets.
//!
//! Maximality is checked against an independent oracle -- a max-flow over identities and budgets,
//! written here and sharing no code with the thing it judges.

use std::collections::{BTreeMap, BTreeSet};

use er_build_import_core::plan::{Grant, NO_SKILL};
use er_build_import_core::sweep::{
    Allowance, Category, Disposition, EMPTY_PROTECTOR_ITEM_IDS, Held, Kept, UNARMED_ITEM_ID,
    Untouchable, apply, identity, is_engine_placeholder, plan_sweep, survivors,
};

/// How many generated pairs each family runs.
///
/// Large enough that a shape appearing in one case in a thousand is found on nearly every run,
/// small enough that the whole file is a fraction of a second. The seed is fixed, so a failure is
/// reproducible by its case number rather than by luck.
const CASES: u64 = 4000;

/// A deterministic generator. `SplitMix64`, which is short enough to read and good enough that
/// consecutive draws do not correlate -- a generator whose low bits march in step would make
/// every generated inventory the same shape.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..bound`.
    fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        (self.next() % u64::from(bound)) as u32
    }

    /// True one time in `odds`.
    fn one_in(&mut self, odds: u32) -> bool {
        self.below(odds) == 0
    }
}

/// The pool of items a generated case draws from.
///
/// Deliberately small and deliberately overlapping: repeated identities are the whole point, and
/// a pool of a hundred distinct items would generate inventories in which nothing ever collides.
struct Universe {
    /// Base `EquipParamWeapon` rows, category nibble zero.
    armaments: Vec<u32>,
    /// Category-tagged protector rows.
    protectors: Vec<u32>,
    /// Category-tagged accessory rows.
    accessories: Vec<u32>,
    /// Things the sweep must never touch: goods, gems, and a category nothing claims.
    outsiders: Vec<u32>,
}

impl Universe {
    fn new() -> Self {
        Self {
            armaments: (0..6).map(|n| 1_010_000 + n * 10_000).collect(),
            protectors: (0..5).map(|n| 0x1000_0000 + 10_000 + n * 100).collect(),
            accessories: (0..5).map(|n| 0x2000_0000 + 3_000 + n).collect(),
            outsiders: vec![
                0x4000_0084,
                0x4000_00C8,
                0x8000_2AF8,
                0x8000_2B5C,
                0x3000_0001,
            ],
        }
    }

    /// One gear id, at a random affinity and level when it is an armament.
    fn gear(&self, rng: &mut Rng) -> u32 {
        match rng.below(3) {
            0 => {
                let row = self.armaments[rng.below(self.armaments.len() as u32) as usize];
                // Affinity is a multiple of 100 inside the row's block; level is the last two
                // digits. Both have to be invisible to the allowance, which is what makes a
                // generated `+25 Occult` copy spend the plan's `+0` budget.
                let affinity = u32::from(rng.below(13) as u16) * 100;
                let level = rng.below(26);
                row + affinity + level
            }
            1 => self.protectors[rng.below(self.protectors.len() as u32) as usize],
            _ => self.accessories[rng.below(self.accessories.len() as u32) as usize],
        }
    }

    fn outsider(&self, rng: &mut Rng) -> u32 {
        self.outsiders[rng.below(self.outsiders.len() as u32) as usize]
    }
}

fn grant_of(item_id: u32, quantity: u32, also: Vec<u32>) -> Grant {
    Grant {
        item_id,
        also_known_as: also,
        quantity,
        reinforce_lv: 0,
        upgrade_is_character_default: true,
        weapon_skill: NO_SKILL,
        label: format!("grant 0x{item_id:08X}"),
        pot_group: None,
        armament: Category::of(item_id) == Category::Armament,
    }
}

/// A generated target build.
fn grants(rng: &mut Rng, universe: &Universe, singletons_only: bool) -> Vec<Grant> {
    let count = rng.below(9);
    (0..count)
        .map(|_| {
            let item = universe.gear(rng);
            // Alternates are the shape that made a first fit spend the wrong budget: a name that
            // resolves to several rows draws on one allowance, and two grants can share a row.
            let also = if rng.one_in(3) {
                vec![universe.gear(rng)]
            } else {
                Vec::new()
            };
            let quantity = if singletons_only {
                1
            } else {
                1 + rng.below(60)
            };
            grant_of(item, quantity, also)
        })
        .collect()
}

/// A generated prior inventory: what the character was carrying before the import.
fn inventory(rng: &mut Rng, universe: &Universe, singletons_only: bool) -> Vec<Held> {
    let count = rng.below(34);
    let mut handle = 1000u32;
    (0..count)
        .map(|_| {
            handle += 1;
            let quantity = if singletons_only {
                1
            } else {
                match rng.below(12) {
                    // A freed entry, which the pass this replaces counted and then said nothing
                    // about.
                    0 => 0,
                    1 => 1 + rng.below(99) as i32,
                    _ => 1,
                }
            };
            let item_id = match rng.below(10) {
                // The player's own belongings, which an import has no opinion about.
                0 | 1 => universe.outsider(rng),
                // What the vacate pass leaves in a hand or an armour slot it has just cleared.
                2 if rng.one_in(2) => UNARMED_ITEM_ID,
                2 => EMPTY_PROTECTOR_ITEM_IDS[rng.below(4) as usize],
                _ => universe.gear(rng),
            };
            Held {
                handle,
                item_id,
                quantity,
            }
        })
        .collect()
}

/// The handles this import would have minted or adopted: copies of what the build names, added to
/// the inventory as the grant pass adds them.
fn mint(rng: &mut Rng, grants: &[Grant], entries: &mut Vec<Held>) -> Vec<u32> {
    let mut pinned = Vec::new();
    let mut handle = 90_000u32;
    for grant in grants {
        if !grant.armament || rng.one_in(3) {
            continue;
        }
        handle += 1;
        // At a level the plan does not name, which is the trap that would have deposited the
        // build's own weapons: the id the plan writes is `+0` and the id the mint produces is not.
        entries.push(Held::one(handle, grant.item_id + rng.below(26)));
        pinned.push(handle);
    }
    pinned
}

/// Every part of the invariant that does not need an independent oracle.
fn check_invariant(case: u64, entries: &[Held], allowance: &Allowance) {
    let plan = plan_sweep(entries, allowance);

    // Totality. Every entry is answered, and the answers add up to the entries.
    assert_eq!(
        plan.dispositions().len(),
        entries.len(),
        "case {case}: an entry left the classification without a disposition"
    );
    assert_eq!(
        plan.counts().total(),
        entries.len(),
        "case {case}: the counts do not add up to the entries they were taken over"
    );

    for (index, entry) in entries.iter().enumerate() {
        let disposition = plan.disposition(index).expect("every entry is classified");
        let untouchable = !Category::of(entry.item_id).is_gear()
            || is_engine_placeholder(entry.item_id)
            || entry.quantity <= 0;
        if untouchable {
            assert!(
                matches!(disposition, Disposition::Untouchable(_)),
                "case {case}: entry {index} (0x{:08X} x{}) is not the sweep's business and was \
                 classified {disposition:?}",
                entry.item_id,
                entry.quantity
            );
        } else {
            assert!(
                !matches!(disposition, Disposition::Untouchable(_)),
                "case {case}: entry {index} (0x{:08X}) is gear and was excused as {disposition:?}",
                entry.item_id
            );
        }
        if allowance.is_pinned(entry.handle) && !untouchable {
            assert_eq!(
                disposition,
                Disposition::Kept(Kept::Pinned),
                "case {case}: entry {index} is an instance this import made and did not survive"
            );
        }
    }

    // Exactness, checked as a fixpoint. If the sweep did its job, running it again finds nothing.
    let after = apply(entries, &plan);
    let left_behind = survivors(&after, allowance);
    assert!(
        left_behind.is_empty(),
        "case {case}: {} entr(ies) survive a sweep that claimed to have finished: {:?}",
        left_behind.len(),
        left_behind
            .iter()
            .map(|index| format!(
                "0x{:08X} x{}",
                after[*index].item_id, after[*index].quantity
            ))
            .collect::<Vec<_>>()
    );

    // Applying it a second time changes nothing, which is the same statement from the other side.
    let twice = apply(&after, &plan_sweep(&after, allowance));
    assert_eq!(
        twice, after,
        "case {case}: a second sweep moved something the first left alone"
    );

    // Nothing outside the three gear categories was touched at all, and neither was any engine
    // placeholder. The pass may not tidy a player's belongings.
    let untouched_before: Vec<Held> = entries
        .iter()
        .copied()
        .filter(|entry| {
            !Category::of(entry.item_id).is_gear() || is_engine_placeholder(entry.item_id)
        })
        .collect();
    let untouched_after: Vec<Held> = after
        .iter()
        .copied()
        .filter(|entry| {
            !Category::of(entry.item_id).is_gear() || is_engine_placeholder(entry.item_id)
        })
        .collect();
    assert_eq!(
        untouched_before, untouched_after,
        "case {case}: the sweep moved something that is not gear"
    );

    // Every instance the import produced is still there.
    for entry in entries {
        if allowance.is_pinned(entry.handle) && entry.quantity > 0 {
            assert!(
                after
                    .iter()
                    .any(|held| held.handle == entry.handle && held.quantity >= entry.quantity),
                "case {case}: the sweep evicted an instance this import had just made"
            );
        }
    }
}

/// Family one: the whole shape, pinned instances included.
#[test]
fn the_invariant_holds_for_any_prior_inventory_and_any_build() {
    let universe = Universe::new();
    for case in 0..CASES {
        let mut rng = Rng::new(case);
        let grants = grants(&mut rng, &universe, false);
        let mut entries = inventory(&mut rng, &universe, false);
        let pinned = mint(&mut rng, &grants, &mut entries);
        let allowance = Allowance::new(&grants, pinned);
        check_invariant(case, &entries, &allowance);
    }
}

/// Family two: singletons only, where the assignment can be judged exactly.
///
/// An entry of one item and a budget counted in items are the same unit, so the most the sweep
/// could possibly keep is a max flow through identities and budgets -- and the sweep has to keep
/// exactly that many. Anything less is the sweep evicting something the build asked for.
#[test]
fn the_sweep_keeps_as_much_as_any_arrangement_of_the_allowance_could() {
    let universe = Universe::new();
    for case in 0..CASES {
        let mut rng = Rng::new(case ^ 0x5EED);
        let grants = grants(&mut rng, &universe, true);
        let entries = inventory(&mut rng, &universe, true);
        let allowance = Allowance::new(&grants, []);
        check_invariant(case, &entries, &allowance);

        let plan = plan_sweep(&entries, &allowance);
        let kept: i32 = entries
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                matches!(
                    plan.disposition(*index),
                    Some(Disposition::Kept(Kept::WithinAllowance))
                )
            })
            .map(|(_, entry)| entry.quantity)
            .sum();
        let most = max_keepable(&entries, &grants);
        assert_eq!(
            kept, most,
            "case {case}: the sweep kept {kept} of the {most} items the allowance could have \
             covered, so it evicted something a different assignment would have spared"
        );
    }
}

/// Family three: the pathological shapes on purpose.
///
/// A build every one of whose grants accepts the same two rows, over an inventory drawn from
/// those two rows alone. Every entry can satisfy every grant, so which budget an entry takes is
/// arbitrary and a first fit has the most room to pick wrong.
#[test]
fn the_invariant_holds_when_every_grant_accepts_every_entry() {
    let left = 0x2000_0BB8;
    let right = 0x2000_0BB9;
    for case in 0..CASES {
        let mut rng = Rng::new(case ^ 0xC0FFEE);
        let grants: Vec<Grant> = (0..rng.below(6))
            .map(|n| {
                let (item, also) = if n % 2 == 0 {
                    (left, vec![right])
                } else {
                    (right, vec![left])
                };
                grant_of(item, 1 + rng.below(3), also)
            })
            .collect();
        let entries: Vec<Held> = (0..rng.below(12))
            .map(|n| Held::one(2000 + n, if rng.one_in(2) { left } else { right }))
            .collect();
        let allowance = Allowance::new(&grants, []);
        check_invariant(case, &entries, &allowance);

        let plan = plan_sweep(&entries, &allowance);
        let kept = plan.counts().within_allowance as i32;
        let most = max_keepable(&entries, &grants);
        assert_eq!(
            kept, most,
            "case {case}: {kept} kept where {most} were coverable, with every grant accepting \
             every entry"
        );
    }
}

/// Family four: a re-import of the build that is already on the character.
///
/// The case the user actually hits. Nothing should move, because the character already holds
/// exactly what the build names -- and the sweep evicting one of those copies is the failure this
/// guards against.
#[test]
fn re_importing_a_build_the_character_already_holds_moves_nothing() {
    let universe = Universe::new();
    for case in 0..CASES {
        let mut rng = Rng::new(case ^ 0xBEEF);
        let grants = grants(&mut rng, &universe, true);
        let mut handle = 500u32;
        let entries: Vec<Held> = grants
            .iter()
            .map(|grant| {
                handle += 1;
                // At the level the character holds it, which the plan writes as `+0`.
                let level = if grant.armament { rng.below(26) } else { 0 };
                Held::one(handle, grant.item_id + level)
            })
            .collect();
        let allowance = Allowance::new(&grants, []);
        check_invariant(case, &entries, &allowance);

        let plan = plan_sweep(&entries, &allowance);
        assert_eq!(
            plan.counts().surplus,
            0,
            "case {case}: a re-import evicted {} of the {} things the build names",
            plan.counts().surplus,
            grants.len()
        );
    }
}

/// The five ids the engine writes to mean "empty" are never gear to move, whatever the build says
/// and whatever else is in the inventory.
#[test]
fn an_empty_position_is_never_evicted_however_the_build_is_shaped() {
    let universe = Universe::new();
    for case in 0..600 {
        let mut rng = Rng::new(case ^ 0xE47);
        let grants = grants(&mut rng, &universe, false);
        let mut entries = inventory(&mut rng, &universe, false);
        entries.push(Held::one(1, UNARMED_ITEM_ID));
        for (n, id) in EMPTY_PROTECTOR_ITEM_IDS.iter().enumerate() {
            entries.push(Held::one(2 + n as u32, *id));
        }
        let allowance = Allowance::new(&grants, []);
        let plan = plan_sweep(&entries, &allowance);
        for (index, entry) in entries.iter().enumerate() {
            if is_engine_placeholder(entry.item_id) {
                assert_eq!(
                    plan.disposition(index),
                    Some(Disposition::Untouchable(Untouchable::EnginePlaceholder)),
                    "case {case}: the engine's own empty-slot row was treated as an item"
                );
            }
        }
    }
}

/// After the sweep, no identity is held in more items than the build asks for.
///
/// The player-facing half of the invariant, checked directly rather than through the fixpoint, so
/// a failure names the item rather than an index.
#[test]
fn no_identity_outlives_its_allowance() {
    let universe = Universe::new();
    for case in 0..CASES {
        let mut rng = Rng::new(case ^ 0xA11);
        let grants = grants(&mut rng, &universe, true);
        let entries = inventory(&mut rng, &universe, true);
        let allowance = Allowance::new(&grants, []);
        let after = apply(&entries, &plan_sweep(&entries, &allowance));

        // The most any identity can be entitled to: every budget that admits it, spent on it.
        let mut ceiling: BTreeMap<u32, i64> = BTreeMap::new();
        for grant in &grants {
            for id in std::iter::once(grant.item_id).chain(grant.also_known_as.iter().copied()) {
                *ceiling.entry(identity(id)).or_default() += i64::from(grant.quantity);
            }
        }
        for entry in &after {
            if !Category::of(entry.item_id).is_gear() || is_engine_placeholder(entry.item_id) {
                continue;
            }
            let held: i64 = after
                .iter()
                .filter(|other| {
                    Category::of(other.item_id).is_gear()
                        && !is_engine_placeholder(other.item_id)
                        && other.identity() == entry.identity()
                })
                .map(|other| i64::from(other.quantity.max(0)))
                .sum();
            let allowed = ceiling.get(&entry.identity()).copied().unwrap_or(0);
            assert!(
                held <= allowed,
                "case {case}: the character still holds {held} of identity {} and the build asks \
                 for at most {allowed}",
                entry.identity()
            );
        }
    }
}

/// The most items any assignment of these entries to these grants could keep.
///
/// An independent oracle, sharing no code with the thing it judges: a max flow from a source
/// through one node per entry, into the budgets that admit it, and out to a sink with each
/// budget's item count as its capacity. Ford-Fulkerson over a graph this small is instant and is
/// easy to read, which is the point -- a clever oracle that is wrong proves nothing.
fn max_keepable(entries: &[Held], grants: &[Grant]) -> i32 {
    // Only entries the pass would consider at all.
    let candidates: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            Category::of(entry.item_id).is_gear()
                && !is_engine_placeholder(entry.item_id)
                && entry.quantity > 0
        })
        .map(|(index, _)| index)
        .collect();

    // Node numbering: 0 = source, 1..=candidates = entries, then budgets, then the sink.
    let entry_base = 1;
    let budget_base = entry_base + candidates.len();
    let sink = budget_base + grants.len();
    let nodes = sink + 1;
    let mut capacity = vec![vec![0i32; nodes]; nodes];

    for (position, index) in candidates.iter().enumerate() {
        capacity[0][entry_base + position] = entries[*index].quantity;
        for (budget, grant) in grants.iter().enumerate() {
            let admits = std::iter::once(grant.item_id)
                .chain(grant.also_known_as.iter().copied())
                .any(|id| identity(id) == entries[*index].identity());
            if admits {
                capacity[entry_base + position][budget_base + budget] = entries[*index].quantity;
            }
        }
    }
    for (budget, grant) in grants.iter().enumerate() {
        capacity[budget_base + budget][sink] = i32::try_from(grant.quantity).unwrap_or(i32::MAX);
    }

    let mut flow = 0;
    loop {
        // One breadth-first search for an augmenting path, which is Edmonds-Karp and bounds the
        // number of rounds without needing an argument about it.
        let mut seen = BTreeSet::new();
        let mut from = vec![usize::MAX; nodes];
        let mut queue = std::collections::VecDeque::from([0usize]);
        seen.insert(0usize);
        while let Some(node) = queue.pop_front() {
            for next in 0..nodes {
                if capacity[node][next] > 0 && seen.insert(next) {
                    from[next] = node;
                    queue.push_back(next);
                }
            }
        }
        if !seen.contains(&sink) {
            return flow;
        }
        let mut bottleneck = i32::MAX;
        let mut node = sink;
        while node != 0 {
            bottleneck = bottleneck.min(capacity[from[node]][node]);
            node = from[node];
        }
        let mut node = sink;
        while node != 0 {
            capacity[from[node]][node] -= bottleneck;
            capacity[node][from[node]] += bottleneck;
            node = from[node];
        }
        flow += bottleneck;
    }
}
