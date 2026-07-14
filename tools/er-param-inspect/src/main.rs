use std::{env, fs, path::PathBuf, process::ExitCode};

use er_effects_data::embedded_effects;
use er_soulsformats::{ParamRowsResponse, SoulsFormats, recon::parse_recon_report};

const PROGRAM_NAME: &str = "er-param-inspect";
const MODE_ARG_INDEX: usize = 1;
const ROWS_MODE: &str = "rows";
const VALIDATE_MODE: &str = "validate";
const RECON_MODE: &str = "recon";
const ROWS_REGULATION_ARG_INDEX: usize = 2;
const ROWS_PARAM_NAME_ARG_INDEX: usize = 3;
const ROWS_ROW_ID_ARGS_START_INDEX: usize = 4;
const ROWS_MINIMUM_ARGUMENT_COUNT: usize = 5;
const VALIDATE_REGULATION_ARG_INDEX: usize = 2;
const VALIDATE_ARGUMENT_COUNT: usize = 3;
const RECON_REPORT_ARG_INDEX: usize = 2;
const RECON_REGULATION_ARG_INDEX: usize = 3;
const RECON_MINIMUM_ARGUMENT_COUNT: usize = 3;
const SP_EFFECT_PARAM_NAME: &str = "SpEffectParam";

const USAGE: &str = "\
usage:
  er-param-inspect rows <regulation.bin> <param-name> <row-id> [row-id...]
      Print name/found status for the given param rows.
  er-param-inspect validate <regulation.bin>
      Check every entry of data/effects.json against SpEffectParam.
      Exits non-zero if any built-in catalog ID has no param row.
  er-param-inspect recon <recon-report.txt> [regulation.bin]
      Summarize FastSpEffectRecon output and extract candidate SpEffect IDs.
      With a regulation path, also checks candidates against SpEffectParam.";

fn main() -> ExitCode {
    let args = env::args().collect::<Vec<_>>();

    let result = match args.get(MODE_ARG_INDEX).map(String::as_str) {
        Some(ROWS_MODE) => run_rows(&args),
        Some(VALIDATE_MODE) => run_validate(&args),
        Some(RECON_MODE) => run_recon(&args),
        _ => Err(USAGE.to_owned()),
    };

    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_rows(args: &[String]) -> Result<ExitCode, String> {
    if args.len() < ROWS_MINIMUM_ARGUMENT_COUNT {
        return Err(format!(
            "usage: {PROGRAM_NAME} rows <regulation.bin> <param-name> <row-id> [row-id...]"
        ));
    }

    let regulation_path = PathBuf::from(&args[ROWS_REGULATION_ARG_INDEX]);
    let param_name = &args[ROWS_PARAM_NAME_ARG_INDEX];
    let row_ids = parse_row_ids(&args[ROWS_ROW_ID_ARGS_START_INDEX..])?;

    let response = query_rows(&regulation_path, param_name, &row_ids)?;
    print_rows(&response);

    Ok(ExitCode::SUCCESS)
}

fn run_validate(args: &[String]) -> Result<ExitCode, String> {
    if args.len() != VALIDATE_ARGUMENT_COUNT {
        return Err(format!("usage: {PROGRAM_NAME} validate <regulation.bin>"));
    }

    let regulation_path = PathBuf::from(&args[VALIDATE_REGULATION_ARG_INDEX]);
    let effects = embedded_effects()
        .map_err(|error| format!("embedded data/effects.json is invalid: {error}"))?;
    let ids = effects.calls.iter().map(|call| call.id).collect::<Vec<_>>();

    let response = query_rows(&regulation_path, SP_EFFECT_PARAM_NAME, &ids)?;

    let mut missing = Vec::new();
    for call in &effects.calls {
        let row = response.rows.iter().find(|row| row.id == call.id);
        let found = row.is_some_and(|row| row.found);
        let row_name = row.map(|row| row.name.as_str()).unwrap_or_default();
        let status = if found { "ok" } else { "MISSING" };
        println!("{status}\t{}\t{}\t{row_name}", call.id, call.name);
        if !found {
            missing.push(call.id);
        }
    }

    if missing.is_empty() {
        println!(
            "all {} built-in SpEffect IDs found in {SP_EFFECT_PARAM_NAME}",
            effects.calls.len()
        );
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("missing {SP_EFFECT_PARAM_NAME} rows for IDs: {missing:?}");
        Ok(ExitCode::FAILURE)
    }
}

fn run_recon(args: &[String]) -> Result<ExitCode, String> {
    if args.len() < RECON_MINIMUM_ARGUMENT_COUNT {
        return Err(format!(
            "usage: {PROGRAM_NAME} recon <recon-report.txt> [regulation.bin]"
        ));
    }

    let report_path = PathBuf::from(&args[RECON_REPORT_ARG_INDEX]);
    let report_text = fs::read_to_string(&report_path)
        .map_err(|error| format!("failed to read {}: {error}", report_path.display()))?;
    let report = parse_recon_report(&report_text).map_err(|error| error.to_string())?;

    println!(
        "program={} complete={} symbols={} strings={} symbols_truncated={} strings_truncated={}",
        report.program.as_deref().unwrap_or("<unknown>"),
        report.complete,
        report.symbols.len(),
        report.strings.len(),
        report.symbol_matches_truncated,
        report.string_matches_truncated,
    );

    let candidates = report.speffect_id_candidates();
    println!("speffect_id_candidates={}", candidates.len());
    for candidate in &candidates {
        println!("candidate\t{candidate}");
    }

    let Some(regulation_arg) = args.get(RECON_REGULATION_ARG_INDEX) else {
        return Ok(ExitCode::SUCCESS);
    };

    let regulation_path = PathBuf::from(regulation_arg);
    let response = query_rows(&regulation_path, SP_EFFECT_PARAM_NAME, &candidates)?;
    print_rows(&response);

    Ok(ExitCode::SUCCESS)
}

fn query_rows(
    regulation_path: &PathBuf,
    param_name: &str,
    row_ids: &[i32],
) -> Result<ParamRowsResponse, String> {
    let soulsformats = SoulsFormats::from_env_or_default().map_err(|error| error.to_string())?;
    soulsformats
        .query_param_rows(regulation_path, param_name, row_ids)
        .map_err(|error| error.to_string())
}

fn print_rows(response: &ParamRowsResponse) {
    println!(
        "binder_version={} param={} rows={}",
        response.binder_version, response.param_name, response.row_count
    );

    for row in &response.rows {
        let name = if row.name.is_empty() {
            "<empty>"
        } else {
            &row.name
        };
        println!(
            "{}\toccurrence={}\tfound={}\t{}",
            row.id, row.occurrence_index, row.found, name
        );
    }
}

fn parse_row_ids(values: &[String]) -> Result<Vec<i32>, String> {
    values
        .iter()
        .map(|value| {
            value
                .parse::<i32>()
                .map_err(|error| format!("invalid row id {value:?}: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_PARSED_ID: i32 = 4330;
    const SECOND_PARSED_ID: i32 = -1;

    #[test]
    fn parses_valid_row_ids() {
        let values = vec!["4330".to_owned(), "-1".to_owned()];

        assert_eq!(
            parse_row_ids(&values).expect("valid ids"),
            vec![FIRST_PARSED_ID, SECOND_PARSED_ID]
        );
    }

    #[test]
    fn rejects_non_numeric_row_ids() {
        let values = vec!["not-a-number".to_owned()];

        assert!(parse_row_ids(&values).is_err());
    }

    #[test]
    fn rejects_out_of_range_row_ids() {
        let values = vec!["99999999999999".to_owned()];

        assert!(parse_row_ids(&values).is_err());
    }
}
