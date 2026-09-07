//! Produce an `er-build-planner` `?i=` share link, entirely offline.
//!
//! This is the write half of what `er-build-import-core` reads, and it is a different problem.
//! A `?b=` link is an id the planner resolves server-side, so writing one would mean holding
//! an account and posting a build. A `?i=` link is *self-contained*: the whole character
//! document is compressed into the URL, so it can be produced from nothing but the document,
//! by anyone, with no network at all. That is the link this crate emits.
//!
//! # The pipeline
//!
//! Transcribed from the live site bundle -- `serialise() { return zc(btoa(JSON.stringify(
//! this.character))) }` and the `zc` it calls -- and verified against the site's own decoder.
//! In order:
//!
//! 1. [`ascii_json`] -- `JSON.stringify(character)`, with every non-ASCII character escaped
//!    as `\uXXXX`. See [`json_ascii`] for why this crate escapes where the site cannot.
//! 2. [`base64::encode`] -- `btoa` of that ASCII text.
//! 3. [`substitute_json_tokens`] -- the site's three JSON-shorthand substitutions, which are
//!    provably no-ops on base64 and are applied anyway; see that function.
//! 4. [`lzutf8::compress`] then [`base64::encode`] again -- `LZUTF8.compress(text, {
//!    outputEncoding: "Base64" })`, i.e. the compressed *bytes*, base64'd.
//! 5. [`to_url_alphabet`] -- the URL-safe substitution and two legacy prefix rewrites.
//! 6. [`SHARE_URL_PREFIX`] in front.
//!
//! ```
//! use er_build_export::{model::BuildExportDoc, share_url, SHARE_URL_PREFIX};
//!
//! let mut doc = BuildExportDoc::with_level(150, false);
//! doc.name = "Occult Mage".to_string();
//! doc.stats.intelligence = 36;
//!
//! let url = share_url(&doc);
//! assert!(url.starts_with(SHARE_URL_PREFIX));
//! ```
//!
//! # Testability
//!
//! Every stage above is a public function taking and returning plain data, so each can be
//! pinned on its own. On top of that sit three end-to-end checks, in descending order of
//! authority and ascending order of availability:
//!
//! * `tests/reference_decoder.rs` -- the site's **actual** decoder, run under Node over the
//!   finished payload, asserting the recovered JSON is byte-identical. The real gate. Skips
//!   when Node or the extracted bundle is absent.
//! * `tests/python_decoder.rs` -- `scripts/decode-build-link.py`, an independently written
//!   decoder that lives in this repository. Compares parsed documents rather than bytes.
//!   Skips when Python or the script is absent.
//! * `tests/pipeline.rs` and `tests/lzutf8_roundtrip.rs` -- pure Rust, against a decoder
//!   transcribed from the reference implementation. Never skip, so `cargo test` is still
//!   meaningful on a machine with no interpreter at all.

pub mod base64;
pub mod json_ascii;
pub mod lzutf8;
pub mod model;

pub use model::BuildExportDoc;

/// Everything before the payload in a self-contained share link.
///
/// `?i=` and not `?b=`: the planner treats them as different link kinds, and
/// `er_build_import_core::UrlRejection::SelfContained` is the importer refusing this one because
/// it has no id to fetch. A link produced here is read by the planner, not by us.
pub const SHARE_URL_PREFIX: &str = "https://er-build-planner.nyasu.business/?i=";

/// Build the full share link for `doc`.
///
/// # Panics
///
/// Never, in practice. [`model::BuildExportDoc`] is plain owned data whose only untyped
/// corners are `serde_json::Value`s, and serialising it can fail on exactly two things:
/// a map key that is not a string, and a non-finite float. It has no float fields, and every
/// map it contains is keyed by [`String`]. The alternative -- a `Result` no caller could
/// meaningfully handle -- would push that unreachable case onto every call site instead.
pub fn share_url(doc: &BuildExportDoc) -> String {
    format!("{SHARE_URL_PREFIX}{}", share_payload(doc))
}

/// The `?i=` value alone, without the URL around it.
///
/// # Panics
///
/// See [`share_url`].
pub fn share_payload(doc: &BuildExportDoc) -> String {
    encode_payload(&ascii_json(doc))
}

/// Stage 1: the character document as pure-ASCII JSON.
///
/// # Panics
///
/// See [`share_url`].
pub fn ascii_json(doc: &BuildExportDoc) -> String {
    json_ascii::to_ascii_json(doc).expect("a BuildExportDoc has no unserialisable field")
}

/// Stages 2 to 5: character JSON in, `?i=` payload out.
///
/// Split from [`share_payload`] so the encoder can be pinned against a fixed JSON string
/// without a document in the way -- and so a caller holding JSON from elsewhere can use it.
pub fn encode_payload(character_json: &str) -> String {
    planner_encode(&base64::encode(character_json.as_bytes()))
}

/// Stages 3 to 5: the site's `zc`, applied to an already-base64'd document.
///
/// Kept as its own function because it is the piece with an exact counterpart on the site;
/// anything that drifts from `zc` drifts here, and this is where it is checked.
pub fn planner_encode(text: &str) -> String {
    // This substitution is a no-OP here, and it is still not optional.
    //
    // `?i=` always reaches this function as base64, which has no braces or brackets, so none of
    // the three sequences can occur and the scan changes nothing. But that is a property of the
    // input, not of the transform -- and `planner_encode` is public, so the input is not this
    // crate's to guarantee. Eliding the stage would produce a function that happens to work for
    // one caller instead of one that is the site's `zc`, and the first caller to hand it raw JSON
    // would get a payload the planner cannot decode, silently and only at the far end.
    //
    // See [`substitute_json_tokens`] for the three pairs and for why the reverse direction is
    // unpaired.
    let prepared = substitute_json_tokens(text.trim());
    let compressed = lzutf8::compress(&prepared);
    to_url_alphabet(base64::encode(&compressed).trim())
}

/// `},{` in a JSON document, which `zc` replaces with a single byte.
pub const JSON_TOKEN_OBJECT_SEPARATOR: &str = "},{";
/// The byte `zc` substitutes for [`JSON_TOKEN_OBJECT_SEPARATOR`].
pub const JSON_TOKEN_OBJECT_SEPARATOR_BYTE: &str = "\u{1}";
/// `[{` in a JSON document, which `zc` replaces with a single byte.
pub const JSON_TOKEN_ARRAY_OPEN: &str = "[{";
/// The byte `zc` substitutes for [`JSON_TOKEN_ARRAY_OPEN`].
pub const JSON_TOKEN_ARRAY_OPEN_BYTE: &str = "\u{5}";
/// `}]` in a JSON document, which `zc` replaces with a single byte.
pub const JSON_TOKEN_ARRAY_CLOSE: &str = "}]";
/// The byte `zc` substitutes for [`JSON_TOKEN_ARRAY_CLOSE`].
pub const JSON_TOKEN_ARRAY_CLOSE_BYTE: &str = "\u{6}";

/// The site's pre-compression shorthand: three common JSON sequences collapsed to one control
/// byte each, undone by its decoder before `JSON.parse`.
///
/// **This is a no-op on the `?i=` path, and it is here so that stays true by inspection
/// rather than by luck.** The input to `zc` is base64, whose alphabet is `A-Za-z0-9+/=` --
/// no braces, no brackets -- so none of the three sequences can occur. But `zc` really does
/// apply them, its decoder really does undo them, and an agent reading only the pipeline
/// summary would not know the stage existed. Reproducing it costs three string scans and
/// removes a class of "why does our payload differ from the site's" question.
///
/// The unpaired direction matters too: the decoder's substitution runs on *every* payload, so
/// a stream that happened to contain `\u{1}` would come back as `},{`. Ours cannot, because
/// LZ-UTF8's output is base64'd before it is ever a string.
///
/// ```
/// use er_build_export::substitute_json_tokens;
///
/// // Base64 is untouched.
/// let base64 = "eyJuYW1lIjoiT2NjdWx0IE1hZ2UifQ==";
/// assert_eq!(substitute_json_tokens(base64), base64);
///
/// // Raw JSON is not.
/// assert_eq!(substitute_json_tokens(r#"[{"a":1},{"b":2}]"#), "\u{5}\"a\":1\u{1}\"b\":2\u{6}");
/// ```
pub fn substitute_json_tokens(text: &str) -> String {
    text.replace(
        JSON_TOKEN_OBJECT_SEPARATOR,
        JSON_TOKEN_OBJECT_SEPARATOR_BYTE,
    )
    .replace(JSON_TOKEN_ARRAY_OPEN, JSON_TOKEN_ARRAY_OPEN_BYTE)
    .replace(JSON_TOKEN_ARRAY_CLOSE, JSON_TOKEN_ARRAY_CLOSE_BYTE)
}

/// Base64 prefix the planner rewrites to [`LEGACY_LOWER_MARKER`].
pub const LEGACY_LOWER_PREFIX: &str = "eyI";
/// What [`LEGACY_LOWER_PREFIX`] becomes.
pub const LEGACY_LOWER_MARKER: &str = "uwu";
/// Base64 prefix the planner rewrites to [`LEGACY_UPPER_MARKER`].
pub const LEGACY_UPPER_PREFIX: &str = "eyJ";
/// What [`LEGACY_UPPER_PREFIX`] becomes.
pub const LEGACY_UPPER_MARKER: &str = "UWU";

/// Stage 5: make the base64 URL-safe, and apply the planner's two legacy prefix rewrites.
///
/// `+` and `/` are the two standard-alphabet symbols a query string cannot carry literally,
/// so they become `-` and `_`; the planner's decoder reverses that before decoding.
///
/// The two prefix rewrites are dead weight *for this format* and are implemented regardless.
/// `eyI` and `eyJ` are base64 for `{"` -- they mark a payload that was **not** compressed,
/// just base64'd JSON, which is what an older sharing format emitted. A compressed payload
/// begins with base64 of an LZ-UTF8 stream, in practice `ZXlK` (the compressed form still
/// opens with the literal bytes `eyJ`), so neither branch fires. They are still here because
/// this function's contract is "whatever `zc` does", and a reader who found `uwu` in a real
/// link and no `uwu` in this code would have to go and rediscover why.
///
/// ```
/// use er_build_export::to_url_alphabet;
/// assert_eq!(to_url_alphabet("a+b/c="), "a-b_c=");
/// assert_eq!(to_url_alphabet("eyJhIjoxfQ=="), "UWUhIjoxfQ==");
/// ```
pub fn to_url_alphabet(base64: &str) -> String {
    let marked = match base64 {
        _ if base64.starts_with(LEGACY_LOWER_PREFIX) => {
            format!(
                "{LEGACY_LOWER_MARKER}{}",
                &base64[LEGACY_LOWER_PREFIX.len()..]
            )
        }
        _ if base64.starts_with(LEGACY_UPPER_PREFIX) => {
            format!(
                "{LEGACY_UPPER_MARKER}{}",
                &base64[LEGACY_UPPER_PREFIX.len()..]
            )
        }
        _ => base64.to_string(),
    };
    marked.replace('+', "-").replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_alphabet_replaces_both_unsafe_symbols() {
        assert_eq!(to_url_alphabet("ab+cd/ef+gh/"), "ab-cd_ef-gh_");
    }

    #[test]
    fn url_alphabet_keeps_padding() {
        assert_eq!(to_url_alphabet("Zg=="), "Zg==");
    }

    #[test]
    fn legacy_prefixes_are_rewritten_at_the_start_only() {
        assert_eq!(to_url_alphabet("eyIabc"), "uwuabc");
        assert_eq!(to_url_alphabet("eyJabc"), "UWUabc");
        // Mid-string occurrences are ordinary base64 and must survive.
        assert_eq!(to_url_alphabet("AeyJabc"), "AeyJabc");
    }

    #[test]
    fn compressed_payloads_never_trip_the_legacy_prefixes() {
        // The point of the rewrites is that they do not fire on this format.
        let doc = BuildExportDoc::default();
        let payload = share_payload(&doc);
        assert!(!payload.starts_with(LEGACY_LOWER_MARKER), "{payload}");
        assert!(!payload.starts_with(LEGACY_UPPER_MARKER), "{payload}");
    }

    #[test]
    fn json_token_substitution_is_a_no_op_on_every_base64_symbol() {
        let alphabet: String = ('A'..='Z')
            .chain('a'..='z')
            .chain('0'..='9')
            .chain(['+', '/', '='])
            .collect();
        assert_eq!(substitute_json_tokens(&alphabet), alphabet);
    }

    #[test]
    fn json_token_substitution_matches_the_site_on_raw_json() {
        assert_eq!(
            substitute_json_tokens("[{\"a\":1},{\"b\":2}]"),
            "\u{5}\"a\":1\u{1}\"b\":2\u{6}"
        );
    }

    #[test]
    fn share_url_is_the_prefix_plus_the_payload() {
        let doc = BuildExportDoc::default();
        assert_eq!(
            share_url(&doc),
            format!("{SHARE_URL_PREFIX}{}", share_payload(&doc))
        );
    }

    #[test]
    fn payload_is_url_safe() {
        let mut doc = BuildExportDoc::with_level(150, false);
        doc.name = "Dongerino \u{e9}\u{30c6}\u{30b9}\u{30c8}".to_string();
        doc.description = "a".repeat(500);
        let payload = share_payload(&doc);
        assert!(
            payload
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=')),
            "{payload}"
        );
    }

    #[test]
    fn document_json_is_ascii_even_with_a_unicode_name() {
        let doc = BuildExportDoc {
            name: "Dongerino \u{e9}\u{30c6}\u{30b9}\u{30c8} \u{1f525}".to_string(),
            ..BuildExportDoc::default()
        };
        assert!(ascii_json(&doc).is_ascii());
    }

    #[test]
    fn encoding_is_deterministic() {
        let mut doc = BuildExportDoc::with_level(150, false);
        doc.name = "Occult Mage".to_string();
        assert_eq!(share_payload(&doc), share_payload(&doc));
    }
}
