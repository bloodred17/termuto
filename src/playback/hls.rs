//! Reading a master playlist and picking a rendition out of it.
//!
//! Shared by every extractor, because the choice is the host's problem only in
//! where the playlist comes from: once fetched, a master playlist is a master
//! playlist. Hosts list their renditions worst-first, so a player handed the
//! master would open the lowest one — the wanted variant is resolved here and
//! the player is given a single media playlist instead.

use super::prefs::Quality;
use reqwest::Url;
use std::cmp::Reverse;

/// One rendition listed in a master playlist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub url: String,
    /// The vertical resolution, which is how a quality preference names it.
    /// Read from `RESOLUTION` rather than `NAME`, because hosts mislabel: one
    /// megavid variant is `NAME="480p"` at `RESOLUTION=640x360`.
    pub height: Option<u32>,
    pub bandwidth: u64,
}

const STREAM_INF: &str = "#EXT-X-STREAM-INF:";

pub fn parse_variants(master: &str, base: &Url) -> Vec<Variant> {
    let lines: Vec<&str> = master.lines().map(str::trim).collect();
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let attributes = line.strip_prefix(STREAM_INF)?;
            // The URI follows the tag, past any blank or commented lines.
            let uri = lines[index + 1..]
                .iter()
                .find(|line| !line.is_empty() && !line.starts_with('#'))?;
            Some(Variant {
                url: base.join(uri).ok()?.to_string(),
                height: attribute(attributes, "RESOLUTION")
                    .and_then(|resolution| resolution.rsplit('x').next()?.parse().ok()),
                bandwidth: attribute(attributes, "BANDWIDTH")
                    .and_then(|bandwidth| bandwidth.parse().ok())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Splitting on commas can cut a quoted value such as `CODECS="a,b"` in half,
/// but only into fragments that match no name asked for here.
fn attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    attributes
        .split(',')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .map(|value| value.trim_matches('"'))
}

/// An exact quality that the host does not carry degrades to the nearest
/// rendition it does, per [`Quality::Exact`], rather than failing playback.
/// `None` means the playlist listed no variants — it is already a media
/// playlist, and the caller should use it as it is.
pub fn choose_variant<'a>(variants: &'a [Variant], quality: &Quality) -> Option<&'a Variant> {
    let highest = || variants.iter().max_by_key(|variant| variant.bandwidth);
    let Quality::Exact(height) = quality else {
        return highest();
    };
    let Ok(wanted) = height.parse::<u32>() else {
        return highest();
    };
    variants
        .iter()
        .filter_map(|variant| Some((variant, variant.height?)))
        .min_by_key(|(variant, height)| (height.abs_diff(wanted), Reverse(variant.bandwidth)))
        .map(|(variant, _)| variant)
        .or_else(highest)
}

#[cfg(test)]
mod tests {
    use super::{Quality, Url, Variant, choose_variant, parse_variants};

    const MASTER: &str = "\
#EXTM3U
#EXT-X-VERSION:4
#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360
360/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720
720/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=5300000,RESOLUTION=1920x1080
1080/index.m3u8
";

    fn base() -> Url {
        Url::parse("https://hls2.example.test/v/a/b/c/master.m3u8").expect("a base")
    }

    #[test]
    fn variants_are_read_with_their_height_and_bandwidth_and_resolved_against_the_master() {
        let variants = parse_variants(MASTER, &base());
        assert_eq!(variants.len(), 3);
        assert_eq!(
            variants[2],
            Variant {
                url: "https://hls2.example.test/v/a/b/c/1080/index.m3u8".into(),
                height: Some(1080),
                bandwidth: 5_300_000,
            }
        );
    }

    /// The playlist lists its renditions worst-first, so the default pick has to
    /// be resolved here rather than left to the player.
    #[test]
    fn the_best_quality_is_the_highest_bandwidth_not_the_first_listed() {
        let variants = parse_variants(MASTER, &base());
        let chosen = choose_variant(&variants, &Quality::Best).expect("a variant");
        assert_eq!(chosen.height, Some(1080));
    }

    #[test]
    fn an_exact_quality_is_matched_and_an_absent_one_degrades_to_the_nearest() {
        let variants = parse_variants(MASTER, &base());
        let height = |quality: &str| {
            choose_variant(&variants, &quality.parse().expect("a quality"))
                .expect("a variant")
                .height
        };
        assert_eq!(height("720"), Some(720));
        // 1440 is not carried; the nearest rendition plays instead of failing.
        assert_eq!(height("1440"), Some(1080));
        assert_eq!(height("240"), Some(360));
    }

    #[test]
    fn a_media_playlist_with_no_variants_selects_nothing() {
        let media = "#EXTM3U\n#EXTINF:4.0,\nseg-1.ts\n";
        assert!(choose_variant(&parse_variants(media, &base()), &Quality::Best).is_none());
    }

    /// megavid labels a 640x360 rendition `NAME="480p"`, so the name is ignored
    /// and the resolution is what a quality preference is matched against.
    #[test]
    fn a_mislabelled_variant_is_matched_on_its_real_resolution() {
        let master = "\
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,NAME=\"480p\"
low.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720,NAME=\"720p\"
mid.m3u8
";
        let variants = parse_variants(master, &base());
        assert_eq!(variants[0].height, Some(360));
        let chosen = choose_variant(&variants, &Quality::Exact("480".into())).expect("a variant");
        // 480 is nearer 360 than 720, and the bogus name plays no part.
        assert_eq!(chosen.height, Some(360));
    }
}
