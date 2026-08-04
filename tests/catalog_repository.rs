use termuto_poc::catalog::CatalogRepository;
use std::{fs, path::PathBuf};
use tempfile::TempDir;

fn catalog_file(contents: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("catalog.json");
    fs::write(&path, contents).expect("catalog file");
    (directory, path)
}

fn valid_catalog() -> &'static str {
    r#"{
      "anime": {
        "solo": {
          "id": "solo",
          "title": "Solo Leveling",
          "alternative_titles": ["Ore dake Level Up na Ken"],
          "kind": "series",
          "status": "ongoing",
          "latest_release_at": "2026-08-03T18:30:00Z",
          "description": "A series.",
          "episodes": [{"number": 1, "title": "One", "released_at": "2026-08-01T18:30:00Z"}]
        },
        "frieren": {
          "id": "frieren",
          "title": "Frieren",
          "alternative_titles": ["Sousou no Frieren"],
          "kind": "series",
          "status": "completed",
          "latest_release_at": "2026-07-28T18:30:00Z",
          "description": "A second series.",
          "episodes": [{"number": 1, "title": "The Journey's End", "released_at": "2026-07-01T18:30:00Z"}]
        },
        "movie": {
          "id": "movie",
          "title": "Look Back",
          "alternative_titles": [],
          "kind": "movie",
          "status": "completed",
          "latest_release_at": "2026-07-15T00:00:00Z",
          "description": "A movie.",
          "episodes": []
        }
      }
    }"#
}

#[tokio::test]
async fn loads_catalog_records_through_repository() {
    let (_directory, path) = catalog_file(valid_catalog());
    let repository = CatalogRepository::open(path).await.expect("opens catalog");
    assert_eq!(repository.load_all().await.expect("loads records").len(), 3);
}

#[tokio::test]
async fn latest_is_descending_and_limited() {
    let (_directory, path) = catalog_file(valid_catalog());
    let repository = CatalogRepository::open(path).await.expect("opens catalog");
    let latest = repository.latest(2).await.expect("latest records");
    assert_eq!(
        latest
            .iter()
            .map(|anime| anime.id.as_str())
            .collect::<Vec<_>>(),
        ["solo", "frieren"]
    );
}

#[tokio::test]
async fn ongoing_filters_records() {
    let (_directory, path) = catalog_file(valid_catalog());
    let repository = CatalogRepository::open(path).await.expect("opens catalog");
    let ongoing = repository.ongoing().await.expect("ongoing records");
    assert_eq!(ongoing.len(), 1);
    assert_eq!(ongoing[0].id, "solo");
}

#[tokio::test]
async fn search_is_case_insensitive_and_searches_aliases() {
    let (_directory, path) = catalog_file(valid_catalog());
    let repository = CatalogRepository::open(path).await.expect("opens catalog");
    assert_eq!(
        repository.search("  SOLO ").await.expect("title search")[0].id,
        "solo"
    );
    assert_eq!(
        repository.search("sousou").await.expect("alias search")[0].id,
        "frieren"
    );
    assert!(
        repository
            .search("  ")
            .await
            .expect("empty search")
            .is_empty()
    );
}

#[tokio::test]
async fn duplicate_anime_ids_are_rejected() {
    let catalog = valid_catalog().replace("\"id\": \"frieren\"", "\"id\": \"solo\"");
    let (_directory, path) = catalog_file(&catalog);
    let error = CatalogRepository::open(path)
        .await
        .expect_err("duplicate must fail");
    assert!(format!("{error:#}").contains("Duplicate anime ID: solo"));
}

#[tokio::test]
async fn series_and_movie_rules_are_enforced() {
    let empty_series = valid_catalog().replace(
        "[{\"number\": 1, \"title\": \"One\", \"released_at\": \"2026-08-01T18:30:00Z\"}]",
        "[]",
    );
    let (_directory, path) = catalog_file(&empty_series);
    let error = CatalogRepository::open(path)
        .await
        .expect_err("empty series must fail");
    assert!(format!("{error:#}").contains("must contain at least one episode"));

    let movie_episodes = valid_catalog().replace(
        "\"episodes\": []",
        "\"episodes\": [{\"number\": 1, \"title\": \"Nope\"}]",
    );
    let (_directory, path) = catalog_file(&movie_episodes);
    let error = CatalogRepository::open(path)
        .await
        .expect_err("movie episodes must fail");
    assert!(format!("{error:#}").contains("must not contain episodes"));
}

#[tokio::test]
async fn invalid_episode_numbers_are_rejected() {
    let catalog = valid_catalog().replace(
        "\"number\": 1, \"title\": \"One\"",
        "\"number\": 0, \"title\": \"One\"",
    );
    let (_directory, path) = catalog_file(&catalog);
    let error = CatalogRepository::open(path)
        .await
        .expect_err("episode zero must fail");
    assert!(format!("{error:#}").contains("Episode numbers must be greater than zero"));
}

#[tokio::test]
async fn malformed_catalog_is_actionable() {
    let (_directory, path) = catalog_file("not json");
    let error = CatalogRepository::open(path)
        .await
        .expect_err("malformed catalog must fail");
    assert!(error.to_string().contains("malformed or incompatible"));
}
