//! N25: the app data dir (DB, logs, config, models, tombstones) and the recordings dir are owner-only (0700)
//! whatever the umask, like Electron's `userData` (`apps/desktop/src/main/index.ts`) and `NoteService`'s
//! recordings dir (N18). A directory an older build created 0755 is tightened too.
use std::io;
use std::path::{Path, PathBuf};

/// Create `dir` (and missing parents) 0700, then chmod it 0700 in case it already existed with a wider mode.
pub fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Where the Tauri build keeps its data. `recordings` is what `RuntimeDeps::recordings_dir` gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    pub data: PathBuf,
    pub recordings: PathBuf,
}

impl AppDirs {
    /// Boot step: make both directories owner-only before anything writes into them.
    pub fn prepare(data: impl Into<PathBuf>) -> io::Result<AppDirs> {
        let data = data.into();
        let recordings = data.join("recordings");
        private_dir(&data)?;
        private_dir(&recordings)?;
        Ok(AppDirs { data, recordings })
    }
}
