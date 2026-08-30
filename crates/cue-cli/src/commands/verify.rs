//! `cue verify` — validate a completed output against its run receipt.

use std::path::Path;

use crate::cli::VerifyArgs;
use crate::render::println_line;
use crate::verification::VerificationDiagnostic;

const VERIFY_REPORT_SCHEMA_VERSION: u32 = 2;

pub fn run(args: VerifyArgs, output_root: Option<&Path>) -> i32 {
    let output_label = args.output.display().to_string();
    let result = crate::commands::correct::resolve_output_dir_at(&args.output, output_root)
        .map_err(|error| OperationalError {
            id: "CUE-VERIFY-OUTPUT-INVALID",
            message: error.to_string().trim().to_owned(),
        })
        .and_then(|output_dir| {
            crate::verification::verify_output(&output_dir)
                .map(|verification| (output_dir, verification))
                .map_err(|error| OperationalError {
                    id: error.diagnostic_id(),
                    message: error.message(),
                })
        });

    if args.json {
        let report = match result {
            Ok((output_dir, verification)) => {
                VerifyReport::verified(output_dir.display().to_string(), verification)
            }
            Err(error) => VerifyReport::failure(output_label, error),
        };
        return match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println_line(&json);
                i32::from(!report.valid)
            }
            Err(error) => {
                eprintln!("could not serialize verification report: {error}");
                1
            }
        };
    }

    match result {
        Ok((output_dir, verification)) if verification.is_valid() => {
            println_line(&format!(
                "Verified {} artifact(s) in {}.",
                verification.artifact_count(),
                output_dir.display()
            ));
            0
        }
        Ok((_, verification)) => {
            for diagnostic in &verification.diagnostics {
                println_line(&format!("{}: {}", diagnostic.id, diagnostic.message));
            }
            println_line(&format!(
                "\n{} verification issue(s) found.",
                verification.diagnostics.len()
            ));
            1
        }
        Err(error) => {
            eprintln!("{}", error.message);
            1
        }
    }
}

struct OperationalError {
    id: &'static str,
    message: String,
}

#[derive(serde::Serialize)]
struct VerifyReport {
    schema_version: u32,
    output: String,
    valid: bool,
    artifact_count: usize,
    diagnostics: Vec<VerificationDiagnostic>,
}

impl VerifyReport {
    fn verified(output: String, verification: crate::verification::VerificationResult) -> Self {
        Self {
            schema_version: VERIFY_REPORT_SCHEMA_VERSION,
            output,
            valid: verification.is_valid(),
            artifact_count: verification.artifact_count(),
            diagnostics: verification.diagnostics,
        }
    }

    fn failure(output: String, error: OperationalError) -> Self {
        Self {
            schema_version: VERIFY_REPORT_SCHEMA_VERSION,
            output,
            valid: false,
            artifact_count: 0,
            diagnostics: vec![VerificationDiagnostic::operational(error.id, error.message)],
        }
    }
}
