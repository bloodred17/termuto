//! Handing a resolved stream to an external player.
//!
//! The player is spawned detached: it opens in its own window and the terminal
//! is never given up, so the TUI keeps drawing and the CLI returns immediately.
//! Nothing waits on the child, so finished players are reaped opportunistically
//! before each new spawn rather than lingering as zombies for the session.
//!
//! Because nothing waits, a player that dies on its first request — a rejected
//! manifest, a missing file — would otherwise fail invisibly. Its stderr is
//! therefore appended to a log rather than discarded: writing it to the terminal
//! would corrupt the TUI's alternate screen, and discarding it turns a 403 into
//! a silent no-op.

use super::provider::Stream;
use anyhow::{Context, Result, bail};
use std::env;
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The environment variable consulted when `--player` is absent.
pub const PLAYER_ENV: &str = "TERMUTO_PLAYER";
pub const DEFAULT_PLAYER: &str = "mpv";

/// Resolution order: `--player`, then `TERMUTO_PLAYER`, then `mpv`.
pub fn resolve_player(option: Option<String>) -> String {
    option
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            env::var(PLAYER_ENV)
                .ok()
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| DEFAULT_PLAYER.to_string())
        .trim()
        .to_string()
}

pub struct Player {
    program: String,
    log: PathBuf,
    /// Spawned players that have not been reaped yet.
    running: Vec<Child>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Player")
            .field("program", &self.program)
            .field("log", &self.log)
            .field("running", &self.running.len())
            .finish()
    }
}

impl Player {
    pub fn new(program: String) -> Self {
        Self {
            program,
            log: env::temp_dir().join("termuto-player.log"),
            running: Vec::new(),
        }
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    /// Where the player's own diagnostics go.
    pub fn log_path(&self) -> &Path {
        &self.log
    }

    /// Starts `stream` and returns as soon as the player is running.
    pub fn play(&mut self, stream: &Stream, title: &str) -> Result<()> {
        self.reap();
        let arguments = player_arguments(&self.program, stream, title);
        // A log that cannot be opened must not stop playback; the stream may
        // well play, and the only thing lost is the diagnostics.
        let errors = File::options()
            .create(true)
            .append(true)
            .open(&self.log)
            .map_or_else(|_| Stdio::null(), Stdio::from);
        let child = Command::new(&self.program)
            .args(&arguments)
            // Detached: the player must not read the terminal or draw over it.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(errors)
            .spawn();

        match child {
            Ok(child) => {
                self.running.push(child);
                Ok(())
            }
            Err(error) if error.kind() == ErrorKind::NotFound => bail!(
                "Could not find the player \"{}\" on PATH. Install it, or choose another \
                 with --player or {PLAYER_ENV}.",
                self.program
            ),
            Err(error) => Err(error)
                .with_context(|| format!("Could not start the player \"{}\"", self.program)),
        }
    }

    /// Collects players that have already exited. Ignores the ones still going.
    fn reap(&mut self) {
        self.running
            .retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_)) | Err(_)));
    }

    /// Whether any player started here is still going. Reaps first, so this
    /// answers about live players rather than unclaimed exit statuses.
    pub fn any_running(&mut self) -> bool {
        self.reap();
        !self.running.is_empty()
    }
}

/// Built separately from the spawn so the command line can be asserted on
/// without starting a process. Only mpv's flags are known here; any other player
/// is given the URL alone, which every player accepts.
pub fn player_arguments(program: &str, stream: &Stream, title: &str) -> Vec<String> {
    if !is_mpv(program) {
        return vec![stream.url.clone()];
    }

    let mut arguments = Vec::new();
    if !title.trim().is_empty() {
        arguments.push(format!("--force-media-title={title}"));
    }
    if !stream.headers.is_empty() {
        // mpv takes one comma-separated list, not a flag per header.
        let fields = stream
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join(",");
        arguments.push(format!("--http-header-fields={fields}"));
    }
    for track in &stream.subtitles {
        arguments.push(format!("--sub-file={track}"));
    }
    // `--` keeps a URL that starts with a dash from being read as a flag.
    arguments.push("--".to_string());
    arguments.push(stream.url.clone());
    arguments
}

fn is_mpv(program: &str) -> bool {
    std::path::Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("mpv"))
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PLAYER, PLAYER_ENV, player_arguments, resolve_player};
    use crate::playback::prefs::Audio;
    use crate::playback::provider::Stream;

    fn stream() -> Stream {
        Stream {
            url: "https://example.test/index.m3u8".into(),
            provider: "mock".into(),
            headers: vec![("Referer".into(), "https://example.test/".into())],
            subtitles: vec!["https://example.test/en.vtt".into()],
            audio: Audio::Sub,
            quality: Some("1080".into()),
            strip_segment_prefix: false,
        }
    }

    #[test]
    fn mpv_receives_headers_subtitles_and_a_title() {
        let arguments = player_arguments("mpv", &stream(), "Example — episode 1");
        assert!(arguments.contains(&"--force-media-title=Example — episode 1".to_string()));
        assert!(
            arguments.contains(&"--http-header-fields=Referer: https://example.test/".to_string())
        );
        assert!(arguments.contains(&"--sub-file=https://example.test/en.vtt".to_string()));
        // The URL is last, and guarded by `--`.
        assert_eq!(arguments[arguments.len() - 2], "--");
        assert_eq!(arguments.last().expect("a url"), &stream().url);
    }

    #[test]
    fn an_absolute_mpv_path_is_still_recognised_as_mpv() {
        assert!(player_arguments("/usr/bin/mpv", &stream(), "x").len() > 1);
    }

    #[test]
    fn other_players_are_given_the_url_alone() {
        assert_eq!(player_arguments("vlc", &stream(), "x"), vec![stream().url]);
    }

    #[test]
    fn the_player_defaults_to_mpv_and_the_flag_wins() {
        assert_eq!(resolve_player(None), DEFAULT_PLAYER);
        assert_eq!(resolve_player(Some(" vlc ".into())), "vlc");
        assert!(!PLAYER_ENV.is_empty());
    }
}
