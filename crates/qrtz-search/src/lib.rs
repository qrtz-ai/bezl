//! qrtz semantic search (ADR 0049): the [`Embedder`] seam with full-text-only search until a model has loaded,
//! the index step under its deadline, the bounded / resumable / cancelable vector backfill, chat retrieval and the
//! lexical + hybrid ranker. The full-text query builders (N46 phrases, N48 substrings, N49 bigrams) live in
//! qrtz-storage, next to the SQL that runs them. See `docs/rust-core-api.md` §qrtz-search.
pub mod backfill;
pub mod embedder;
pub mod index;
#[cfg(feature = "minilm")]
pub mod minilm;
pub mod rank;
pub mod retrieve;

pub use backfill::{Backfill, BusyFn, PassReport, BACKFILL_PASS_LINES, BUSY_POLL};
pub use embedder::{blob_to_vec, dot, same_embedder, vec_to_blob, Embedder, EmbedderState, LazyEmbedder};
pub use index::{index_vectors, index_vectors_paused, SearchSteps, INDEX_DEADLINE_MS};
pub use rank::{hybrid_rank, query_terms, rank_sources, ChatSource, VecSource};
pub use retrieve::{retrieve, retrieve_visible, HiddenFn, Retrieval, CHAT_SOURCES};
