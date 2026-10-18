//! Model sets as packages (ADR 0157 §6). A model set pinned in the qrtz binary today (the Hugging Face repo, the
//! exact revision, every file's size and sha256) becomes a `model.*` package: a `qrtz.json` plus a tree whose files
//! carry the pinned Hugging Face URL as an extra mirror. The publisher builds the same tree from the same pins, so the
//! package the hosted index serves is byte-for-byte this one (same tree sha256), and an install made before the index
//! existed is already the indexed package.
//!
//! There is no second download path: these trees go through the one fetcher, the store and the swap like every
//! package, and the Hugging Face URL is only ever a mirror of bytes whose hash is already pinned.
use crate::index::{canonical, IndexFile, Tree, MANIFEST_FILE};
use crate::install::PinnedPackage;
use crate::qrtz_json::{Model, PackageManifest};
use serde::{Deserialize, Serialize};

/// The Hugging Face host the pinned mirror URLs name.
pub const HUGGING_FACE: &str = "https://huggingface.co";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFile {
    /// The file's path in the upstream repo.
    pub remote: String,
    /// Its path in the package.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSource {
    /// `model.parakeet-tdt-v2-en`.
    pub package: String,
    pub version: String,
    pub task: String,
    /// `FluidInference/parakeet-tdt-0.6b-v2-coreml`.
    pub repo: String,
    pub revision: String,
    pub license: String,
    pub files: Vec<ModelFile>,
}

impl ModelSource {
    pub fn manifest(&self) -> PackageManifest {
        PackageManifest {
            name: self.package.clone(),
            version: self.version.clone(),
            kind: crate::index::PackageKind::Model,
            description: String::new(),
            dependencies: Default::default(),
            extension: None,
            service: None,
            model: Some(Model {
                task: self.task.clone(),
                upstream: self.repo.clone(),
                revision: self.revision.clone(),
                license: self.license.clone(),
            }),
        }
    }

    /// Canonical `qrtz.json` bytes (so the same pins always give the same tree).
    pub fn manifest_bytes(&self) -> Vec<u8> {
        canonical(&self.manifest())
    }

    /// The pinned Hugging Face URL of one file.
    pub fn mirror_url(&self, f: &ModelFile) -> String {
        format!(
            "{HUGGING_FACE}/{}/resolve/{}/{}",
            self.repo, self.revision, f.remote
        )
    }

    pub fn tree(&self) -> Tree {
        let m = self.manifest_bytes();
        let mut files: Vec<IndexFile> = self
            .files
            .iter()
            .map(|f| IndexFile {
                path: f.path.clone(),
                size: f.size,
                sha256: f.sha256.clone(),
                executable: false,
                mirrors: vec![self.mirror_url(f)],
            })
            .collect();
        files.push(IndexFile {
            path: MANIFEST_FILE.into(),
            size: m.len() as u64,
            sha256: crate::sha256_hex(&m),
            executable: false,
            mirrors: vec![],
        });
        Tree::new(files)
    }

    pub fn pinned(&self, team_id: &str) -> PinnedPackage {
        PinnedPackage {
            name: self.package.clone(),
            version: self.version.clone(),
            team_id: team_id.into(),
            tree: self.tree(),
            manifest: self.manifest_bytes(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinnedFile {
    version: String,
    sets: Vec<PinnedSet>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinnedSet {
    package: String,
    task: String,
    #[serde(default)]
    package_version: Option<String>,
    #[serde(default)]
    license: String,
    repo: String,
    revision: String,
    files: Vec<ModelFile>,
}

/// The model sets of an app pin file (`local_models.json`, `ctrl_laya_model.json`): each set names its package,
/// task, upstream repo and revision, and every file's path, size and sha256.
pub fn parse_pinned(bytes: &[u8]) -> Result<Vec<ModelSource>, String> {
    let f: PinnedFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    f.sets
        .into_iter()
        .map(|s| {
            let version = s.package_version.unwrap_or_else(|| f.version.clone());
            crate::manifest::exact_version("packageVersion", &version).map_err(|e| e.to_string())?;
            let m = ModelSource {
                package: s.package,
                version,
                task: s.task,
                repo: s.repo,
                revision: s.revision,
                license: s.license,
                files: s.files,
            };
            m.manifest().validate().map_err(|e| e.to_string())?;
            m.tree().validate().map_err(|e| format!("{}: {e}", m.package))?;
            Ok(m)
        })
        .collect()
}
