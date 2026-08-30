use std::num::NonZeroUsize;
use std::path::Path;

use cue_core::Result;

use crate::batch_recovery::RecoveryStore;

pub async fn run(
    target: Option<&Path>,
    jobs: NonZeroUsize,
    config: impl FnOnce() -> Result<cue_core::Config>,
) -> Result<i32> {
    let store = RecoveryStore::from_environment()?;
    let stored = match target {
        Some(target) => Some(select_explicit(&store, target)?),
        None => store.newest_incomplete(&std::env::current_dir()?)?,
    };
    let Some(stored) = stored else {
        println!("No incomplete batch to resume in the current directory.");
        return Ok(0);
    };
    crate::commands::process::resume_stored_batch(store, stored, jobs, config).await
}

pub(crate) fn select_explicit(
    store: &RecoveryStore,
    target: &Path,
) -> Result<crate::batch_recovery::StoredBatch> {
    let is_path = target.is_absolute()
        || target.components().count() > 1
        || target.extension().is_some()
        || std::fs::symlink_metadata(target).is_ok();
    if is_path {
        return store.load_path(target);
    }
    store.find_by_id(&target.to_string_lossy())?.ok_or_else(|| {
        cue_core::CueError::general(format!(
            "batch recovery target {} was not found",
            target.display()
        ))
        .remedy("run `cue batches list` or provide an existing recovery-state file")
    })
}
