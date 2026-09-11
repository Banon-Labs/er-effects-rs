//! What the character is still entitled to be holding once an import has finished.
//!
//! # The invariant
//!
//! Every gear entry the character holds after an import is a copy the build's grant list names,
//! in no more items of it than that list asks for, and every copy that is not has to appear by
//! name in the import's own report with the reason it could not be moved.
//!
//! This module makes the first two thirds of that sentence a function rather than a habit.
//! [`plan_sweep`] gives *every* entry exactly one [`Disposition`], so an entry cannot fall out of
//! the accounting by taking an early exit, and [`survivors`] is the same function run again over
//! the state the pass produced -- which makes the invariant a fixpoint rather than a second
//! opinion about it:
//!
//! ```text
//! survivors(apply(plan_sweep(before, allowance)), allowance) is empty
//! ```
//!
//! # Why this is not a list of cases
//!
//! The pass it serves grew one branch per reported failure: a shield that survived because the
//! shelf count was stale, a shield that survived because the copies being kept were not counted,
//! a shield that survived because taking its Ash of War off changed its item id. Each fix was
//! right and none of them was a rule, so the next shape of the same failure needed a fourth.
//!
//! What all three have in common is that the pass was answering a question about *copies* in a
//! vocabulary of *ids*, over a population it modelled instead of measured. So the decision here
//! is made per entry, keyed on the one name that identifies a copy -- the `GaItemHandle` -- and
//! every category that genuinely behaves differently does so through a named property of the
//! item: its [`Category`], whether it is an [`is_engine_placeholder`], what [`identity`] two
//! copies share. None of them is a branch that names the item.
//!
//! # What the allowance is counted in
//!
//! Items, not entries. A build asking for one Serpent Crest Shield entitles the character to one,
//! however many entries the inventory files them under; a build asking for ninety-nine arrows
//! entitles it to ninety-nine, which may arrive as a single stack. An entry whose quantity runs
//! past what is left of its budget is kept in part and surplus for the remainder, because holding
//! six of something a build asks five of is exactly the state this module exists to forbid.

use std::collections::{BTreeMap, BTreeSet};

use crate::plan::{Grant, split_armament_id};

/// The nibble at the top of a category-tagged item id.
pub const ITEM_CATEGORY_MASK: u32 = 0xF000_0000;

/// The row id under the category nibble.
pub const ITEM_ID_ROW_MASK: u32 = 0x0FFF_FFFF;

/// What kind of thing an item id names, read off its category nibble.
///
/// The game files every item under one of these and the nibble is the whole of the answer, so a
/// pass that wants to act on gear and leave a player's belongings alone can say which is which
/// from the id itself rather than from a list of exceptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// `EquipParamWeapon`, ammunition included -- an arrow is a weapon row.
    Armament,
    /// `EquipParamProtector`.
    Protector,
    /// `EquipParamAccessory`, which is what the game calls a talisman.
    Accessory,
    /// `EquipParamGoods`: consumables, crafting materials, key items, spells, remembrances.
    Goods,
    /// `EquipParamGem`, an Ash of War carried as an item.
    Gem,
    /// A nibble none of the above claims, carried whole so a log line can name it.
    Other(u32),
}

impl Category {
    /// Which category an item id belongs to.
    ///
    /// ```
    /// use er_build_import_core::sweep::Category;
    /// assert_eq!(Category::of(0x0000_2710), Category::Armament);
    /// assert_eq!(Category::of(0x1000_2710), Category::Protector);
    /// assert_eq!(Category::of(0x2000_0BB8), Category::Accessory);
    /// assert_eq!(Category::of(0x4000_0084), Category::Goods);
    /// assert_eq!(Category::of(0x8000_2AF8), Category::Gem);
    /// ```
    #[must_use]
    pub fn of(item_id: u32) -> Self {
        match item_id & ITEM_CATEGORY_MASK {
            0x0000_0000 => Self::Armament,
            0x1000_0000 => Self::Protector,
            0x2000_0000 => Self::Accessory,
            0x4000_0000 => Self::Goods,
            0x8000_0000 => Self::Gem,
            other => Self::Other(other >> 28),
        }
    }

    /// Whether this is gear: something a build dresses the character in, rather than something
    /// the player keeps.
    ///
    /// A build is a statement about what the character wears and fights with, so armaments,
    /// armour and talismans it does not name are the previous build still in the pockets.
    /// Everything else is the player's own belongings and an import has no opinion about it.
    #[must_use]
    pub fn is_gear(self) -> bool {
        matches!(self, Self::Armament | Self::Protector | Self::Accessory)
    }

    /// Lower-case name for a log line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Armament => "armament",
            Self::Protector => "armour",
            Self::Accessory => "talisman",
            Self::Goods => "goods",
            Self::Gem => "ash of war",
            Self::Other(_) => "unknown category",
        }
    }
}

/// The category-tagged id of the Unarmed fist, which is what a cleared hand holds.
///
/// `GetDefaultUnarmedParamId` returns `0x1ADB0`; the weapon category nibble is zero, so the
/// tagged id is the param row itself.
pub const UNARMED_ITEM_ID: u32 = 0x0001_ADB0;

/// The category-tagged ids of the four empty armour pieces, head first.
///
/// `GetDefaultItemIdForEmptyProtectorSlot` is a four-way constant switch returning exactly these,
/// already tagged as the game returns them.
pub const EMPTY_PROTECTOR_ITEM_IDS: [u32; 4] = [0x1000_2710, 0x1000_2774, 0x1000_27D8, 0x1000_283C];

/// Whether an item id is one the engine writes to mean "this position is empty".
///
/// Clearing a hand does not leave it holding nothing: the engine puts the Unarmed fist in it, and
/// clearing a piece of armour puts that slot's empty-piece row in it. Both are ordinary inventory
/// entries in gear categories, so a pass that looks only at the nibble finds them, tries to move
/// them, and is refused because they are worn. They are not possessions; they are how the engine
/// spells an absence.
///
/// A property of the id rather than an exception list each pass carries its own copy of. The two
/// natives named above are the whole source of these five values, and this is the only place they
/// are written down.
///
/// ```
/// use er_build_import_core::sweep::{UNARMED_ITEM_ID, is_engine_placeholder};
/// assert!(is_engine_placeholder(UNARMED_ITEM_ID));
/// assert!(is_engine_placeholder(0x1000_283C));
/// assert!(!is_engine_placeholder(0x0000_2710));
/// ```
#[must_use]
pub fn is_engine_placeholder(item_id: u32) -> bool {
    item_id == UNARMED_ITEM_ID || EMPTY_PROTECTOR_ITEM_IDS.contains(&item_id)
}

/// The identity two copies share when a player reading the menu would call them the same item.
///
/// An armament id carries three things: the base row, the affinity and the upgrade level. A Heavy
/// Longsword +25 and a plain Longsword +0 are both a Longsword, and an allowance of one Longsword
/// has to be spendable by either. [`split_armament_id`] is the arithmetic the exporter already
/// runs for exactly this, so the affinity table stays in one place.
///
/// Everything else is its own identity. Armour cannot be upgraded and a talisman's `+1` is a
/// different item with a different name, so folding those together would spend one allowance on
/// two things the player owns one of each.
///
/// ```
/// use er_build_import_core::sweep::identity;
/// // An Occult Longsword +25 and the bare Longsword are one identity.
/// assert_eq!(identity(1_010_000 + 1_200 + 25), identity(1_010_000));
/// // A talisman keeps its own.
/// assert_ne!(identity(0x2000_0BB8), identity(0x2000_0BB9));
/// ```
#[must_use]
pub fn identity(item_id: u32) -> u32 {
    if Category::of(item_id) == Category::Armament {
        split_armament_id(item_id).row
    } else {
        item_id
    }
}

/// One inventory entry, as much of it as this module needs.
///
/// The handle is the only name that tells two copies of one item apart: an Ash of War lives on
/// the instance, so the item id cannot say which shield is the one this import just made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// `GaItemHandle`, or zero for an entry that has none.
    pub handle: u32,
    /// Category-tagged item id.
    pub item_id: u32,
    /// How many the entry holds. Non-positive means the entry names nothing.
    pub quantity: i32,
}

impl Held {
    /// A plain entry of one.
    #[must_use]
    pub fn one(handle: u32, item_id: u32) -> Self {
        Self {
            handle,
            item_id,
            quantity: 1,
        }
    }

    /// A stack.
    #[must_use]
    pub fn stack(handle: u32, item_id: u32, quantity: i32) -> Self {
        Self {
            handle,
            item_id,
            quantity,
        }
    }

    /// The identity this entry counts against.
    #[must_use]
    pub fn identity(&self) -> u32 {
        identity(self.item_id)
    }
}

/// Why an entry is none of the sweep's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Untouchable {
    /// Not one of the three gear categories: a consumable, a crafting material, a key item, a
    /// spell, or an Ash of War carried as an item.
    NotGear,
    /// An id the engine uses to spell an empty position. See [`is_engine_placeholder`].
    EnginePlaceholder,
    /// The entry holds nothing, so there is nothing to move and nothing to count.
    ///
    /// Its own disposition rather than an early exit. An entry that leaves the classification
    /// without one is exactly the shape that disappears out of a denominator, and the pass this
    /// replaces counted such an entry as found and then recorded no outcome for it.
    EmptyStack,
}

impl Untouchable {
    /// The sentence for a log line.
    #[must_use]
    pub fn explain(self) -> &'static str {
        match self {
            Self::NotGear => "not an armament, a piece of armour or a talisman",
            Self::EnginePlaceholder => "the engine's own id for an empty position, not an item",
            Self::EmptyStack => "the entry holds nothing",
        }
    }
}

/// Why an entry stays on the character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// This exact instance was minted or adopted by this import, named by its handle.
    ///
    /// Kept whatever the allowance says. An Ash of War lives on the instance, so the item id
    /// cannot tell this copy from the one it replaces, and the copy the import produced is by
    /// definition the one the build asked for.
    Pinned,
    /// The build asks for this identity and the allowance had room for the whole entry.
    WithinAllowance,
}

impl Kept {
    /// The sentence for a log line.
    #[must_use]
    pub fn explain(self) -> &'static str {
        match self {
            Self::Pinned => "this import made or adopted this exact copy",
            Self::WithinAllowance => "the build names this item and had allowance left for it",
        }
    }
}

/// What the sweep decided about one entry.
///
/// Total and disjoint by construction: [`plan_sweep`] produces exactly one of these per entry, in
/// the same order, so `dispositions.len() == entries.len()` is the accounting identity and there
/// is no path by which an entry has no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The pass has no business with this entry.
    Untouchable(Untouchable),
    /// The entry stays in full.
    Kept(Kept),
    /// Some or all of the entry is more than the build asks for.
    Surplus {
        /// How many of the entry the allowance covers. Zero for an entry the build does not name
        /// at all; positive for a stack that runs past what is left of its budget.
        keep: i32,
        /// How many have to leave for the character to hold no more than the build asks for.
        shed: i32,
    },
}

impl Disposition {
    /// Whether any part of this entry has to leave.
    #[must_use]
    pub fn is_surplus(&self) -> bool {
        matches!(self, Self::Surplus { .. })
    }

    /// How many items have to leave, which is zero for everything that stays.
    #[must_use]
    pub fn shed(&self) -> i32 {
        match self {
            Self::Surplus { shed, .. } => *shed,
            _ => 0,
        }
    }

    /// The sentence for a log line.
    #[must_use]
    pub fn explain(&self) -> String {
        match self {
            Self::Untouchable(why) => why.explain().to_owned(),
            Self::Kept(why) => why.explain().to_owned(),
            Self::Surplus { keep: 0, shed } => {
                format!("the build does not name it, so all {shed} have to go")
            }
            Self::Surplus { keep, shed } => {
                format!("the build asks for {keep} of these, so {shed} have to go")
            }
        }
    }
}

/// One grant's claim on the inventory: the identities that satisfy it and how many items of them
/// the build asks for.
#[derive(Debug, Clone)]
struct Budget {
    identities: Vec<u32>,
    items: i32,
    label: String,
}

/// What the build entitles the character to keep.
///
/// # Why a count and not a set of ids
///
/// It was a set of ids until 2026-09-10, and a set cannot express "one". A build naming one
/// Serpent Crest Shield kept every Serpent Crest Shield in the inventory, because they all share
/// an item id, so the copy this import had just made and the copy the previous build left behind
/// both survived.
///
/// # Why the assignment is a matching and not a first fit
///
/// A name can resolve to more than one row ([`Grant::also_known_as`]), so one entry can satisfy
/// several grants and one grant can be satisfied by several entries. Spending the first budget
/// that fits leaves the build's own item without one when a different order would have covered
/// both, and the pass then evicts the thing it was importing. [`plan_sweep`] therefore computes a
/// maximum assignment, so an entry is surplus only when no arrangement of the allowance covers
/// it, and "the wrong budget got spent" stops being a way for anything to survive or to vanish.
#[derive(Debug, Clone, Default)]
pub struct Allowance {
    budgets: Vec<Budget>,
    pinned: BTreeSet<u32>,
}

impl Allowance {
    /// Build the allowance from the plan's grants and the handles this import produced.
    ///
    /// `pinned` is the set of `GaItemHandle`s the grant minted or adopted. A zero handle names no
    /// instance and is dropped.
    pub fn new(grants: &[Grant], pinned: impl IntoIterator<Item = u32>) -> Self {
        let budgets = grants
            .iter()
            .map(|grant| Budget {
                identities: std::iter::once(grant.item_id)
                    .chain(grant.also_known_as.iter().copied())
                    .map(identity)
                    .collect(),
                items: i32::try_from(grant.quantity).unwrap_or(i32::MAX),
                label: grant.label.clone(),
            })
            .collect();
        Self {
            budgets,
            pinned: pinned.into_iter().filter(|handle| *handle != 0).collect(),
        }
    }

    /// Whether this exact instance is one the import produced.
    #[must_use]
    pub fn is_pinned(&self, handle: u32) -> bool {
        handle != 0 && self.pinned.contains(&handle)
    }

    /// How many instances the import pinned.
    #[must_use]
    pub fn pinned_count(&self) -> usize {
        self.pinned.len()
    }

    /// How many items in total the build asks for, over every grant.
    #[must_use]
    pub fn items(&self) -> i32 {
        self.budgets.iter().map(|budget| budget.items).sum()
    }

    /// How many grants the allowance was built from.
    #[must_use]
    pub fn len(&self) -> usize {
        self.budgets.len()
    }

    /// Whether the build names nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.budgets.is_empty()
    }

    /// The grant labels whose identity set admits `item_id`, for a report that has to say which
    /// line of the build an entry was kept against.
    #[must_use]
    pub fn claimants(&self, item_id: u32) -> Vec<&str> {
        let wanted = identity(item_id);
        self.budgets
            .iter()
            .filter(|budget| budget.identities.contains(&wanted))
            .map(|budget| budget.label.as_str())
            .collect()
    }
}

/// The decision for every entry, in the order they were given.
#[derive(Debug, Clone, Default)]
pub struct SweepPlan {
    dispositions: Vec<Disposition>,
}

impl SweepPlan {
    /// What was decided about the entry at `index`.
    #[must_use]
    pub fn disposition(&self, index: usize) -> Option<Disposition> {
        self.dispositions.get(index).copied()
    }

    /// Every decision, in entry order.
    #[must_use]
    pub fn dispositions(&self) -> &[Disposition] {
        &self.dispositions
    }

    /// The indices of entries some part of which has to leave.
    pub fn surplus(&self) -> impl Iterator<Item = usize> + '_ {
        self.dispositions
            .iter()
            .enumerate()
            .filter(|(_, disposition)| disposition.is_surplus())
            .map(|(index, _)| index)
    }

    /// Totals over the whole classification.
    #[must_use]
    pub fn counts(&self) -> SweepCounts {
        let mut counts = SweepCounts::default();
        for disposition in &self.dispositions {
            match disposition {
                Disposition::Untouchable(_) => counts.untouchable += 1,
                Disposition::Kept(Kept::Pinned) => counts.pinned += 1,
                Disposition::Kept(Kept::WithinAllowance) => counts.within_allowance += 1,
                Disposition::Surplus { shed, .. } => {
                    counts.surplus += 1;
                    counts.shed_items += *shed;
                }
            }
        }
        counts
    }
}

/// Totals over a [`SweepPlan`].
///
/// [`Self::total`] is the accounting identity: it has to equal the number of entries the plan was
/// computed over, because every entry has exactly one disposition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepCounts {
    /// Entries outside the pass entirely.
    pub untouchable: usize,
    /// Entries kept because this import produced them.
    pub pinned: usize,
    /// Entries kept because the build asks for them.
    pub within_allowance: usize,
    /// Entries some part of which has to leave.
    pub surplus: usize,
    /// How many items that comes to.
    pub shed_items: i32,
}

impl SweepCounts {
    /// Every entry the plan was computed over.
    #[must_use]
    pub fn total(&self) -> usize {
        self.untouchable + self.pinned + self.within_allowance + self.surplus
    }

    /// Entries that stay in full.
    #[must_use]
    pub fn kept(&self) -> usize {
        self.pinned + self.within_allowance
    }
}

/// Decide what has to leave the character for the build to be the whole of what it holds.
///
/// Every entry gets exactly one [`Disposition`]. Entries outside the gear categories, engine
/// placeholders and empty stacks are answered first and never reach the allowance; of the rest,
/// the instances this import produced are pinned, and the remainder are assigned to the build's
/// budgets by a maximum matching, so an entry is surplus only when no arrangement of the
/// allowance covers it.
#[must_use]
pub fn plan_sweep(entries: &[Held], allowance: &Allowance) -> SweepPlan {
    let mut dispositions = vec![Disposition::Untouchable(Untouchable::NotGear); entries.len()];

    // The order entries are offered a budget: the instances this import produced first, so an
    // older twin sharing their identity cannot spend the allowance ahead of them, then everything
    // else in inventory order.
    let mut candidates: Vec<usize> = Vec::new();
    let mut pinned: Vec<usize> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if !Category::of(entry.item_id).is_gear() {
            dispositions[index] = Disposition::Untouchable(Untouchable::NotGear);
        } else if is_engine_placeholder(entry.item_id) {
            dispositions[index] = Disposition::Untouchable(Untouchable::EnginePlaceholder);
        } else if entry.quantity <= 0 {
            dispositions[index] = Disposition::Untouchable(Untouchable::EmptyStack);
        } else if allowance.is_pinned(entry.handle) {
            dispositions[index] = Disposition::Kept(Kept::Pinned);
            pinned.push(index);
        } else {
            candidates.push(index);
        }
    }

    let mut assignment = Assignment::new(entries, allowance);
    // A pinned entry stays whatever happens, and still spends the allowance it fits, so the older
    // copy it replaces cannot also be kept against the same grant.
    for index in pinned {
        assignment.offer(index);
    }
    for &index in &candidates {
        let kept = assignment.offer(index);
        let quantity = entries[index].quantity;
        dispositions[index] = if kept >= quantity {
            Disposition::Kept(Kept::WithinAllowance)
        } else {
            Disposition::Surplus {
                keep: kept,
                shed: quantity - kept,
            }
        };
    }

    SweepPlan { dispositions }
}

/// The entries that break the invariant: gear the character holds that the build does not entitle
/// it to.
///
/// The same function as [`plan_sweep`], run again over the state a pass produced. That is what
/// makes the invariant a fixpoint rather than a second opinion about it -- if the sweep did its
/// job, running it again finds nothing left to do.
///
/// ```
/// use er_build_import_core::sweep::{Allowance, Held, survivors};
/// let allowance = Allowance::default();
/// // A consumable is never a survivor: the pass has no business with it.
/// assert!(survivors(&[Held::one(0, 0x4000_0084)], &allowance).is_empty());
/// // A talisman the build does not name is one.
/// assert_eq!(survivors(&[Held::one(0, 0x2000_0BB8)], &allowance), vec![0]);
/// ```
#[must_use]
pub fn survivors(entries: &[Held], allowance: &Allowance) -> Vec<usize> {
    plan_sweep(entries, allowance).surplus().collect()
}

/// Apply a plan to the entries it was computed over, as the game would if every move landed.
///
/// The reference the runtime is measured against, and the left-hand side of the fixpoint. A
/// surplus entry keeps the part the allowance covers and loses the rest; an entry that keeps
/// nothing disappears.
#[must_use]
pub fn apply(entries: &[Held], plan: &SweepPlan) -> Vec<Held> {
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        match plan.disposition(index) {
            Some(Disposition::Surplus { keep, .. }) => {
                if keep > 0 {
                    out.push(Held {
                        quantity: keep,
                        ..*entry
                    });
                }
            }
            _ => out.push(*entry),
        }
    }
    out
}

/// How many items of each identity a set of entries holds, gear only.
///
/// The other direction of the same question, for a report that has to say "the build asks for one
/// of these and the character holds three".
#[must_use]
pub fn held_by_identity(entries: &[Held]) -> BTreeMap<u32, i64> {
    let mut counts: BTreeMap<u32, i64> = BTreeMap::new();
    for entry in entries {
        if entry.quantity <= 0
            || !Category::of(entry.item_id).is_gear()
            || is_engine_placeholder(entry.item_id)
        {
            continue;
        }
        *counts.entry(entry.identity()).or_default() += i64::from(entry.quantity);
    }
    counts
}

/// Assigns entries to budgets, maximally.
///
/// Kuhn's augmenting-path matching with a capacity per budget. The recursion is bounded by the
/// number of budgets, which is the number of rows in the build.
struct Assignment<'a> {
    entries: &'a [Held],
    budgets: &'a [Budget],
    /// Items each budget has handed out.
    used: Vec<i32>,
    /// Which entries each budget is carrying, so one can be displaced to make room.
    occupants: Vec<Vec<usize>>,
    /// Which budget each entry sits in, and for how many items.
    seat: Vec<Option<(usize, i32)>>,
}

impl<'a> Assignment<'a> {
    fn new(entries: &'a [Held], allowance: &'a Allowance) -> Self {
        Self {
            entries,
            budgets: &allowance.budgets,
            used: vec![0; allowance.budgets.len()],
            occupants: vec![Vec::new(); allowance.budgets.len()],
            seat: vec![None; entries.len()],
        }
    }

    /// Offer one entry the allowance, displacing an earlier occupant where that frees a seat for
    /// both. Returns how many of the entry's items are covered.
    fn offer(&mut self, index: usize) -> i32 {
        let mut visited = vec![false; self.budgets.len()];
        self.augment(index, &mut visited)
    }

    fn augment(&mut self, index: usize, visited: &mut [bool]) -> i32 {
        let wanted = self.entries[index].identity();
        let quantity = self.entries[index].quantity.max(1);
        // The largest partial seat found so far, taken only when no budget covers the entry whole.
        let mut best = 0;
        let mut best_budget = None;
        for budget_index in 0..self.budgets.len() {
            if visited[budget_index] || !self.budgets[budget_index].identities.contains(&wanted) {
                continue;
            }
            visited[budget_index] = true;
            let free = self.budgets[budget_index].items - self.used[budget_index];
            if free >= quantity {
                self.sit(index, budget_index, quantity);
                return quantity;
            }
            if free > best {
                best = free;
                best_budget = Some(budget_index);
            }
            // The budget is short. Move one of its occupants elsewhere and ask again.
            for position in 0..self.occupants[budget_index].len() {
                let other = self.occupants[budget_index][position];
                let Some((_, items)) = self.seat[other] else {
                    continue;
                };
                self.occupants[budget_index].remove(position);
                self.used[budget_index] -= items;
                self.seat[other] = None;
                if self.augment(other, visited) >= items {
                    let free = self.budgets[budget_index].items - self.used[budget_index];
                    if free >= quantity {
                        self.sit(index, budget_index, quantity);
                        return quantity;
                    }
                    if free > best {
                        best = free;
                        best_budget = Some(budget_index);
                    }
                    break;
                }
                // The move failed, so the occupant goes back where it was, into the space the
                // failed augment left for it.
                self.used[budget_index] += items;
                self.occupants[budget_index].insert(position, other);
                self.seat[other] = Some((budget_index, items));
            }
        }
        match best_budget {
            Some(budget_index) if best > 0 => {
                self.sit(index, budget_index, best);
                best
            }
            _ => 0,
        }
    }

    fn sit(&mut self, index: usize, budget_index: usize, items: i32) {
        self.used[budget_index] += items;
        self.occupants[budget_index].push(index);
        self.seat[index] = Some((budget_index, items));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::NO_SKILL;

    fn grant_of(item_id: u32, quantity: u32, also: &[u32]) -> Grant {
        Grant {
            item_id,
            also_known_as: also.to_vec(),
            quantity,
            reinforce_lv: 0,
            upgrade_is_character_default: true,
            weapon_skill: NO_SKILL,
            label: format!("item 0x{item_id:08X}"),
            pot_group: None,
            armament: false,
        }
    }

    /// Every entry has exactly one disposition, whatever it is.
    #[test]
    fn the_classification_is_total() {
        let entries = [
            Held::one(0, 0x4000_0084),
            Held::one(0, UNARMED_ITEM_ID),
            Held::stack(0, 0x2000_0BB8, 0),
            Held::one(7, 0x2000_0BB8),
            Held::one(0, 0x1000_2AF8),
        ];
        let allowance = Allowance::new(&[grant_of(0x2000_0BB8, 1, &[])], [7]);
        let plan = plan_sweep(&entries, &allowance);
        assert_eq!(plan.dispositions().len(), entries.len());
        assert_eq!(plan.counts().total(), entries.len());
    }

    /// Consumables, materials and spells are outside the pass by category, not by exception.
    #[test]
    fn nothing_outside_the_gear_categories_is_ever_surplus() {
        let entries = [
            Held::stack(0, 0x4000_0084, 99),
            Held::one(0, 0x8000_2AF8),
            Held::stack(0, 0x4000_00C8, 5),
        ];
        assert!(survivors(&entries, &Allowance::default()).is_empty());
    }

    /// A cleared hand holds the fist and a cleared armour slot holds its empty piece. Neither is
    /// a possession, and the sweep that tried to deposit them was refused once per copy.
    #[test]
    fn the_engines_empty_slot_rows_are_not_gear_to_move() {
        let mut entries = vec![Held::one(0, UNARMED_ITEM_ID)];
        entries.extend(EMPTY_PROTECTOR_ITEM_IDS.map(|id| Held::one(0, id)));
        assert!(survivors(&entries, &Allowance::default()).is_empty());
        let plan = plan_sweep(&entries, &Allowance::default());
        assert_eq!(plan.counts().untouchable, 5);
    }

    /// An entry holding nothing is answered, not skipped. The pass this replaces counted it as
    /// found and then recorded no outcome for it at all.
    #[test]
    fn an_empty_stack_has_its_own_answer_rather_than_no_answer() {
        let entries = [Held::stack(0, 0x2000_0BB8, 0)];
        let plan = plan_sweep(&entries, &Allowance::default());
        assert_eq!(
            plan.disposition(0),
            Some(Disposition::Untouchable(Untouchable::EmptyStack))
        );
    }

    /// The allowance is a count, so a build naming one shield keeps one shield.
    #[test]
    fn the_allowance_is_a_count_not_a_set() {
        let entries = [
            Held::one(0, 0x2000_0BB8),
            Held::one(0, 0x2000_0BB8),
            Held::one(0, 0x2000_0BB8),
        ];
        let allowance = Allowance::new(&[grant_of(0x2000_0BB8, 1, &[])], []);
        assert_eq!(survivors(&entries, &allowance).len(), 2);
    }

    /// The upgrade level and the infusion are not part of an armament's identity, so the copy the
    /// grant just minted at +25 spends the allowance the plan wrote at +0.
    #[test]
    fn an_armaments_allowance_ignores_its_level_and_infusion() {
        let bare = 1_010_000;
        let occult_25 = bare + 1_200 + 25;
        let entries = [Held::one(0, bare), Held::one(99, occult_25)];
        let allowance = Allowance::new(&[grant_of(bare, 1, &[])], [99]);
        let plan = plan_sweep(&entries, &allowance);
        // The minted copy is kept and the old bare one goes, in that direction and not the other.
        assert_eq!(plan.disposition(1), Some(Disposition::Kept(Kept::Pinned)));
        assert!(plan.disposition(0).expect("classified").is_surplus());
    }

    /// A pinned instance is kept even when the allowance is already spent, because the item id
    /// cannot tell it from the copy it replaces.
    #[test]
    fn a_pinned_instance_survives_an_exhausted_allowance() {
        let entries = [Held::one(11, 0x2000_0BB8), Held::one(12, 0x2000_0BB8)];
        let allowance = Allowance::new(&[grant_of(0x2000_0BB8, 1, &[])], [11, 12]);
        let plan = plan_sweep(&entries, &allowance);
        assert_eq!(plan.counts().pinned, 2);
        assert_eq!(plan.counts().surplus, 0);
    }

    /// One entry can satisfy several grants. Spending the first that fits leaves the build's own
    /// item without one, and the pass then evicts the thing it was importing.
    #[test]
    fn a_shared_alternate_does_not_starve_the_grant_that_needs_it() {
        let common = 0x2000_0BB8;
        let only = 0x2000_0BB9;
        // The first grant accepts either row; the second accepts only the second row.
        let grants = [grant_of(common, 1, &[only]), grant_of(only, 1, &[])];
        let allowance = Allowance::new(&grants, []);
        // The entry that could only ever satisfy the second grant comes first.
        let entries = [Held::one(0, only), Held::one(0, common)];
        let plan = plan_sweep(&entries, &allowance);
        assert_eq!(plan.counts().surplus, 0, "both entries have a budget");
    }

    /// A stack that runs past the allowance is kept in part, not dropped whole and not kept whole.
    #[test]
    fn a_stack_is_kept_up_to_the_allowance_and_sheds_the_rest() {
        let arrows = 0x0000_0BB8;
        let entries = [Held::stack(0, arrows, 99)];
        let allowance = Allowance::new(&[grant_of(arrows, 40, &[])], []);
        let plan = plan_sweep(&entries, &allowance);
        assert_eq!(
            plan.disposition(0),
            Some(Disposition::Surplus { keep: 40, shed: 59 })
        );
    }

    /// The invariant, as a fixpoint: applying the plan leaves nothing for a second pass to do.
    #[test]
    fn applying_the_plan_satisfies_the_invariant() {
        let entries = [
            Held::one(0, 0x2000_0BB8),
            Held::one(0, 0x2000_0BB8),
            Held::one(5, 1_010_000 + 25),
            Held::one(0, 1_010_000),
            Held::stack(0, 0x4000_0084, 12),
            Held::one(0, UNARMED_ITEM_ID),
        ];
        let allowance = Allowance::new(
            &[grant_of(0x2000_0BB8, 1, &[]), grant_of(1_010_000, 1, &[])],
            [5],
        );
        let plan = plan_sweep(&entries, &allowance);
        let after = apply(&entries, &plan);
        assert!(survivors(&after, &allowance).is_empty());
    }

    /// The report has to be able to say which line of the build an entry was kept against.
    #[test]
    fn the_allowance_names_the_grants_that_would_admit_an_item() {
        let allowance = Allowance::new(&[grant_of(0x2000_0BB8, 1, &[0x2000_0BB9])], []);
        assert_eq!(allowance.claimants(0x2000_0BB9), vec!["item 0x20000BB8"]);
        assert!(allowance.claimants(0x2000_0BBA).is_empty());
    }
}
