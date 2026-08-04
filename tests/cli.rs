use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog.json")
}

/// Every test here runs offline, so the mode is pinned rather than left to the
/// `live` default. `TERMUTO_MODE` is cleared so an exported value cannot change
/// what is being asserted.
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_termuto"));
    command.env_remove("TERMUTO_MODE").env_remove("TERMUTO_CATALOG");
    command
}

fn cached() -> Command {
    let mut command = command();
    command.args([
        "--mode",
        "cached",
        "--catalog",
        catalog_path().to_str().expect("utf-8 path"),
    ]);
    command
}

#[test]
fn latest_prints_sorted_catalog() {
    cached()
        .arg("latest")
        .assert()
        .success()
        .stdout(predicate::str::contains("Latest releases"))
        .stdout(predicate::str::contains("Solo Leveling Season 2"));
}

#[test]
fn search_prints_case_insensitive_matches() {
    cached()
        .args(["search", "SOLO"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 results for \"SOLO\""))
        .stdout(predicate::str::contains("Solo Leveling Season 2"));
}

#[test]
fn no_result_search_succeeds() {
    cached()
        .args(["search", "not a title"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "No anime found for \"not a title\".",
        ));
}

#[test]
fn the_mode_environment_variable_selects_the_cached_source() {
    command()
        .env("TERMUTO_MODE", "cached")
        .env("TERMUTO_CATALOG", catalog_path())
        .arg("latest")
        .assert()
        .success()
        .stdout(predicate::str::contains("Solo Leveling Season 2"));
}

#[test]
fn an_unknown_mode_is_rejected() {
    command()
        .args(["--mode", "offline", "latest"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("offline"));

    command()
        .env("TERMUTO_MODE", "offline")
        .arg("latest")
        .assert()
        .failure()
        .stderr(predicate::str::contains("TERMUTO_MODE is invalid"));
}

#[test]
fn api_only_listings_explain_themselves_in_cached_mode() {
    cached()
        .arg("top")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--mode live"));
}

#[test]
fn season_arguments_must_be_given_together() {
    cached()
        .args(["season", "--year", "2023"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--year and --season go together"));
}
