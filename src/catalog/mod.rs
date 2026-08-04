//! Catalog models and the Deeb-backed catalog repository.

pub mod model;
pub mod repository;

pub use model::{Anime, AnimeKind, AnimeStatus, Episode};
pub use repository::CatalogRepository;
