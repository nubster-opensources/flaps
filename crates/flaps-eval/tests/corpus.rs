//! Golden corpus conformance test for the `flaps-eval` engine.
//!
//! Discovers every `*.json` case under `corpus/`, evaluates each one
//! through the public `FlagSet` API, and accumulates all failures before
//! reporting them in a single panic. A missing or empty corpus directory
//! is itself a failure (an empty corpus must never silently pass green).
//!
//! ## Oracle guarantee
//!
//! The `expected` field in every case is derived from an external oracle
//! (`MurmurHash3` public vectors, the flagd specification, or the JsonLogic
//! specification). It is never produced by running `flaps-eval` and copying
//! its output, which would make the test a tautology.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flaps_eval::{EvaluationContext, EvaluationError, FlagSet, Reason};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A single golden test case deserialized from a corpus `*.json` file.
#[derive(Debug, Deserialize)]
struct GoldenCase {
    name: String,
    /// Human-readable rationale; carried for failure messages but not
    /// compared against any oracle value.
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
    flagd: serde_json::Value,
    #[serde(rename = "flagKey")]
    flag_key: String,
    #[serde(default)]
    context: ContextCase,
    #[serde(flatten)]
    outcome: Outcome,
}

/// The evaluation context section of a corpus case.
#[derive(Debug, Default, Deserialize)]
struct ContextCase {
    #[serde(rename = "targetingKey")]
    targeting_key: Option<String>,
    #[serde(default)]
    attributes: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    timestamp: u64,
}

/// The expected outcome: either a successful resolution or an error code.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Outcome {
    /// The evaluation must succeed and match all provided fields.
    Ok { expected: ExpectedResolution },
    /// The evaluation (or parsing) must fail with the given error code.
    Err {
        #[serde(rename = "expectedError")]
        expected_error: String,
    },
}

/// Expected fields of a successful evaluation.
#[derive(Debug, Deserialize)]
struct ExpectedResolution {
    variant: Option<String>,
    reason: String,
    value: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Recursively collects all `*.json` files under `root`, sorted by path.
///
/// Files named `README.md` or any other non-`.json` extension are ignored.
/// The sort guarantees reproducible failure reports across platforms.
fn discover_cases(root: &Path) -> Vec<(PathBuf, GoldenCase)> {
    let mut files: Vec<PathBuf> = collect_json_files(root);
    files.sort();

    let mut cases = Vec::with_capacity(files.len());
    for path in files {
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read corpus file {}: {e}", path.display()));
        match serde_json::from_str::<GoldenCase>(&raw) {
            Ok(case) => cases.push((path, case)),
            Err(e) => panic!(
                "corpus file {} is not valid JSON or does not match the GoldenCase schema: {e}",
                path.display()
            ),
        }
    }
    cases
}

/// Walks `dir` recursively and collects every `*.json` file path.
fn collect_json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let read = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read corpus directory {}: {e}", dir.display()));
    for entry in read {
        let entry = entry.unwrap_or_else(|e| panic!("corpus directory entry error: {e}"));
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_json_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Maps a [`Reason`] to its OFREP string representation.
fn reason_to_ofrep(reason: Reason) -> &'static str {
    match reason {
        Reason::Static => "STATIC",
        Reason::TargetingMatch => "TARGETING_MATCH",
        Reason::Default => "DEFAULT",
        Reason::Disabled => "DISABLED",
    }
}

/// Maps an [`EvaluationError`] to its OFREP error code string.
fn error_to_code(err: &EvaluationError) -> &'static str {
    match err {
        EvaluationError::FlagNotFound { .. } => "FLAG_NOT_FOUND",
        EvaluationError::InvalidVariant { .. } | EvaluationError::UnsupportedOperation { .. } => {
            "VARIANT_NOT_FOUND"
        }
    }
}

/// Runs a single case and returns `Err(message)` when the actual output
/// diverges from the oracle.
fn run_case(case: &GoldenCase) -> Result<(), String> {
    let document = serde_json::to_string(&case.flagd)
        .unwrap_or_else(|e| panic!("cannot re-serialize flagd field of case {}: {e}", case.name));

    let ctx = EvaluationContext {
        targeting_key: case.context.targeting_key.clone(),
        attributes: case.context.attributes.clone(),
        timestamp: case.context.timestamp,
    };

    match &case.outcome {
        Outcome::Err { expected_error } => {
            // The error may come from parsing or from evaluation.
            match FlagSet::from_json(&document) {
                Err(_) => {
                    // Parse error: check code.
                    if expected_error == "PARSE_ERROR" {
                        Ok(())
                    } else {
                        Err(format!(
                            "expected error {expected_error} but got PARSE_ERROR",
                        ))
                    }
                }
                Ok(flag_set) => match flag_set.evaluate(&case.flag_key, &ctx) {
                    Err(eval_err) => {
                        let code = error_to_code(&eval_err);
                        if code == expected_error {
                            Ok(())
                        } else {
                            Err(format!(
                                "expected error {expected_error} but got {code} ({eval_err})",
                            ))
                        }
                    }
                    Ok(resolution) => Err(format!(
                        "expected error {expected_error} but evaluation succeeded with variant={:?} reason={}",
                        resolution.variant,
                        reason_to_ofrep(resolution.reason),
                    )),
                },
            }
        }
        Outcome::Ok { expected } => {
            let flag_set = FlagSet::from_json(&document).map_err(|e| {
                format!("expected success but flagd document failed to parse: {e}",)
            })?;

            let resolution = flag_set.evaluate(&case.flag_key, &ctx).map_err(|e| {
                format!(
                    "expected success (variant={:?} reason={}) but evaluation failed: {e}",
                    expected.variant, expected.reason,
                )
            })?;

            let mut failures: Vec<String> = Vec::new();

            // Check reason.
            let actual_reason = reason_to_ofrep(resolution.reason);
            if actual_reason != expected.reason {
                failures.push(format!(
                    "reason: expected={} actual={actual_reason}",
                    expected.reason,
                ));
            }

            // Check variant (None in JSON becomes JSON null).
            let actual_variant = resolution
                .variant
                .as_deref()
                .map_or(serde_json::Value::Null, |v| {
                    serde_json::Value::String(v.to_owned())
                });
            let expected_variant = expected
                .variant
                .as_deref()
                .map_or(serde_json::Value::Null, |v| {
                    serde_json::Value::String(v.to_owned())
                });
            if actual_variant != expected_variant {
                failures.push(format!(
                    "variant: expected={expected_variant} actual={actual_variant}",
                ));
            }

            // Check value (None in Rust -> null in JSON).
            let actual_value = resolution.value.unwrap_or(serde_json::Value::Null);
            if actual_value != expected.value {
                failures.push(format!(
                    "value: expected={} actual={actual_value}",
                    expected.value,
                ));
            }

            if failures.is_empty() {
                Ok(())
            } else {
                Err(failures.join("; "))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test entry point
// ---------------------------------------------------------------------------

#[test]
fn golden_corpus_conformance() {
    let corpus_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");

    let cases = discover_cases(&corpus_root);

    assert!(
        !cases.is_empty(),
        "corpus directory is empty or missing -- a corpus with zero cases must not pass silently"
    );

    eprintln!("running {} golden corpus cases", cases.len());

    let mut failures: Vec<String> = Vec::new();

    for (path, case) in &cases {
        let rel = path
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap_or(path);
        match run_case(case) {
            Ok(()) => {}
            Err(msg) => {
                failures.push(format!(
                    "[FAIL] {} (\"{}\"): {msg}",
                    rel.display(),
                    case.name,
                ));
            }
        }
    }

    if failures.is_empty() {
        eprintln!("all {} cases passed", cases.len());
    } else {
        let report = failures.join("\n");
        panic!(
            "{} of {} golden corpus cases failed:\n{report}",
            failures.len(),
            cases.len(),
        );
    }
}
