//! ADR 0079: the Mac helper's own speech models (Nemotron ASR for recordings, Nemotron 3 diarization, LocalVQE echo
//! cancellation, Parakeet TDT v2 for echo dictation) for builds without the signed speech module. The files and their
//! sha256s are pinned in this binary (`local_models.json`, generated from the model host's tree at a fixed revision),
//! downloaded only from onboarding or settings through `qrtz-speech`'s resumable, size-capped, hash-checked downloader
//! (ADR 0078's one network path), into `<data>/models/<set>/` (0700). A set is usable once every file verified and its
//! `.qrtz-verified` marker is written; the helper reads only marked sets (`QRTZ_MODELS_DIR`) and never fetches
//! anything itself.
use qrtz_speech::bundle::FileEntry;
use qrtz_speech::download::Downloader;
/// The download's control and progress types, for the model sets that go through here (ctrl's Laya model, ADR
/// 0116): this module is the only qrtz-app caller of the downloader besides `setup.rs` (ADR 0078/0079).
pub use qrtz_speech::download::{DownloadControl, Progress};
use qrtz_speech::{fsx, SpeechError};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = include_str!("local_models.json");
/// Written last in a set's directory, holding the manifest version: the helper loads only marked sets.
pub const MARKER: &str = ".qrtz-verified";
/// The env var the helper reads the models root from.
pub const ENV: &str = "QRTZ_MODELS_DIR";

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub base: String,
    pub sets: Vec<ModelSet>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelSet {
    pub id: String,
    pub dir: String,
    pub repo: String,
    pub revision: String,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    pub remote: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    pub fn pinned() -> Manifest {
        serde_json::from_str(MANIFEST).expect("local_models.json")
    }
    pub fn total_size(&self) -> u64 {
        self.sets.iter().flat_map(|s| &s.files).map(|f| f.size).sum()
    }
}

pub struct LocalModels {
    root: PathBuf,
    manifest: Manifest,
    base: String,
}

fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && Path::new(p)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

impl LocalModels {
    /// The pinned manifest from the model host (`base` overrides the host: tests serve it from loopback).
    pub fn new(root: impl Into<PathBuf>, base: Option<String>) -> Self {
        Self::with_manifest(root, Manifest::pinned(), base)
    }

    pub fn with_manifest(root: impl Into<PathBuf>, manifest: Manifest, base: Option<String>) -> Self {
        for s in &manifest.sets {
            assert!(safe_rel(&s.dir), "model set dir {}", s.dir);
            for f in &s.files {
                assert!(safe_rel(&f.path) && safe_rel(&f.remote), "model file {}", f.path);
            }
        }
        let base = base.unwrap_or_else(|| manifest.base.clone());
        Self {
            root: root.into(),
            manifest,
            base,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn version(&self) -> &str {
        &self.manifest.version
    }
    pub fn total_size(&self) -> u64 {
        self.manifest.total_size()
    }

    fn set_ok(&self, s: &ModelSet) -> bool {
        let dir = self.root.join(&s.dir);
        std::fs::read_to_string(dir.join(MARKER)).is_ok_and(|v| v.trim() == self.manifest.version)
            && s.files
                .iter()
                .all(|f| std::fs::metadata(dir.join(&f.path)).is_ok_and(|m| m.is_file() && m.len() == f.size))
    }

    /// Every set downloaded, verified and marked for this manifest version (a cheap check: marker + sizes).
    pub fn installed(&self) -> bool {
        self.manifest.sets.iter().all(|s| self.set_ok(s))
    }

    /// Bytes under the models root (finished files and `.part`s).
    pub fn bytes_on_disk(&self) -> u64 {
        fn walk(d: &Path) -> u64 {
            std::fs::read_dir(d)
                .map(|it| {
                    it.flatten()
                        .map(|e| match e.file_type() {
                            Ok(t) if t.is_dir() => walk(&e.path()),
                            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                            _ => 0,
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(&self.root)
    }

    /// Downloads whatever is missing (resuming `.part` files), checks every file's size and sha256 before it is
    /// renamed into place, then marks each set. `force` re-fetches everything.
    pub async fn download(
        &self,
        force: bool,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), SpeechError> {
        if force {
            self.delete()?;
        }
        fsx::mkdir_private(&self.root)?;
        let dl = Downloader::new(&self.base);
        let total = self.total_size();
        let mut before = 0u64;
        for s in &self.manifest.sets {
            let size: u64 = s.files.iter().map(|f| f.size).sum();
            if self.set_ok(s) {
                before += size;
                on_progress(Progress { done: before, total });
                continue;
            }
            let dir = self.root.join(&s.dir);
            fsx::mkdir_private(&dir)?;
            let _ = std::fs::remove_file(dir.join(MARKER));
            let files: Vec<(String, FileEntry)> = s
                .files
                .iter()
                .map(|f| {
                    (
                        format!("{}/resolve/{}/{}", s.repo, s.revision, f.remote),
                        FileEntry {
                            path: f.path.clone(),
                            size: f.size,
                            sha256: f.sha256.clone(),
                        },
                    )
                })
                .collect();
            let off = before;
            dl.download_files(&files, &dir, ctl, &|p: Progress| {
                on_progress(Progress {
                    done: off + p.done,
                    total,
                })
            })
            .await?;
            fsx::write_private_atomic(&dir.join(MARKER), self.manifest.version.as_bytes())?;
            before += size;
        }
        Ok(())
    }

    /// Removes every set (and partial downloads). A recording that already loaded a model keeps it until it ends.
    pub fn delete(&self) -> std::io::Result<()> {
        for s in &self.manifest.sets {
            match std::fs::remove_dir_all(self.root.join(&s.dir)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_manifest_is_complete_and_safe() {
        let m = Manifest::pinned();
        let ids: Vec<&str> = m.sets.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["asr", "diarizer", "echo", "echo-asr"]);
        // The dirs the helper reads (mac/Models.swift `LocalModels`).
        let dirs: Vec<&str> = m.sets.iter().map(|s| s.dir.as_str()).collect();
        assert_eq!(
            dirs,
            [
                "nemotron-560ms",
                "nemotron3-diarizer",
                "localvqe",
                "parakeet-tdt-v2"
            ]
        );
        assert!(m.base.starts_with("https://"));
        for s in &m.sets {
            assert_eq!(s.revision.len(), 40, "{}: a pinned commit, not a branch", s.id);
            for f in &s.files {
                assert!(safe_rel(&f.path) && safe_rel(&f.remote), "{}", f.path);
                assert_eq!(f.sha256.len(), 64, "{}", f.path);
                assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            }
        }
        // What the helper loads must be there.
        let has = |set: &str, p: &str| {
            m.sets
                .iter()
                .find(|s| s.id == set)
                .unwrap()
                .files
                .iter()
                .any(|f| f.path == p)
        };
        assert!(has("asr", "encoder/encoder_int8.mlmodelc/weights/weight.bin"));
        assert!(has("asr", "decoder.mlmodelc/coremldata.bin"));
        assert!(has("asr", "joint.mlmodelc/coremldata.bin"));
        assert!(has("asr", "tokenizer.json"));
        assert!(has("asr", "metadata.json"));
        assert!(has(
            "diarizer",
            "Nemotron3Diarizer_fast32.mlmodelc/coremldata.bin"
        ));
        assert!(has("diarizer", "learnable_sil_emb.bin"));
        assert!(has("echo", "localvqe-v1.3-4.8M-16ms.mlmodelc/coremldata.bin"));
        // Echo's recogniser (Parakeet TDT v2, FluidAudio `AsrModels.loadLocal(version: .v2)`): its four models and
        // the vocabulary.
        for model in ["Preprocessor", "Encoder", "Decoder", "JointDecision"] {
            assert!(
                has("echo-asr", &format!("{model}.mlmodelc/coremldata.bin")),
                "{model}"
            );
            assert!(
                has("echo-asr", &format!("{model}.mlmodelc/weights/weight.bin")),
                "{model}"
            );
        }
        assert!(has("echo-asr", "parakeet_vocab.json"));
        let total = m.total_size();
        assert!((1_250_000_000..1_350_000_000).contains(&total), "{total}");
    }

    #[test]
    fn unsafe_paths_are_refused() {
        for p in ["../x", "/etc/passwd", "a/../../b", ""] {
            assert!(!safe_rel(p), "{p}");
        }
        assert!(safe_rel("encoder/encoder_int8.mlmodelc/weights/weight.bin"));
    }
}
