//! Runs the binary as documented and compares stdout against a committed file.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Runs the binary with `arguments` and returns the finished process.
fn run(arguments: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_payments-engine"))
        .args(arguments)
        .output()
        .expect("the binary under test should start")
}

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

/// Asserts that `<case>.csv` produces `<case>.expected.csv` on stdout.
#[track_caller]
fn assert_case(case: &str) {
    let output = run(&[&data(&format!("{case}.csv"))]);
    let expected = std::fs::read_to_string(data(&format!("{case}.expected.csv")))
        .expect("expected output should be readable");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Compared exactly: the fixtures are committed with `eol=lf`, and the
    // engine writes `\n`, so a stray carriage return would be a regression.
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected, "{case}");
}

#[test]
fn processes_the_documented_example() {
    assert_case("basic");
}

#[test]
fn processes_disputes_resolutions_and_chargebacks() {
    assert_case("disputes");
}

#[test]
fn processes_an_input_full_of_partner_errors() {
    assert_case("messy");
}

#[test]
fn processes_a_file_with_no_transactions() {
    assert_case("empty");
}

#[test]
fn processes_a_generated_file_of_partner_errors() {
    // Chargebacks, frozen accounts, duplicate ids, cross-client disputes and
    // unparseable amounts. The expected output came from a second
    // implementation, not from this engine.
    assert_case("adversarial");
}

#[test]
fn ignored_rows_are_summarised_on_stderr() {
    let output = run(&[&data("messy.csv")]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("ignored 7 row(s):"), "{stderr}");
    for reason in [
        "unknown transaction type",
        "missing or invalid amount",
        "duplicate transaction id",
        "insufficient funds",
        "transaction belongs to another client",
        "transaction is not under dispute",
    ] {
        assert!(stderr.contains(reason), "{reason} missing from:\n{stderr}");
    }
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("client,available"));
}

#[test]
fn a_clean_run_says_nothing_on_stderr() {
    let output = run(&[&data("empty.csv")]);
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn the_example_reports_its_failed_withdrawal() {
    // The example ends with a withdrawal client 2 cannot afford.
    let output = run(&[&data("basic.csv")]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stderr.contains("ignored 1 row(s):"), "{stderr}");
    assert!(stderr.contains("insufficient funds"), "{stderr}");
}

#[test]
fn a_missing_input_file_fails_loudly() {
    let output = run(&[&data("does-not-exist.csv")]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "no partial output on failure");
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("error: "),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn no_argument_is_a_usage_error() {
    let output = run(&[]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
}

#[test]
fn extra_arguments_are_ignored_with_a_warning() {
    let output = run(&[&data("basic.csv"), &data("disputes.csv")]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("extra arguments ignored"));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        std::fs::read_to_string(data("basic.expected.csv")).unwrap()
    );
}
