//! Which audio track and which rendition playback should ask a provider for.

use std::env;
use std::fmt;
use std::str::FromStr;

/// The environment variables consulted when the matching flag is absent.
pub const AUDIO_ENV: &str = "TERMUTO_AUDIO";
pub const QUALITY_ENV: &str = "TERMUTO_QUALITY";

/// Which audio track a provider should resolve.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum Audio {
    /// Original audio with subtitles. The default.
    #[default]
    Sub,
    /// A dubbed audio track, when the provider has one.
    Dub,
}

impl fmt::Display for Audio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sub => formatter.pad("sub"),
            Self::Dub => formatter.pad("dub"),
        }
    }
}

impl FromStr for Audio {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_lowercase().as_str() {
            "sub" | "subbed" => Ok(Self::Sub),
            "dub" | "dubbed" => Ok(Self::Dub),
            other => Err(format!("unknown audio \"{other}\" (expected sub or dub)")),
        }
    }
}

/// Which rendition to prefer. A provider that cannot offer it falls back to the
/// closest it has rather than failing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Quality {
    /// The highest rendition the provider offers. The default.
    #[default]
    Best,
    /// A named vertical resolution, e.g. `1080`.
    Exact(String),
}

impl fmt::Display for Quality {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Best => formatter.pad("best"),
            Self::Exact(height) => formatter.pad(&format!("{height}p")),
        }
    }
}

impl FromStr for Quality {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim().to_lowercase();
        if trimmed == "best" || trimmed == "max" {
            return Ok(Self::Best);
        }
        // `1080` and `1080p` are the same request; anything else is a typo
        // worth reporting rather than silently treating as "best".
        let digits = trimmed.strip_suffix('p').unwrap_or(&trimmed);
        if !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit()) {
            Ok(Self::Exact(digits.to_string()))
        } else {
            Err(format!(
                "unknown quality \"{value}\" (expected best or a height such as 1080)"
            ))
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackPrefs {
    pub audio: Audio,
    pub quality: Quality,
}

impl fmt::Display for TrackPrefs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.audio, self.quality)
    }
}

/// Resolution order per field: the flag, then the environment variable, then the
/// default. An unparsable environment value is an error rather than a silent
/// fallback, matching how [`crate::mode::resolve_mode`] treats `TERMUTO_MODE`.
pub fn resolve_prefs(audio: Option<Audio>, quality: Option<Quality>) -> Result<TrackPrefs, String> {
    Ok(TrackPrefs {
        audio: resolve_field(audio, AUDIO_ENV)?,
        quality: resolve_field(quality, QUALITY_ENV)?,
    })
}

fn resolve_field<T>(option: Option<T>, variable: &str) -> Result<T, String>
where
    T: Default + FromStr<Err = String>,
{
    if let Some(value) = option {
        return Ok(value);
    }
    match env::var(variable) {
        Ok(raw) if !raw.trim().is_empty() => raw
            .parse()
            .map_err(|error: String| format!("{variable} is invalid: {error}")),
        _ => Ok(T::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Audio, Quality, TrackPrefs, resolve_prefs};

    #[test]
    fn flags_win_and_the_defaults_are_sub_at_best_quality() {
        let prefs = resolve_prefs(Some(Audio::Dub), None).expect("resolves");
        assert_eq!(prefs.audio, Audio::Dub);
        assert_eq!(prefs.quality, Quality::Best);
        assert_eq!(TrackPrefs::default().to_string(), "sub best");
    }

    #[test]
    fn quality_accepts_a_bare_height_or_a_p_suffix() {
        assert_eq!("1080".parse(), Ok(Quality::Exact("1080".into())));
        assert_eq!("720P".parse(), Ok(Quality::Exact("720".into())));
        assert_eq!("BEST".parse(), Ok(Quality::Best));
        assert!("ultra".parse::<Quality>().is_err());
    }

    #[test]
    fn audio_parsing_is_case_insensitive_and_rejects_unknown_values() {
        assert_eq!("DUB".parse(), Ok(Audio::Dub));
        assert_eq!("subbed".parse(), Ok(Audio::Sub));
        assert!("japanese".parse::<Audio>().is_err());
    }
}
