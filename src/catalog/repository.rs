use super::{Anime, AnimeKind};
use anyhow::{Context, Result, bail};
use deeb::{Deeb, Query};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The sole catalog access point for both the CLI and terminal UI.
#[derive(Clone, Debug)]
pub struct CatalogRepository {
    db: Deeb,
    path: PathBuf,
}

impl CatalogRepository {
    /// Opens an existing JSON instance with Deeb and validates every typed record.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.is_file() {
            bail!(
                "Catalog not found at {}.\nPass --catalog <PATH>, set TERMUTO_CATALOG, or place a catalog at ~/.termuto/catalog.json.",
                path.display()
            );
        }

        let path_text = path
            .to_str()
            .context("Catalog path is not valid UTF-8 and cannot be passed to Deeb")?;
        let db = Deeb::new();
        db.add_instance("catalog", path_text, vec![Anime::entity()])
            .await
            .with_context(|| {
                format!(
                    "Catalog at {} is malformed or incompatible with Deeb 0.0.13",
                    path.display()
                )
            })?;

        let repository = Self { db, path };
        repository.validate_all().await?;
        Ok(repository)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Retrieves every record through Deeb's generated `find_many` operation.
    pub async fn load_all(&self) -> Result<Vec<Anime>> {
        Anime::find_many(&self.db, Query::all(), None, None)
            .await
            .context("Deeb could not query the anime collection")
            .map(|records| records.unwrap_or_default())
    }

    pub async fn latest(&self, limit: usize) -> Result<Vec<Anime>> {
        let mut records = self.load_all().await?;
        sort_by_latest_release(&mut records);
        records.truncate(limit);
        Ok(records)
    }

    pub async fn ongoing(&self) -> Result<Vec<Anime>> {
        // Deeb can express the status predicate cleanly. Sorting remains application-side
        // because the typed chrono timestamp is represented as a JSON string on disk.
        let mut records = Anime::find_many(&self.db, Query::eq("status", "ongoing"), None, None)
            .await
            .context("Deeb could not query ongoing anime")?
            .unwrap_or_default();
        sort_by_latest_release(&mut records);
        Ok(records)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let normalized = query.trim().to_lowercase();
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        // Query::like is case-sensitive in Deeb 0.0.13 and cannot cover aliases in
        // this schema. Fetch with Deeb, then apply predictable normalized matching here.
        let mut records: Vec<_> = self
            .load_all()
            .await?
            .into_iter()
            .filter(|anime| {
                anime.title.to_lowercase().contains(&normalized)
                    || anime
                        .alternative_titles
                        .iter()
                        .any(|title| title.to_lowercase().contains(&normalized))
            })
            .collect();
        sort_by_latest_release(&mut records);
        Ok(records)
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<Anime>> {
        Anime::find_many(&self.db, Query::eq("id", id), None, None)
            .await
            .context("Deeb could not query an anime by id")
            .map(|records| records.and_then(|mut records| records.pop()))
    }

    async fn validate_all(&self) -> Result<()> {
        let records = self.load_all().await?;
        validate_records(&records)
            .with_context(|| format!("Invalid catalog at {}", self.path.display()))
    }
}

fn sort_by_latest_release(records: &mut [Anime]) {
    records.sort_by(|left, right| {
        right
            .latest_release_at
            .cmp(&left.latest_release_at)
            .then_with(|| left.title.cmp(&right.title))
    });
}

fn validate_records(records: &[Anime]) -> Result<()> {
    let mut ids = HashSet::new();
    for anime in records {
        if anime.id.trim().is_empty() {
            bail!("Anime IDs must not be empty");
        }
        if anime.title.trim().is_empty() {
            bail!("Anime titles must not be empty (id: {})", anime.id);
        }
        if !ids.insert(anime.id.as_str()) {
            bail!("Duplicate anime ID: {}", anime.id);
        }

        match anime.kind {
            AnimeKind::Series if anime.episodes.is_empty() => {
                bail!("Series {} must contain at least one episode", anime.id);
            }
            AnimeKind::Movie if !anime.episodes.is_empty() => {
                bail!("Movie {} must not contain episodes", anime.id);
            }
            _ => {}
        }

        let mut episode_numbers = HashSet::new();
        for episode in &anime.episodes {
            if episode.number == 0 {
                bail!(
                    "Episode numbers must be greater than zero (anime: {})",
                    anime.id
                );
            }
            if !episode_numbers.insert(episode.number) {
                bail!(
                    "Duplicate episode number {} in series {}",
                    episode.number,
                    anime.id
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sort_by_latest_release;
    use crate::catalog::{Anime, AnimeKind, AnimeStatus};

    #[test]
    fn undated_releases_sort_last() {
        let mut anime = vec![
            Anime {
                id: "undated".into(),
                title: "Undated".into(),
                alternative_titles: vec![],
                kind: AnimeKind::Movie,
                status: AnimeStatus::Upcoming,
                latest_release_at: None,
                description: String::new(),
                source: None,
                episodes: vec![],
            },
            Anime {
                id: "dated".into(),
                title: "Dated".into(),
                alternative_titles: vec![],
                kind: AnimeKind::Movie,
                status: AnimeStatus::Completed,
                latest_release_at: Some("2026-08-01T00:00:00Z".parse().expect("valid date")),
                description: String::new(),
                source: None,
                episodes: vec![],
            },
        ];
        sort_by_latest_release(&mut anime);
        assert_eq!(anime[0].id, "dated");
    }
}
