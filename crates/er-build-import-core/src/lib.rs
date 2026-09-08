//! Import an Elden Ring build from an `er-build-planner` share link.
//!
//! The planner's `?b=<id>` links are not encoded payloads -- they are ids for
//! builds stored server-side, fetched with a single unauthenticated
//! `get https://er-inventory-api.nyasu.business/inventories/<id>`. The response
//! identifies every item by **display name**, so importing a build is two
//! problems: parsing the document ([`model`]) and resolving those names to item
//! ids ([`catalog`]) before computing what to grant ([`plan`]).
//!
//! This crate is deliberately host-buildable and free of game and network
//! dependencies, so the whole mapping is provable with `cargo test`.
//!
//! ```
//! use er_build_import_core::{catalog::{entry, Kind, MapCatalog}, model, plan::plan};
//!
//! let doc = model::parse(r#"{
//!     "name": "example", "weaponUpgrade": 25,
//!     "inventory": {"slots": [{"name": "Misericorde", "infusion": "Occult"}]}
//! }"#).expect("valid build document");
//!
//! let catalog = MapCatalog::new().with(Kind::Weapon, "Misericorde", entry(1_070_000));
//! let result = plan(&doc, &catalog);
//!
//! assert!(result.is_complete());
//! assert_eq!(result.grants[0].item_id, 1_070_000 + 1200); // Occult offset
//! assert_eq!(result.grants[0].reinforce_lv, 25);
//! ```

pub mod catalog;
pub mod chr_name;
pub mod class;
pub mod equip;
pub mod model;
pub mod name;
pub mod plan;
pub mod stats;

pub use catalog::{Catalog, Entry, Kind};
pub use equip::{
    Capacity, EquipLedger, EquipPlan, EquipRef, LedgerCounts, PlannedPosition, PositionKind,
    PositionResult,
};
pub use model::BuildDoc;
pub use plan::{Grant, Plan, Unresolved};

/// The API host serving `?b=` builds.
pub const API_HOST: &str = "er-inventory-api.nyasu.business";

/// Path for fetching a build by its share id.
///
/// Verified to need no `Authorization` header: the response is byte-identical
/// with and without a session.
pub fn build_path(share_id: &str) -> String {
    format!("/inventories/{share_id}")
}

/// Config key naming the build to import, in the game-directory `er-quickload.toml`.
///
/// A key in the file the product already ships rather than an environment variable: a
/// runtime-affecting product lever behind an agent-only env var is not a product lever.
pub const BUILD_URL_KEY: &str = "build_url";

/// Pull `build_url` out of an `er-quickload.toml`'s text.
///
/// A deliberate one-key scan rather than a TOML dependency. The product's own parser is private to
/// `er-quickload`, and pulling a whole TOML crate into a crate that needs exactly one string would
/// be the larger sin -- but the scan still has to agree with the file the product writes, which is
/// why it lives here, where `cargo test` can hold it to that.
///
/// Accepts `key = 'value'` and `key = "value"`, ignores comment lines, and returns `None` when the
/// key is absent or its value is empty, so an unset key means "import nothing" rather than "import
/// the empty build".
///
/// ```
/// assert_eq!(
///     er_build_import_core::build_url_from_config("# comment\nbuild_url = 'x?b=abc'\n"),
///     Some("x?b=abc"),
/// );
/// assert_eq!(er_build_import_core::build_url_from_config("slot = 0\n"), None);
/// ```
pub fn build_url_from_config(contents: &str) -> Option<&str> {
    config_value(contents, BUILD_URL_KEY)
}

/// Pull one key's value out of an `er-quickload.toml`'s text.
///
/// The generic form of [`build_url_from_config`], and the same deliberate one-key scan rather than
/// a TOML dependency -- see that function for why. Returns `None` for an absent or empty value, so
/// a key that is present but blank reads as unset rather than as the empty string.
///
/// ```
/// assert_eq!(
///     er_build_import_core::config_value("# a comment\nslot = 0\nname = 'x'\n", "name"),
///     Some("x"),
/// );
/// assert_eq!(er_build_import_core::config_value("name = ''\n", "name"), None);
/// ```
pub fn config_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = value.trim().trim_matches(|c| c == '\'' || c == '"').trim();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// Whether a boolean key is set to `true`. Anything else -- absent, `false`, a typo -- is `false`,
/// because every caller of this is an opt-in switch and a misread must leave it off.
///
/// ```
/// assert!(er_build_import_core::config_flag("export_build_link_on_load = true\n", "export_build_link_on_load"));
/// assert!(!er_build_import_core::config_flag("export_build_link_on_load = false\n", "export_build_link_on_load"));
/// assert!(!er_build_import_core::config_flag("", "export_build_link_on_load"));
/// ```
pub fn config_flag(contents: &str, key: &str) -> bool {
    config_value(contents, key).is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Config key that makes the standalone shell export one build link at character load.
///
/// Read only by `er-build-import`, the harness DLL -- never by the product, whose export is the
/// System>Quit row a player presses. It exists because the content of an exported link is worth
/// checking without a human driving a menu: with this set, one link is written to
/// `er-build-import.log` as soon as the character is in the world, and
/// `scripts/decode-build-link.py --log <that file> --summary` says exactly what it carries.
pub const EXPORT_ON_LOAD_KEY: &str = "export_build_link_on_load";

/// Config key that makes the standalone shell import the configured build and then export the
/// character it produced -- the round trip, whose answer is known in advance.
///
/// Separate from [`EXPORT_ON_LOAD_KEY`], which exports the character as it already is. Measuring
/// an export that ran straight after an import measures the importer as much as the exporter, and
/// the import also grants items, so repeating it inflates the very inventory being exported.
pub const ROUND_TRIP_ON_LOAD_KEY: &str = "round_trip_build_link_on_load";

/// The URL prefix the in-game editor opens with, so a player only has to supply the id.
///
/// Not used for validation -- [`validate_build_url`] accepts any host, because the planner has
/// moved domain before and refusing an unfamiliar one would reject a link that works.
pub const BUILD_URL_PREFIX: &str = "https://er-build-planner.nyasu.business/?b=";

/// The System>Quit row's help line when nothing has gone wrong. The row's help is a live buffer the
/// link field rewrites to report a refusal, so this is what it is restored to.
pub const BUILD_URL_ROW_HELP: &str =
    "Paste or type an er-build-planner link to rebuild this character";

/// Why a URL cannot be imported. Each variant carries the one sentence a player sees when the
/// editor refuses their input, so the reason a thing was rejected lives with the rejection rather
/// than being reconstructed at the call site.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UrlRejection {
    /// Nothing was entered, or only the prefix the editor pre-filled.
    Empty,
    /// The self-contained `?i=` form. It carries the whole build in the URL and is never fetched,
    /// so it is called out separately: telling a player "no ?b=" about a link that plainly has a
    /// payload reads as a bug in us.
    SelfContained,
    /// No `b=` parameter at all.
    NoShareId,
    /// A `b=` that is present but not an id -- empty, or carrying something other than the
    /// letters and digits every share id is made of.
    MalformedShareId,
}

impl UrlRejection {
    /// The line the editor shows. Kept short: it is rendered on a menu row's help field, and the
    /// player is looking at their own text while reading it.
    pub fn indicator(self) -> &'static str {
        match self {
            UrlRejection::Empty => "Enter a build link - nothing was typed after the prefix",
            UrlRejection::SelfContained => {
                "That is a ?i= link; only shared ?b= links can be loaded"
            }
            UrlRejection::NoShareId => "That link has no ?b= build id",
            UrlRejection::MalformedShareId => "That build id is not valid",
        }
    }

    /// Stable telemetry code (`0` means "accepted").
    pub fn code(self) -> usize {
        match self {
            UrlRejection::Empty => 1,
            UrlRejection::SelfContained => 2,
            UrlRejection::NoShareId => 3,
            UrlRejection::MalformedShareId => 4,
        }
    }
}

/// Decide whether a typed URL can be imported, and say why not when it cannot.
///
/// This is the gate the in-game editor runs on Accept. It is deliberately the only place that
/// decision is made -- the editor re-opens rather than closing when this returns `Err`, so a
/// disagreement between this and [`share_id_from_url`] would let a link through that the fetch
/// then cannot use.
///
/// Whitespace is trimmed first: the field is pre-filled and typed into on a controller, and a
/// trailing space is a slip, not a different link.
///
/// ```
/// use er_build_import_core::{validate_build_url, UrlRejection};
/// assert_eq!(validate_build_url("https://p/?b=af97a9da874151"), Ok("af97a9da874151"));
/// assert_eq!(validate_build_url("https://p/?i=eyJ2IjoxfQ"), Err(UrlRejection::SelfContained));
/// assert_eq!(validate_build_url("https://p/?b="), Err(UrlRejection::MalformedShareId));
/// ```
pub fn validate_build_url(url: &str) -> Result<&str, UrlRejection> {
    let url = url.trim();
    if url.is_empty() || url == BUILD_URL_PREFIX || url.trim_end_matches('?').is_empty() {
        return Err(UrlRejection::Empty);
    }
    let Some((_, query)) = url.split_once('?') else {
        return Err(UrlRejection::NoShareId);
    };
    // An id typed straight into the prefix leaves `?b=<id>`; a bare `?b=` leaves nothing.
    if let Some(value) = query.split('&').find_map(|pair| pair.strip_prefix("b=")) {
        let id = value.split('#').next().unwrap_or(value);
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(UrlRejection::MalformedShareId);
        }
        return Ok(id);
    }
    if query.split('&').any(|pair| pair.starts_with("i=")) {
        return Err(UrlRejection::SelfContained);
    }
    Err(UrlRejection::NoShareId)
}

/// Extract the `?b=` share id from a planner URL.
///
/// Returns `None` for a URL that carries no `b` parameter, including the
/// self-contained `?i=` form, which needs no network at all.
pub fn share_id_from_url(url: &str) -> Option<&str> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("b="))
        .map(|id| id.split('#').next().unwrap_or(id))
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
}
