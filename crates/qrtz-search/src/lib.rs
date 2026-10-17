//! qrtz semantic search (ADR 0049): the [`Embedder`] seam with full-text-only search until a model has loaded,
//! the index step under its deadline, the bounded / resumable / cancelable vector backfill, chat retrieval and the
//! lexical + hybrid ranker. The full-text query builders (N46 phrases, N48 substrings, N49 bigrams) live in
//! qrtz-storage, next to the SQL that runs them. See `docs/rust-core-api.md` §qrtz-search.
pub mod backfill;
pub mod embedder;
pub mod index;
pub mod rank;
pub mod retrieve;

pub use backfill::{Backfill, PassReport, BACKFILL_PASS_LINES};
pub use embedder::{blob_to_vec, dot, vec_to_blob, Embedder, EmbedderState, LazyEmbedder};
pub use index::{index_vectors, SearchSteps, INDEX_DEADLINE_MS};
pub use rank::{hybrid_rank, query_terms, rank_sources, ChatSource, VecSource};
pub use retrieve::{retrieve, Retrieval};
