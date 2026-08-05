use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog.json")
}

/// Every test here runs offline, so the mode is pinned rather than left to the
/// `live` default. The environment is cleared so an exported value cannot change
/// what is being asserted.
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_termuto"));
    command
        .env_remove("TERMUTO_MODE")
        .env_remove("TERMUTO_CATALOG")
        .env_remove("TERMUTO_PLAYER")
        .env_remove("TERMUTO_AUDIO")
        .env_remove("TERMUTO_PROVIDER")
        .env_remove("TERMUTO_QUALITY");
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

/// `true` stands in for a media player: it accepts any arguments and exits at
/// once, so playback is exercised end to end without opening a window.
fn playable() -> Command {
    let mut command = cached();
    command.args(["--player", "true"]);
    command
}

#[test]
fn playing_a_catalog_episode_uses_the_source_the_catalog_points_at() {
    playable()
        .args(["play", "solo", "--episode", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Solo Leveling Season 2 episode 2",
        ))
        .stdout(predicate::str::contains("via catalog"))
        .stdout(predicate::str::contains(
            "media/solo-leveling-season-2/02.mkv",
        ));
}

#[test]
fn a_catalog_movie_plays_without_an_episode_number() {
    playable()
        .args(["play", "look back"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Playing Look Back via catalog"))
        .stdout(predicate::str::contains("media/look-back.mkv"));
}

/// `frieren` carries no `source`, so the catalog provider declines. ZokoAnime is
/// addressed by MyAnimeList id, which a catalog row does not carry, so it
/// declines too — and the failure names both rather than playing something else.
#[test]
fn a_catalog_row_without_a_source_is_served_by_nothing_in_the_chain() {
    playable()
        .args(["play", "frieren"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("catalog, zokoanime"));
}

#[test]
fn playing_an_episode_the_catalog_does_not_have_is_an_error() {
    playable()
        .args(["play", "solo", "--episode", "99"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no episode 99"));
}

#[test]
fn a_missing_player_names_itself_and_how_to_change_it() {
    cached()
        .args(["--player", "definitely-not-a-player", "play", "solo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("definitely-not-a-player"))
        .stderr(predicate::str::contains("TERMUTO_PLAYER"));
}

/// Silently ignoring an unknown host would look like the choice took effect,
/// and the stream would quietly come from the wrong place.
#[test]
fn an_unknown_provider_is_rejected_and_names_the_known_ones() {
    cached()
        .args(["--provider", "nyaa", "play", "solo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nyaa"))
        .stderr(predicate::str::contains("zokoanime, megavid"));

    command()
        .env("TERMUTO_PROVIDER", "nyaa")
        .args(["--mode", "cached", "latest"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nyaa"));
}

/// Choosing a host must not disturb the catalog, which still leads the chain.
#[test]
fn choosing_a_host_leaves_catalog_rows_playing_from_the_catalog() {
    playable()
        .args(["--provider", "megavid", "play", "solo", "--episode", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("via catalog"))
        .stdout(predicate::str::contains(
            "media/solo-leveling-season-2/02.mkv",
        ));
}

#[test]
fn an_unknown_quality_is_rejected() {
    cached()
        .args(["--quality", "ultra", "play", "solo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ultra"));

    command()
        .env("TERMUTO_QUALITY", "ultra")
        .args(["--mode", "cached", "latest"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("TERMUTO_QUALITY is invalid"));
}

#[test]
fn playing_something_the_catalog_does_not_have_is_an_error() {
    playable()
        .args(["play", "not a title"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No anime found"));
}
