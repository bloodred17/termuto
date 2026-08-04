//! Which backing data source the CLI and terminal UI read from.

use std::env;
use std::fmt;
use std::str::FromStr;

/// The environment variable consulted when `--mode` is absent.
pub const MODE_ENV: &str = "TERMUTO_MODE";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum Mode {
    /// Read only the local Deeb catalog.
    Cached,
    /// Read only the Tenrai API. The default.
    #[default]
    Live,
    /// Read both: local catalog rows first, then the Tenrai API. A failed
    /// request degrades to the catalog instead of failing the screen.
    Hybrid,
}

impl Mode {
    pub fn uses_cache(self) -> bool {
        matches!(self, Self::Cached | Self::Hybrid)
    }

    pub fn uses_live(self) -> bool {
        matches!(self, Self::Live | Self::Hybrid)
    }

    /// The catalog is mandatory only in `cached` mode; `hybrid` treats a missing
    /// catalog as an empty fallback so the API alone still serves every screen.
    pub fn requires_catalog(self) -> bool {
        matches!(self, Self::Cached)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cached => formatter.pad("cached"),
            Self::Live => formatter.pad("live"),
            Self::Hybrid => formatter.pad("hybrid"),
        }
    }
}

impl FromStr for Mode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_lowercase().as_str() {
            "cached" => Ok(Self::Cached),
            "live" => Ok(Self::Live),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(format!(
                "unknown mode \"{other}\" (expected cached, live, or hybrid)"
            )),
        }
    }
}

/// Resolution order: `--mode`, then `TERMUTO_MODE`, then the `live` default.
/// An unparsable environment value is an error rather than a silent fallback.
pub fn resolve_mode(option: Option<Mode>) -> Result<Mode, String> {
    if let Some(mode) = option {
        return Ok(mode);
    }
    match env::var(MODE_ENV) {
        Ok(value) if !value.trim().is_empty() => {
            value.parse().map_err(|error: String| {
                format!("{MODE_ENV} is invalid: {error}")
            })
        }
        _ => Ok(Mode::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, resolve_mode};

    #[test]
    fn explicit_option_wins_and_defaults_to_live() {
        assert_eq!(resolve_mode(Some(Mode::Cached)), Ok(Mode::Cached));
        assert_eq!(Mode::default(), Mode::Live);
    }

    #[test]
    fn parsing_is_case_insensitive_and_rejects_unknown_values() {
        assert_eq!("HYBRID".parse(), Ok(Mode::Hybrid));
        assert!("offline".parse::<Mode>().is_err());
    }
}
