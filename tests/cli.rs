use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog.json")
}

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_termuto"))
}

#[test]
fn latest_prints_sorted_catalog() {
    command()
        .args([
            "--catalog",
            catalog_path().to_str().expect("utf-8 path"),
            "latest",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Latest releases"))
        .stdout(predicate::str::contains("Solo Leveling Season 2"));
}

#[test]
fn search_prints_case_insensitive_matches() {
    command()
        .args([
            "search",
            "SOLO",
            "--catalog",
            catalog_path().to_str().expect("utf-8 path"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 results for \"SOLO\""))
        .stdout(predicate::str::contains("Solo Leveling Season 2"));
}

#[test]
fn no_result_search_succeeds() {
    command()
        .args([
            "--catalog",
            catalog_path().to_str().expect("utf-8 path"),
            "search",
            "not a title",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "No anime found for \"not a title\".",
        ));
}
