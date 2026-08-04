// Deeb 0.0.13's generated collection helpers wrap several expressions in
// `Ok(...?)`; the lint originates in that external derive expansion.
#![allow(clippy::needless_question_mark)]

use chrono::{DateTime, Utc};
use deeb::{Collection, DbResult, Deeb, Entity, FindManyOptions, Query, Transaction};
use serde::{Deserialize, Serialize};
use std::fmt;

/// One title in the local catalog. The derive supplies Deeb's typed query helpers.
#[derive(Collection, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[deeb(name = "anime", primary_key = "id")]
pub struct Anime {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub alternative_titles: Vec<String>,
    pub kind: AnimeKind,
    pub status: AnimeStatus,
    #[serde(default)]
    pub latest_release_at: Option<DateTime<Utc>>,
    pub description: String,
    #[serde(default)]
    pub episodes: Vec<Episode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Episode {
    pub number: u32,
    pub title: String,
    #[serde(default)]
    pub released_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimeKind {
    Series,
    Movie,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimeStatus {
    Ongoing,
    Completed,
    Upcoming,
}

impl fmt::Display for AnimeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Series => formatter.pad("Series"),
            Self::Movie => formatter.pad("Movie"),
        }
    }
}

impl fmt::Display for AnimeStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ongoing => formatter.pad("Ongoing"),
            Self::Completed => formatter.pad("Completed"),
            Self::Upcoming => formatter.pad("Upcoming"),
        }
    }
}
