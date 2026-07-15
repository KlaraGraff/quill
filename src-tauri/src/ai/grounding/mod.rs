//! Local, rebuildable grounding data for book chat.

pub mod chunk;
pub mod extract;
pub mod index;
pub mod retrieve;
pub mod segment;
pub mod summarize;
pub mod vector;

pub const INDEX_VERSION: i64 = 1;
pub const RETRIEVAL_TOP_K: usize = 12;
pub const RETRIEVAL_BUDGET_TOKENS: usize = 4_000;
pub const OVERVIEW_BUDGET_TOKENS: usize = 1_500;
pub const CHUNK_TARGET_TOKENS: usize = 350;
pub const CHUNK_MAX_TOKENS: usize = 500;
pub const SNIPPET_MAX_CHARS: usize = 120;

pub use extract::{BlockText, SectionText};
pub use index::{index_status, IndexStatus};
pub use retrieve::{retrieve, CitedSource, RetrievedChunk, SpoilerCutoff};
