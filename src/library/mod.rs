//! The two lists the user builds themselves: the titles they starred, and the
//! episodes they played.
//!
//! Both live in one JSON file beside the catalog. They are kept together
//! because they share a problem: the same title is `Origin::Cached` in one mode
//! and `Origin::Live` in another, so neither list can be keyed by origin. Both
//! match on the folded title instead — the same key hybrid mode already uses to
//! merge two sources — and keep the origin only to reopen the row with.

use crate::source::{AnimeSummary, Origin};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Consulted when `--library` is absent.
pub const LIBRARY_ENV: &str = "TERMUTO_LIBRARY";

/// How many plays are kept. Long enough to be a history, short enough that the
/// file stays something the app can rewrite on every play without thinking
/// about it.
const WATCH_LIMIT: usize = 500;

/// A starred title. The whole listing row is kept rather than an id, so the
/// favourites screen draws the same columns as the listing it was starred from
/// without going back to the source for them.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Favourite {
    #[serde(flatten)]
    pub summary: AnimeSummary,
    pub added_at: DateTime<Utc>,
}

/// One play. Recorded when the stream reaches the player, which is as much as
/// can be known: the player is detached, so nothing reports back that an
/// episode finished.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Watch {
    pub origin: Origin,
    pub title: String,
    /// `None` for a movie or a single-part title.
    #[serde(default)]
    pub episode: Option<u32>,
    pub watched_at: DateTime<Utc>,
    /// Whether the episode pickers draw a check beside this episode. Alt-D
    /// clears it, which leaves the play in the history while taking the mark
    /// off a row the user does not count as watched.
    #[serde(default = "marked_by_default")]
    pub marked: bool,
}

fn marked_by_default() -> bool {
    true
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Library {
    #[serde(default)]
    favourites: Vec<Favourite>,
    #[serde(default)]
    watched: Vec<Watch>,
    #[serde(skip)]
    path: PathBuf,
    /// Why the file on disk could not be read. Set only when there was a file
    /// and it did not parse, which is the one case where saving would destroy
    /// something — so it also stops every write.
    #[serde(skip)]
    issue: Option<String>,
}

impl Library {
    /// Reads the library, or starts an empty one. Never fails: a library that
    /// cannot be read is not a reason to refuse to browse, so the problem is
    /// reported through [`Self::issue`] and the lists come up empty.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut library = match fs::read_to_string(&path) {
            Ok(text) if text.trim().is_empty() => Self::default(),
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(library) => library,
                Err(error) => Self {
                    issue: Some(format!("{} is not readable: {error}", path.display())),
                    ..Self::default()
                },
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => Self {
                issue: Some(format!("{} could not be opened: {error}", path.display())),
                ..Self::default()
            },
        };
        library.path = path;
        library
    }

    pub fn issue(&self) -> Option<&str> {
        self.issue.as_deref()
    }

    /// The starred titles, most recently starred first.
    pub fn favourites(&self) -> Vec<AnimeSummary> {
        let mut entries: Vec<&Favourite> = self.favourites.iter().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.added_at));
        entries
            .into_iter()
            .map(|entry| entry.summary.clone())
            .collect()
    }

    pub fn is_favourite(&self, title: &str) -> bool {
        let wanted = key(title);
        self.favourites
            .iter()
            .any(|entry| key(&entry.summary.title) == wanted)
    }

    /// Stars the title, or unstars it if it is already starred. Returns whether
    /// it is starred afterwards.
    pub fn toggle_favourite(&mut self, summary: &AnimeSummary) -> Result<bool> {
        let wanted = key(&summary.title);
        let starred = match self
            .favourites
            .iter()
            .position(|entry| key(&entry.summary.title) == wanted)
        {
            Some(position) => {
                self.favourites.remove(position);
                false
            }
            None => {
                self.favourites.push(Favourite {
                    // The note explains why a row was recommended, which says
                    // nothing about the title itself and would read as noise on
                    // the favourites screen.
                    summary: AnimeSummary {
                        note: None,
                        ..summary.clone()
                    },
                    added_at: Utc::now(),
                });
                true
            }
        };
        self.save()?;
        Ok(starred)
    }

    /// The plays, most recent first.
    pub fn watched(&self) -> &[Watch] {
        &self.watched
    }

    /// Records a play, moving an episode that was played before back to the top
    /// rather than listing it twice. A re-play arrives marked again: playing
    /// something is the act the check is meant to record.
    pub fn record_watch(
        &mut self,
        origin: &Origin,
        title: &str,
        episode: Option<u32>,
    ) -> Result<()> {
        self.watched
            .retain(|watch| !matches(watch, &key(title), episode));
        self.watched.insert(
            0,
            Watch {
                origin: origin.clone(),
                title: title.to_string(),
                episode,
                watched_at: Utc::now(),
                marked: true,
            },
        );
        self.watched.truncate(WATCH_LIMIT);
        self.save()
    }

    /// Whether the episode pickers should draw a check beside this episode.
    pub fn is_marked(&self, title: &str, episode: Option<u32>) -> bool {
        let key = key(title);
        self.watched
            .iter()
            .any(|watch| matches(watch, &key, episode) && watch.marked)
    }

    /// Flips the check on an episode that was played. An episode with no play
    /// behind it has no check to flip, so this does nothing there.
    pub fn toggle_mark(&mut self, title: &str, episode: Option<u32>) -> Result<()> {
        let key = key(title);
        let Some(watch) = self
            .watched
            .iter_mut()
            .find(|watch| matches(watch, &key, episode))
        else {
            return Ok(());
        };
        watch.marked = !watch.marked;
        self.save()
    }

    /// Writes the whole file. Both lists are small and every change is one key
    /// press, so there is nothing to gain from batching — and a crash between a
    /// star and a flush would be the kind of loss that is hard to explain.
    ///
    /// The write lands on a temporary file and is renamed over the real one, so
    /// an interrupted save leaves the previous library rather than half of a new
    /// one.
    fn save(&self) -> Result<()> {
        if let Some(issue) = &self.issue {
            anyhow::bail!("Refusing to overwrite the library — {issue}");
        }
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", parent.display()))?;
        }

        let text = serde_json::to_string_pretty(self).context("Could not encode the library")?;
        let temporary = temporary_path(&self.path);
        fs::write(&temporary, text)
            .with_context(|| format!("Could not write {}", temporary.display()))?;
        fs::rename(&temporary, &self.path)
            .with_context(|| format!("Could not save the library at {}", self.path.display()))?;
        Ok(())
    }
}

/// Resolution order matches the catalog's: the flag, then the environment
/// variable, then `~/.termuto/library.json`.
pub fn resolve_library_path(option: Option<PathBuf>) -> PathBuf {
    option
        .or_else(|| env::var_os(LIBRARY_ENV).map(PathBuf::from))
        .unwrap_or_else(|| {
            env::home_dir()
                .map(|home| home.join(".termuto").join("library.json"))
                .unwrap_or_else(|| PathBuf::from("library.json"))
        })
}

/// What two lists match a title on. The same folding as
/// [`AnimeSummary::dedupe_key`], so a title starred from the API is still
/// starred when the catalog serves it under its own id.
pub fn key(title: &str) -> String {
    title.trim().to_lowercase()
}

fn matches(watch: &Watch, key: &str, episode: Option<u32>) -> bool {
    watch.episode == episode && self::key(&watch.title) == key
}

/// A library in a scratch file of its own. Tests that need one go through this
/// rather than the real path, so none of them read or write `~/.termuto`.
#[cfg(test)]
pub(crate) fn scratch() -> Library {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = env::temp_dir().join(format!(
        "termuto-scratch-{}-{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_file(&path);
    Library::open(path)
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::{Library, Watch};
    use crate::source::{AnimeSummary, Origin};

    fn summary(origin: Origin, title: &str) -> AnimeSummary {
        AnimeSummary {
            origin,
            title: title.to_string(),
            kind: "TV".into(),
            status: "Finished".into(),
            score: Some(9.3),
            episodes: Some(28),
            released: "2023-09-29".into(),
            note: None,
        }
    }

    /// Every test writes somewhere of its own, so none of them touch a real
    /// library in `$HOME`.
    fn library(name: &str) -> Library {
        let path = std::env::temp_dir()
            .join("termuto-library-tests")
            .join(format!("{name}.json"));
        let _ = std::fs::remove_file(&path);
        Library::open(path)
    }

    #[test]
    fn starring_twice_unstars_and_the_star_survives_a_reload() {
        let mut library = library("favourites");
        let frieren = summary(Origin::Live(52991), "Frieren");
        assert!(library.toggle_favourite(&frieren).expect("saves"));
        assert!(library.is_favourite("Frieren"));

        let reopened = Library::open(library.path.clone());
        assert!(reopened.is_favourite("Frieren"));
        assert_eq!(reopened.favourites().len(), 1);

        library.toggle_favourite(&frieren).expect("saves");
        assert!(!library.is_favourite("Frieren"));
        assert!(Library::open(library.path.clone()).favourites().is_empty());
    }

    /// The whole point of folding the title: the same show carries its star
    /// between the catalog and the API, which give it different origins.
    #[test]
    fn a_star_follows_the_title_across_origins() {
        let mut library = library("origins");
        library
            .toggle_favourite(&summary(Origin::Live(52991), "Frieren"))
            .expect("saves");
        assert!(library.is_favourite("  frieren "));
        assert!(!library.is_favourite("Bocchi the Rock!"));
    }

    #[test]
    fn a_replayed_episode_moves_to_the_top_instead_of_listing_twice() {
        let mut library = library("watched");
        let origin = Origin::Live(52991);
        library
            .record_watch(&origin, "Frieren", Some(1))
            .expect("saves");
        library
            .record_watch(&origin, "Frieren", Some(2))
            .expect("saves");
        library
            .record_watch(&origin, "Frieren", Some(1))
            .expect("saves");

        let episodes: Vec<Option<u32>> = library
            .watched()
            .iter()
            .map(|watch| watch.episode)
            .collect();
        assert_eq!(episodes, [Some(1), Some(2)]);
    }

    /// Alt-D takes the check off without losing the play, and puts it back.
    #[test]
    fn clearing_a_mark_keeps_the_play_in_the_history() {
        let mut library = library("marks");
        library
            .record_watch(&Origin::Live(52991), "Frieren", Some(3))
            .expect("saves");
        assert!(library.is_marked("Frieren", Some(3)));

        library.toggle_mark("Frieren", Some(3)).expect("saves");
        assert!(!library.is_marked("Frieren", Some(3)));
        assert_eq!(library.watched().len(), 1);

        // And it survives a reload, which is what the field is in the file for.
        let reopened = Library::open(library.path.clone());
        assert!(!reopened.is_marked("Frieren", Some(3)));

        library.toggle_mark("Frieren", Some(3)).expect("saves");
        assert!(library.is_marked("Frieren", Some(3)));
    }

    /// An episode nobody played has no check to flip, so the key is harmless
    /// rather than a way to invent history.
    #[test]
    fn clearing_a_mark_on_an_unplayed_episode_does_nothing() {
        let mut library = library("unplayed");
        library.toggle_mark("Frieren", Some(3)).expect("saves");
        assert!(library.watched().is_empty());
    }

    /// A library that does not parse must not be written over: the file is the
    /// only copy of both lists.
    #[test]
    fn an_unreadable_library_comes_up_empty_and_refuses_to_save() {
        let path = std::env::temp_dir()
            .join("termuto-library-tests")
            .join("broken.json");
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("creates the dir");
        std::fs::write(&path, "{ not json").expect("writes");

        let mut library = Library::open(&path);
        assert!(library.issue().is_some());
        assert!(library.favourites().is_empty());
        assert!(
            library
                .toggle_favourite(&summary(Origin::Live(1), "Frieren"))
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), "{ not json");
    }

    #[test]
    fn an_entry_written_before_marks_existed_reads_as_marked() {
        let watch: Watch = serde_json::from_str(
            r#"{"origin":{"live":1},"title":"Frieren","episode":3,
                "watched_at":"2026-08-05T00:00:00Z"}"#,
        )
        .expect("parses");
        assert!(watch.marked);
    }
}
