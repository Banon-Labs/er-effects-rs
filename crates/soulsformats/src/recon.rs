//! Parser for the output of the `FastSpEffectRecon.java` Ghidra script.
//!
//! The script scans a loaded Elden Ring binary for SpEffect-related symbols
//! and defined strings and emits a line-based `key=value` report (see
//! `docs/recon/README.md` for the full schema). This module turns that report
//! into structured data and extracts candidate SpEffect param IDs.

use std::collections::BTreeSet;

use thiserror::Error;

const PROGRAM_KEY: &str = "program=";
const EXECUTABLE_PATH_KEY: &str = "executablePath=";
const DOMAIN_FILE_KEY: &str = "domainFile=";
const SYMBOL_SECTION_HEADER: &str = "symbol-matches";
const STRING_SECTION_HEADER: &str = "defined-string-matches";
const SYMBOL_LINE_PREFIX: &str = "symbol name=\"";
const STRING_LINE_PREFIX: &str = "string address=";
const REFERENCE_LINE_PREFIX: &str = "  refFrom=";
const REFERENCE_TRUNCATED_VALUE: &str = "<truncated>";
const REFERENCE_TYPE_SEPARATOR: &str = " type=";
const SYMBOL_TYPE_SEPARATOR: &str = "\" type=";
const SYMBOL_ADDRESS_SEPARATOR: &str = " address=";
const SYMBOL_NAMESPACE_SEPARATOR: &str = " namespace=\"";
const STRING_VALUE_SEPARATOR: &str = " value=\"";
const TRAILING_QUOTE: &str = "\"";
const SYMBOL_TRUNCATED_LINE: &str = "symbol-matches-truncated=true";
const STRING_TRUNCATED_LINE: &str = "string-matches-truncated=true";
const DONE_LINE: &str = "done";
const ESCAPE_CHAR: char = '\\';
const LINE_NUMBER_OFFSET: usize = 1;
/// Digit-run bounds for SpEffect ID candidate extraction. SpEffect param IDs
/// in practice are 4..=9 digit decimal values (e.g. `4330`, `20018100`).
const MIN_CANDIDATE_ID_DIGITS: usize = 4;
const MAX_CANDIDATE_ID_DIGITS: usize = 9;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconReport {
    pub program: Option<String>,
    pub executable_path: Option<String>,
    pub domain_file: Option<String>,
    pub symbols: Vec<ReconSymbol>,
    pub strings: Vec<ReconString>,
    pub symbol_matches_truncated: bool,
    pub string_matches_truncated: bool,
    /// True when the report ends with the script's `done` marker, meaning the
    /// Ghidra script ran to completion rather than being cancelled mid-run.
    pub complete: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconSymbol {
    pub name: String,
    pub symbol_type: String,
    pub address: String,
    pub namespace: String,
    pub references: Vec<ReconReference>,
    pub references_truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconString {
    pub address: String,
    pub value: String,
    pub references: Vec<ReconReference>,
    pub references_truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconReference {
    pub from_address: String,
    pub reference_type: String,
}

#[derive(Debug, Error)]
pub enum ReconParseError {
    #[error("recon line {line}: {message}")]
    Malformed { line: usize, message: String },
}

impl ReconReport {
    /// Extracts distinct decimal digit runs that look like SpEffect param IDs
    /// from matched symbol names and string values, sorted ascending.
    pub fn speffect_id_candidates(&self) -> Vec<i32> {
        let mut candidates = BTreeSet::new();

        for symbol in &self.symbols {
            collect_id_candidates(&symbol.name, &mut candidates);
        }
        for string in &self.strings {
            collect_id_candidates(&string.value, &mut candidates);
        }

        candidates.into_iter().collect()
    }
}

enum Section {
    Preamble,
    Symbols,
    Strings,
}

pub fn parse_recon_report(input: &str) -> Result<ReconReport, ReconParseError> {
    let mut report = ReconReport::default();
    let mut section = Section::Preamble;

    for (index, line) in input.lines().enumerate() {
        let line_number = index + LINE_NUMBER_OFFSET;

        if line == SYMBOL_SECTION_HEADER {
            section = Section::Symbols;
            continue;
        }
        if line == STRING_SECTION_HEADER {
            section = Section::Strings;
            continue;
        }
        if line == DONE_LINE {
            report.complete = true;
            continue;
        }
        if line == SYMBOL_TRUNCATED_LINE {
            report.symbol_matches_truncated = true;
            continue;
        }
        if line == STRING_TRUNCATED_LINE {
            report.string_matches_truncated = true;
            continue;
        }

        if let Some(rest) = line.strip_prefix(REFERENCE_LINE_PREFIX) {
            attach_reference(&mut report, &section, rest, line_number)?;
            continue;
        }

        match section {
            Section::Preamble => parse_preamble_line(&mut report, line),
            Section::Symbols => {
                if line.starts_with(SYMBOL_LINE_PREFIX) {
                    report.symbols.push(parse_symbol_line(line, line_number)?);
                }
            }
            Section::Strings => {
                if line.starts_with(STRING_LINE_PREFIX) {
                    report.strings.push(parse_string_line(line, line_number)?);
                }
            }
        }
    }

    Ok(report)
}

fn parse_preamble_line(report: &mut ReconReport, line: &str) {
    if let Some(value) = line.strip_prefix(PROGRAM_KEY) {
        report.program = Some(value.to_owned());
    } else if let Some(value) = line.strip_prefix(EXECUTABLE_PATH_KEY) {
        report.executable_path = Some(value.to_owned());
    } else if let Some(value) = line.strip_prefix(DOMAIN_FILE_KEY) {
        report.domain_file = Some(value.to_owned());
    }
}

fn parse_symbol_line(line: &str, line_number: usize) -> Result<ReconSymbol, ReconParseError> {
    let rest = line
        .strip_prefix(SYMBOL_LINE_PREFIX)
        .ok_or_else(|| malformed(line_number, "expected symbol line"))?;

    // Symbol names are emitted unescaped, so split on the *last* occurrence
    // of each separator working from the end of the line inward.
    let (rest, namespace) = split_once_from_end(rest, SYMBOL_NAMESPACE_SEPARATOR)
        .ok_or_else(|| malformed(line_number, "symbol line missing namespace"))?;
    let namespace = namespace
        .strip_suffix(TRAILING_QUOTE)
        .ok_or_else(|| malformed(line_number, "symbol namespace missing closing quote"))?;
    let (rest, address) = split_once_from_end(rest, SYMBOL_ADDRESS_SEPARATOR)
        .ok_or_else(|| malformed(line_number, "symbol line missing address"))?;
    let (name, symbol_type) = split_once_from_end(rest, SYMBOL_TYPE_SEPARATOR)
        .ok_or_else(|| malformed(line_number, "symbol line missing type"))?;

    Ok(ReconSymbol {
        name: name.to_owned(),
        symbol_type: symbol_type.to_owned(),
        address: address.to_owned(),
        namespace: namespace.to_owned(),
        references: Vec::new(),
        references_truncated: false,
    })
}

fn parse_string_line(line: &str, line_number: usize) -> Result<ReconString, ReconParseError> {
    let rest = line
        .strip_prefix(STRING_LINE_PREFIX)
        .ok_or_else(|| malformed(line_number, "expected string line"))?;

    let (address, value) = rest
        .split_once(STRING_VALUE_SEPARATOR)
        .ok_or_else(|| malformed(line_number, "string line missing value"))?;
    let value = value
        .strip_suffix(TRAILING_QUOTE)
        .ok_or_else(|| malformed(line_number, "string value missing closing quote"))?;

    Ok(ReconString {
        address: address.to_owned(),
        value: unescape_recon_string(value),
        references: Vec::new(),
        references_truncated: false,
    })
}

fn attach_reference(
    report: &mut ReconReport,
    section: &Section,
    rest: &str,
    line_number: usize,
) -> Result<(), ReconParseError> {
    let truncated = rest == REFERENCE_TRUNCATED_VALUE;
    let reference = if truncated {
        None
    } else {
        let (from_address, reference_type) = rest
            .split_once(REFERENCE_TYPE_SEPARATOR)
            .ok_or_else(|| malformed(line_number, "reference line missing type"))?;
        Some(ReconReference {
            from_address: from_address.to_owned(),
            reference_type: reference_type.to_owned(),
        })
    };

    let (references, references_truncated) = match section {
        Section::Symbols => report
            .symbols
            .last_mut()
            .map(|symbol| (&mut symbol.references, &mut symbol.references_truncated))
            .ok_or_else(|| malformed(line_number, "reference before any symbol match"))?,
        Section::Strings => report
            .strings
            .last_mut()
            .map(|string| (&mut string.references, &mut string.references_truncated))
            .ok_or_else(|| malformed(line_number, "reference before any string match"))?,
        Section::Preamble => {
            return Err(malformed(line_number, "reference outside of a section"));
        }
    };

    match reference {
        Some(reference) => references.push(reference),
        None => *references_truncated = true,
    }

    Ok(())
}

/// Splits on the last occurrence of `separator`, mirroring `str::split_once`
/// but anchored at the end of the string.
fn split_once_from_end<'a>(input: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let index = input.rfind(separator)?;
    let after = &input[index + separator.len()..];
    Some((&input[..index], after))
}

/// Reverses the escaping applied by the Ghidra script's `sanitize()`:
/// `\\` -> `\`, `\"` -> `"`, `\n` -> newline, `\r` -> carriage return.
fn unescape_recon_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(current) = chars.next() {
        if current != ESCAPE_CHAR {
            output.push(current);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('"') => output.push('"'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => {
                output.push(ESCAPE_CHAR);
                output.push(other);
            }
            None => output.push(ESCAPE_CHAR),
        }
    }

    output
}

fn collect_id_candidates(text: &str, candidates: &mut BTreeSet<i32>) {
    let mut digits = String::new();

    for character in text.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }
        if (MIN_CANDIDATE_ID_DIGITS..=MAX_CANDIDATE_ID_DIGITS).contains(&digits.len())
            && let Ok(id) = digits.parse::<i32>()
        {
            candidates.insert(id);
        }
        digits.clear();
    }
}

fn malformed(line: usize, message: &str) -> ReconParseError {
    ReconParseError::Malformed {
        line,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_REPORT: &str = concat!(
        "program=eldenring.exe\n",
        "executablePath=C:\\Games\\ELDEN RING\\Game\\eldenring.exe\n",
        "domainFile=/eldenring.exe\n",
        "\n",
        "symbol-matches\n",
        "symbol name=\"ApplySpEffect\" type=Function address=14012aab0 namespace=\"ChrIns\"\n",
        "  refFrom=140a00010 type=UNCONDITIONAL_CALL\n",
        "  refFrom=140a00020 type=UNCONDITIONAL_CALL\n",
        "symbol name=\"sp_effect_20018100\" type=Label address=14300cafe namespace=\"Global\"\n",
        "  refFrom=<truncated>\n",
        "symbol-scan-cancelled=false\n",
        "symbol-scan-count=500000\n",
        "symbol-match-count=2\n",
        "\n",
        "defined-string-matches\n",
        "string address=143fff000 value=\"SpEffect id %d \\\"quoted\\\" line\\nbreak\"\n",
        "  refFrom=140b00030 type=DATA\n",
        "string-matches-truncated=true\n",
        "defined-string-scan-cancelled=false\n",
        "defined-data-scan-count=900000\n",
        "defined-string-match-count=1\n",
        "\n",
        "done\n",
    );
    const EXPECTED_SYMBOL_COUNT: usize = 2;
    const EXPECTED_STRING_COUNT: usize = 1;
    const EXPECTED_FIRST_SYMBOL_REFERENCES: usize = 2;
    const EXPECTED_CANDIDATE_ID: i32 = 20018100;
    const EXPECTED_CANDIDATE_COUNT: usize = 1;

    #[test]
    fn parses_sample_report() {
        let report = parse_recon_report(SAMPLE_REPORT).expect("sample report should parse");

        assert_eq!(report.program.as_deref(), Some("eldenring.exe"));
        assert!(report.complete);
        assert_eq!(report.symbols.len(), EXPECTED_SYMBOL_COUNT);
        assert_eq!(report.strings.len(), EXPECTED_STRING_COUNT);
        assert!(!report.symbol_matches_truncated);
        assert!(report.string_matches_truncated);

        let first = report.symbols.first().expect("first symbol");
        assert_eq!(first.name, "ApplySpEffect");
        assert_eq!(first.symbol_type, "Function");
        assert_eq!(first.address, "14012aab0");
        assert_eq!(first.namespace, "ChrIns");
        assert_eq!(first.references.len(), EXPECTED_FIRST_SYMBOL_REFERENCES);
        assert!(!first.references_truncated);

        let second = report.symbols.last().expect("second symbol");
        assert!(second.references_truncated);
        assert!(second.references.is_empty());
    }

    #[test]
    fn unescapes_string_values() {
        let report = parse_recon_report(SAMPLE_REPORT).expect("sample report should parse");
        let string = report.strings.first().expect("string match");

        assert_eq!(string.value, "SpEffect id %d \"quoted\" line\nbreak");
    }

    #[test]
    fn extracts_speffect_id_candidates() {
        let report = parse_recon_report(SAMPLE_REPORT).expect("sample report should parse");
        let candidates = report.speffect_id_candidates();

        assert_eq!(candidates.len(), EXPECTED_CANDIDATE_COUNT);
        assert_eq!(candidates, vec![EXPECTED_CANDIDATE_ID]);
    }

    #[test]
    fn rejects_reference_before_any_match() {
        const ORPHAN_REFERENCE: &str = "symbol-matches\n  refFrom=140a00010 type=DATA\n";

        let error = parse_recon_report(ORPHAN_REFERENCE).expect_err("orphan reference");
        assert!(matches!(error, ReconParseError::Malformed { .. }));
    }
}
