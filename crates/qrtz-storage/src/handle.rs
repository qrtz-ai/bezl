use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use tokio::sync::oneshot;

use crate::{Db, Result, StorageError};

type Job = Box<dyn FnOnce(&mut Db) + Send>;

/// Async front door to the database: one dedicated OS thread owns the writer connection (like the Electron
/// worker's single `DatabaseSync`). Jobs run FIFO, so writes stay ordered — the TS `saveChain` guarantee comes
/// for free. A panicking job becomes [`StorageError::Panicked`] and the thread keeps serving (an open
/// transaction is rolled back first), the in-process stand-in for the worker's crash isolation (F15).
#[derive(Clone)]
pub struct StorageHandle {
    tx: mpsc::Sender<Job>,
}

impl StorageHandle {
    /// Opens and migrates `path` on a new storage thread. Returns once the database is ready.
    pub fn spawn(path: PathBuf) -> Result<StorageHandle> {
        StorageHandle::spawn_with(move || Db::open(path))
    }

    /// In-memory database (tests).
    pub fn spawn_in_memory() -> Result<StorageHandle> {
        StorageHandle::spawn_with(Db::open_in_memory)
    }

    fn spawn_with(open: impl FnOnce() -> Result<Db> + Send + 'static) -> Result<StorageHandle> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        thread::Builder::new()
            .name("qrtz-storage".into())
            .spawn(move || {
                let mut db = match open() {
                    Ok(db) => {
                        let _ = ready_tx.send(Ok(()));
                        db
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                for job in rx {
                    job(&mut db);
                }
            })?;
        ready_rx.recv().map_err(|_| StorageError::Closed)??;
        Ok(StorageHandle { tx })
    }

    fn submit<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> Result<R> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<R>>> {
        let (done, wait) = oneshot::channel();
        let job: Job = Box::new(move |db: &mut Db| {
            let out = match catch_unwind(AssertUnwindSafe(|| f(db))) {
                Ok(r) => r,
                Err(p) => {
                    if !db.connection().is_autocommit() {
                        let _ = db.connection().execute_batch("ROLLBACK");
                    }
                    let msg = p
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic".into());
                    Err(StorageError::Panicked(msg))
                }
            };
            let _ = done.send(out);
        });
        self.tx.send(job).map_err(|_| StorageError::Closed)?;
        Ok(wait)
    }

    /// Runs `f` on the storage thread and awaits its result.
    pub async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        self.submit(f)?.await.map_err(|_| StorageError::Closed)?
    }

    /// Blocking variant for non-async callers (CLI, tests). Do not call from inside a tokio runtime thread.
    pub fn call_blocking<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        self.submit(f)?
            .blocking_recv()
            .map_err(|_| StorageError::Closed)?
    }
}
