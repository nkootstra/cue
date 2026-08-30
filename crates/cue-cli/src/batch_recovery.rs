//! Durable state for recoverable media batches.
#![allow(
    dead_code,
    reason = "U1 defines the recovery boundary before later units integrate processing and commands"
)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cue_core::config::SubtitleFormat;
use cue_core::error::PersistentFailure;
use cue_core::{CueError, Result};
use serde::{Deserialize, Serialize};

pub(crate) const BATCH_SCHEMA_VERSION: u32 = 1;
const BATCH_DIRECTORY: &str = "batches";
static BATCH_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RecoveryProcessMode {
    Full,
    TranscriptOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessingIntent {
    pub mode: RecoveryProcessMode,
    pub language: Option<String>,
    pub subtitle_formats: Vec<SubtitleFormat>,
    pub summary: bool,
    pub stream: bool,
    pub corrections: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attempt {
    pub number: u32,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum ItemState {
    Pending,
    Running {
        attempt: Attempt,
    },
    Complete {
        attempt: Attempt,
    },
    Failed {
        attempt: Attempt,
        failure: PersistentFailure,
    },
    Missing {
        attempt: Attempt,
        failure: PersistentFailure,
    },
    NeedsReprocessing {
        attempt: Attempt,
        failure: PersistentFailure,
    },
}

impl ItemState {
    pub(crate) fn latest_attempt(&self) -> Option<&Attempt> {
        match self {
            Self::Pending => None,
            Self::Running { attempt }
            | Self::Complete { attempt }
            | Self::Failed { attempt, .. }
            | Self::Missing { attempt, .. }
            | Self::NeedsReprocessing { attempt, .. } => Some(attempt),
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    fn validate(&self) -> Result<()> {
        let Some(attempt) = self.latest_attempt() else {
            return Ok(());
        };
        if attempt.number == 0 {
            return Err(CueError::general("batch attempt numbers must start at one"));
        }
        match self {
            Self::Running { .. } if attempt.finished_at_ms.is_some() => Err(CueError::general(
                "a running batch item cannot have a finished timestamp",
            )),
            Self::Running { .. } => Ok(()),
            _ if attempt.finished_at_ms.is_none() => Err(CueError::general(
                "a terminal batch item must have a finished timestamp",
            )),
            _ if attempt.finished_at_ms < Some(attempt.started_at_ms) => Err(CueError::general(
                "a batch attempt cannot finish before it started",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BatchItem {
    pub position: u32,
    pub source: PathBuf,
    pub workspace: PathBuf,
    pub published_base: PathBuf,
    pub state: ItemState,
}

impl BatchItem {
    pub(crate) fn start(&mut self, started_at_ms: u64) -> Result<u32> {
        let number = match &self.state {
            ItemState::Pending => 1,
            ItemState::Failed { attempt, .. }
            | ItemState::Missing { attempt, .. }
            | ItemState::NeedsReprocessing { attempt, .. } => attempt
                .number
                .checked_add(1)
                .ok_or_else(|| CueError::general("batch item attempt number overflowed"))?,
            ItemState::Running { .. } => {
                return Err(CueError::general("batch item is already running"));
            }
            ItemState::Complete { .. } => {
                return Err(CueError::general(
                    "a complete batch item must fail verification before reprocessing",
                ));
            }
        };
        self.state = ItemState::Running {
            attempt: Attempt {
                number,
                started_at_ms,
                finished_at_ms: None,
            },
        };
        Ok(number)
    }

    pub(crate) fn complete(&mut self, finished_at_ms: u64) -> Result<()> {
        let attempt = self.finish_attempt(finished_at_ms)?;
        self.state = ItemState::Complete { attempt };
        Ok(())
    }

    pub(crate) fn fail(&mut self, finished_at_ms: u64, failure: PersistentFailure) -> Result<()> {
        let attempt = self.finish_attempt(finished_at_ms)?;
        self.state = ItemState::Failed { attempt, failure };
        Ok(())
    }

    pub(crate) fn mark_missing(
        &mut self,
        finished_at_ms: u64,
        failure: PersistentFailure,
    ) -> Result<()> {
        let attempt = self.finish_attempt(finished_at_ms)?;
        self.state = ItemState::Missing { attempt, failure };
        Ok(())
    }

    pub(crate) fn mark_needs_reprocessing(&mut self, failure: PersistentFailure) -> Result<()> {
        let ItemState::Complete { attempt } = &self.state else {
            return Err(CueError::general(
                "only a complete batch item can require verification reprocessing",
            ));
        };
        self.state = ItemState::NeedsReprocessing {
            attempt: attempt.clone(),
            failure,
        };
        Ok(())
    }

    fn mark_running_needs_reprocessing(
        &mut self,
        finished_at_ms: u64,
        failure: PersistentFailure,
    ) -> Result<()> {
        let attempt = self.finish_attempt(finished_at_ms)?;
        self.state = ItemState::NeedsReprocessing { attempt, failure };
        Ok(())
    }

    fn finish_attempt(&self, finished_at_ms: u64) -> Result<Attempt> {
        let ItemState::Running { attempt } = &self.state else {
            return Err(CueError::general(
                "only a running batch item can enter a terminal state",
            ));
        };
        if finished_at_ms < attempt.started_at_ms {
            return Err(CueError::general(
                "a batch attempt cannot finish before it started",
            ));
        }
        let mut attempt = attempt.clone();
        attempt.finished_at_ms = Some(finished_at_ms);
        Ok(attempt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BatchCounts {
    pub complete: usize,
    pub incomplete: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reconciliation {
    Unchanged,
    ReconciledComplete,
    NeedsReprocessing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BatchRecord {
    pub schema_version: u32,
    pub id: String,
    pub cue_version: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub cwd: PathBuf,
    pub intent: ProcessingIntent,
    pub items: Vec<BatchItem>,
}

impl BatchRecord {
    pub(crate) fn from_json(json: &[u8]) -> Result<Self> {
        let record: Self = serde_json::from_slice(json).map_err(|error| {
            CueError::general("could not parse batch recovery state").because(error.to_string())
        })?;
        record.validate()?;
        Ok(record)
    }

    pub(crate) fn to_json(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec_pretty(self).map_err(|error| {
            CueError::general("could not serialize batch recovery state").because(error.to_string())
        })
    }

    pub(crate) fn counts(&self) -> BatchCounts {
        let complete = self
            .items
            .iter()
            .filter(|item| item.state.is_complete())
            .count();
        BatchCounts {
            complete,
            incomplete: self.items.len() - complete,
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.counts().incomplete == 0
    }

    pub(crate) fn has_running_items(&self) -> bool {
        self.items.iter().any(|item| item.state.is_running())
    }

    pub(crate) fn reconcile_item(
        &mut self,
        position: u32,
        finished_at_ms: u64,
    ) -> Result<Reconciliation> {
        let batch_id = self.id.clone();
        let item = self
            .items
            .iter_mut()
            .find(|item| item.position == position)
            .ok_or_else(|| CueError::general(format!("batch item {position} does not exist")))?;
        let attempt_number = match &item.state {
            ItemState::Running { attempt } | ItemState::Complete { attempt } => attempt.number,
            _ => return Ok(Reconciliation::Unchanged),
        };

        let expected_attempt = crate::run_contract::BatchAttemptRef {
            batch_id,
            item_position: position,
            attempt_number,
        };
        let (reusable, diagnostic_id) = match crate::verification::verify_output(&item.workspace) {
            Ok(verified)
                if verified.is_valid()
                    && verified.receipt.batch_attempt.as_ref() == Some(&expected_attempt) =>
            {
                (true, None)
            }
            Ok(verified) => (
                false,
                Some(
                    verified
                        .diagnostics
                        .first()
                        .map(|diagnostic| diagnostic.id)
                        .unwrap_or("CUE-VERIFY-ATTEMPT-MISMATCH"),
                ),
            ),
            Err(error) => (false, Some(error.diagnostic_id())),
        };

        match &item.state {
            ItemState::Running { .. } if reusable => {
                item.complete(finished_at_ms)?;
                Ok(Reconciliation::ReconciledComplete)
            }
            ItemState::Complete { .. } if reusable => Ok(Reconciliation::Unchanged),
            ItemState::Running { .. } => {
                let failure = verification_failure(diagnostic_id.unwrap());
                item.mark_running_needs_reprocessing(finished_at_ms, failure)?;
                Ok(Reconciliation::NeedsReprocessing)
            }
            ItemState::Complete { .. } => {
                let failure = verification_failure(diagnostic_id.unwrap());
                item.mark_needs_reprocessing(failure)?;
                Ok(Reconciliation::NeedsReprocessing)
            }
            _ => Ok(Reconciliation::Unchanged),
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != BATCH_SCHEMA_VERSION {
            return Err(CueError::general(format!(
                "unsupported batch recovery schema version {}; this cue supports schema version {BATCH_SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        validate_id(&self.id)?;
        if self.cue_version.trim().is_empty() {
            return Err(CueError::general("batch recovery state has no cue version"));
        }
        if self.updated_at_ms < self.created_at_ms {
            return Err(CueError::general(
                "batch recovery state was updated before it was created",
            ));
        }
        require_absolute(&self.cwd, "working directory")?;
        if let Some(path) = &self.intent.corrections {
            require_absolute(path, "corrections path")?;
        }
        if self.items.is_empty() {
            return Err(CueError::general("batch recovery state has no items"));
        }
        let mut positions = HashSet::with_capacity(self.items.len());
        for item in &self.items {
            if !positions.insert(item.position) {
                return Err(CueError::general(format!(
                    "batch recovery state repeats item position {}",
                    item.position
                )));
            }
            require_absolute(&item.source, "source path")?;
            require_absolute(&item.workspace, "workspace path")?;
            require_absolute(&item.published_base, "published output path")?;
            item.state.validate()?;
        }
        if positions
            .iter()
            .copied()
            .max()
            .is_none_or(|last| last as usize + 1 != self.items.len())
            || !positions.contains(&0)
        {
            return Err(CueError::general(
                "batch recovery item positions must be contiguous from zero",
            ));
        }
        Ok(())
    }
}

fn verification_failure(diagnostic_id: &str) -> PersistentFailure {
    CueError::new(
        cue_core::PipelineStage::Render,
        format!("recorded output is not reusable ({diagnostic_id})"),
    )
    .remedy("run cue resume to process this item again")
    .persistent_failure()
}

fn require_absolute(path: &std::path::Path, label: &str) -> Result<()> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(CueError::general(format!(
            "batch recovery {label} must be absolute: {}",
            path.display()
        )))
    }
}

fn validate_id(id: &str) -> Result<()> {
    if !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(CueError::general("batch recovery state has an invalid ID"))
    }
}

pub(crate) trait JournalWriter: Send + Sync {
    fn write(&self, path: &Path, content: &[u8]) -> Result<()>;
}

#[derive(Debug)]
struct AtomicJournalWriter;

impl JournalWriter for AtomicJournalWriter {
    fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
        crate::run_contract::write_atomic(path, content)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBatch {
    pub path: PathBuf,
    pub record: BatchRecord,
}

#[derive(Debug)]
pub(crate) enum BatchListing {
    Readable(StoredBatch),
    Unreadable { path: PathBuf, reason: String },
}

#[derive(Clone)]
pub(crate) struct RecoveryStore {
    root: PathBuf,
    writer: Arc<dyn JournalWriter>,
    mutation: Arc<Mutex<()>>,
}

pub(crate) struct BatchLock {
    _file: std::fs::File,
}

pub(crate) enum LockAttempt {
    Acquired(BatchLock),
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchActivity {
    Complete,
    Incomplete,
    Active,
    Interrupted,
}

impl BatchLock {
    pub(crate) fn try_acquire(record_path: &Path) -> Result<LockAttempt> {
        let lock_path = lock_path(record_path)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| {
                CueError::general(format!(
                    "could not open batch recovery lock {}",
                    lock_path.display()
                ))
                .because(error.to_string())
            })?;
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => Ok(LockAttempt::Acquired(Self { _file: file })),
            Err(fs4::TryLockError::WouldBlock) => Ok(LockAttempt::Busy),
            Err(fs4::TryLockError::Error(error)) => Err(CueError::general(format!(
                "could not lock batch recovery state {}",
                record_path.display()
            ))
            .because(error.to_string())),
        }
    }
}

impl RecoveryStore {
    pub(crate) fn from_environment() -> Result<Self> {
        Self::from_state_root(cue_core::paths::state_dir())
    }

    fn from_state_root(root: Option<PathBuf>) -> Result<Self> {
        let root = root.ok_or_else(|| {
            CueError::general("could not determine cue's recovery state directory").remedy(
                "set CUE_STATE_DIR, XDG_STATE_HOME, or HOME to an absolute writable directory",
            )
        })?;
        Ok(Self::new(root))
    }

    pub(crate) fn new(root: PathBuf) -> Self {
        Self::with_writer(root, Arc::new(AtomicJournalWriter))
    }

    pub(crate) fn with_writer(root: PathBuf, writer: Arc<dyn JournalWriter>) -> Self {
        Self {
            root,
            writer,
            mutation: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn create(
        &self,
        cwd: &Path,
        intent: ProcessingIntent,
        items: Vec<BatchItem>,
    ) -> Result<StoredBatch> {
        let cwd = canonical_cwd(cwd)?;
        let now = unix_time_ms()?;
        let record = BatchRecord {
            schema_version: BATCH_SCHEMA_VERSION,
            id: new_batch_id(now),
            cue_version: env!("CARGO_PKG_VERSION").to_owned(),
            created_at_ms: now,
            updated_at_ms: now,
            cwd,
            intent,
            items,
        };
        let path = self.save(&record)?;
        Ok(StoredBatch { path, record })
    }

    /// Publish a new batch only after acquiring the lock that will guard its
    /// initial execution. Readers can therefore never observe a new journal
    /// in an unlocked handoff window.
    pub(crate) fn create_and_lock(
        &self,
        cwd: &Path,
        intent: ProcessingIntent,
        items: Vec<BatchItem>,
    ) -> Result<(StoredBatch, BatchLock)> {
        let cwd = canonical_cwd(cwd)?;
        let now = unix_time_ms()?;
        let record = BatchRecord {
            schema_version: BATCH_SCHEMA_VERSION,
            id: new_batch_id(now),
            cue_version: env!("CARGO_PKG_VERSION").to_owned(),
            created_at_ms: now,
            updated_at_ms: now,
            cwd,
            intent,
            items,
        };
        let content = record.to_json()?;
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| CueError::general("batch recovery state lock was poisoned"))?;
        let directory = self.root.join(BATCH_DIRECTORY).join(scope_key(&record.cwd));
        std::fs::create_dir_all(&directory).map_err(|error| {
            CueError::general(format!(
                "could not create batch recovery directory {}",
                directory.display()
            ))
            .because(error.to_string())
        })?;
        let path = directory.join(format!("{}.json", record.id));
        let lock = match BatchLock::try_acquire(&path)? {
            LockAttempt::Acquired(lock) => lock,
            LockAttempt::Busy => {
                return Err(CueError::general(format!(
                    "new batch recovery state {} is unexpectedly busy",
                    record.id
                )));
            }
        };
        self.writer.write(&path, &content)?;
        Ok((StoredBatch { path, record }, lock))
    }

    pub(crate) fn save(&self, record: &BatchRecord) -> Result<PathBuf> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| CueError::general("batch recovery state lock was poisoned"))?;
        self.save_locked(record)
    }

    fn save_locked(&self, record: &BatchRecord) -> Result<PathBuf> {
        let content = record.to_json()?;
        let scope = scope_key(&record.cwd);
        let directory = self.root.join(BATCH_DIRECTORY).join(scope);
        std::fs::create_dir_all(&directory).map_err(|error| {
            CueError::general(format!(
                "could not create batch recovery directory {}",
                directory.display()
            ))
            .because(error.to_string())
        })?;
        let path = directory.join(format!("{}.json", record.id));
        self.writer.write(&path, &content)?;
        Ok(path)
    }

    pub(crate) fn update<F>(&self, path: &Path, update: F) -> Result<StoredBatch>
    where
        F: FnOnce(&mut BatchRecord) -> Result<()>,
    {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| CueError::general("batch recovery state lock was poisoned"))?;
        let mut record = self.load_path(path)?.record;
        update(&mut record)?;
        record.updated_at_ms = unix_time_ms()?.max(record.updated_at_ms);
        let content = record.to_json()?;
        self.writer.write(path, &content)?;
        Ok(StoredBatch {
            path: path.to_owned(),
            record,
        })
    }

    pub(crate) fn load_path(&self, path: &Path) -> Result<StoredBatch> {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            CueError::general(format!(
                "could not inspect batch recovery state {}",
                path.display()
            ))
            .because(error.to_string())
        })?;
        if !metadata.file_type().is_file() {
            return Err(CueError::general(format!(
                "batch recovery target {} is not a regular file",
                path.display()
            )));
        }
        let json = std::fs::read(path).map_err(|error| {
            CueError::general(format!(
                "could not read batch recovery state {}",
                path.display()
            ))
            .because(error.to_string())
        })?;
        let record = BatchRecord::from_json(&json)?;
        Ok(StoredBatch {
            path: path.to_owned(),
            record,
        })
    }

    pub(crate) fn list_scope(&self, cwd: &Path) -> Result<Vec<BatchListing>> {
        let canonical = canonical_cwd(cwd)?;
        let directory = self.root.join(BATCH_DIRECTORY).join(scope_key(&canonical));
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(CueError::general(format!(
                    "could not list batch recovery directory {}",
                    directory.display()
                ))
                .because(error.to_string()));
            }
        };
        let mut listings = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                CueError::general(format!(
                    "could not enumerate batch recovery directory {}",
                    directory.display()
                ))
                .because(error.to_string())
            })?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            match self.load_path(&path) {
                Ok(stored) if stored.record.cwd == canonical => {
                    listings.push(BatchListing::Readable(stored));
                }
                Ok(_) => listings.push(BatchListing::Unreadable {
                    path,
                    reason: "record working directory does not match its scope".into(),
                }),
                Err(error) => listings.push(BatchListing::Unreadable {
                    path,
                    reason: error.to_string().trim().to_owned(),
                }),
            }
        }
        listings.sort_by(|left, right| listing_order(right).cmp(&listing_order(left)));
        Ok(listings)
    }

    pub(crate) fn newest_incomplete(&self, cwd: &Path) -> Result<Option<StoredBatch>> {
        Ok(self
            .list_scope(cwd)?
            .into_iter()
            .filter_map(|listing| match listing {
                BatchListing::Readable(stored) if !stored.record.is_complete() => Some(stored),
                _ => None,
            })
            .max_by(|left, right| {
                (left.record.created_at_ms, &left.record.id)
                    .cmp(&(right.record.created_at_ms, &right.record.id))
            }))
    }

    pub(crate) fn find_by_id(&self, id: &str) -> Result<Option<StoredBatch>> {
        validate_id(id)?;
        let batches = self.root.join(BATCH_DIRECTORY);
        let scopes = match std::fs::read_dir(&batches) {
            Ok(scopes) => scopes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(CueError::general(format!(
                    "could not search batch recovery directory {}",
                    batches.display()
                ))
                .because(error.to_string()));
            }
        };
        let mut found = None;
        for scope in scopes {
            let path = scope
                .map_err(|error| {
                    CueError::general("could not enumerate batch recovery scopes")
                        .because(error.to_string())
                })?
                .path()
                .join(format!("{id}.json"));
            if path.exists() {
                if found.is_some() {
                    return Err(CueError::general(format!(
                        "batch recovery ID {id} is ambiguous"
                    )));
                }
                found = Some(self.load_path(&path)?);
            }
        }
        Ok(found)
    }

    pub(crate) fn activity(&self, stored: &StoredBatch) -> Result<BatchActivity> {
        if stored.record.is_complete() {
            return Ok(BatchActivity::Complete);
        }
        if !stored.record.has_running_items() {
            return Ok(BatchActivity::Incomplete);
        }
        match BatchLock::try_acquire(&stored.path)? {
            LockAttempt::Busy => Ok(BatchActivity::Active),
            LockAttempt::Acquired(lock) => {
                drop(lock);
                Ok(BatchActivity::Interrupted)
            }
        }
    }
}

fn lock_path(record_path: &Path) -> Result<PathBuf> {
    let name = record_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| CueError::general("batch recovery state has an invalid file name"))?;
    Ok(record_path.with_file_name(format!(".{name}.lock")))
}

fn listing_order(listing: &BatchListing) -> (u64, &str) {
    match listing {
        BatchListing::Readable(stored) => (stored.record.created_at_ms, &stored.record.id),
        BatchListing::Unreadable { .. } => (0, ""),
    }
}

fn canonical_cwd(cwd: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(cwd).map_err(|error| {
        CueError::general(format!(
            "could not resolve working directory {} for batch recovery",
            cwd.display()
        ))
        .because(error.to_string())
    })
}

fn scope_key(cwd: &Path) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in cwd.as_os_str().to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("cwd-{hash:016x}")
}

fn unix_time_ms() -> Result<u64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            CueError::general("system clock is before the Unix epoch").because(error.to_string())
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| CueError::general("system clock value is too large"))
}

fn new_batch_id(now_ms: u64) -> String {
    let sequence = BATCH_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "batch-{now_ms:016x}-{:08x}-{sequence:08x}",
        std::process::id()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cue_core::PipelineStage;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

    fn publish_attempt_receipt(item: &BatchItem, batch_id: &str, attempt_number: u32) {
        use crate::run_contract::{
            BatchAttemptRef, ProcessModeName, ProviderIdentity, RemoteDataUsage, RunReceipt,
            StageRecord, StageStatus, TrackedFile,
        };

        std::fs::create_dir_all(&item.workspace).unwrap();
        std::fs::write(&item.source, b"media").unwrap();
        std::fs::write(item.workspace.join("transcript.json"), b"{}\n").unwrap();
        std::fs::write(item.workspace.join("transcript.txt"), b"transcript\n").unwrap();
        let receipt = RunReceipt {
            schema_version: crate::run_contract::SCHEMA_VERSION,
            cue_version: "test".into(),
            mode: ProcessModeName::TranscriptOnly,
            source: TrackedFile::from_path("../0.mp4", &item.source).unwrap(),
            configuration: crate::run_contract::configuration_snapshot(
                &cue_core::Config::default(),
                None,
            ),
            providers: vec![
                ProviderIdentity {
                    stage: PipelineStage::Inspect,
                    provider: "ffprobe".into(),
                    model: None,
                    endpoint: None,
                },
                ProviderIdentity {
                    stage: PipelineStage::Extract,
                    provider: "ffmpeg".into(),
                    model: None,
                    endpoint: None,
                },
                ProviderIdentity {
                    stage: PipelineStage::Transcribe,
                    provider: "faster-whisper".into(),
                    model: Some("test".into()),
                    endpoint: None,
                },
            ],
            stages: [
                PipelineStage::Inspect,
                PipelineStage::Extract,
                PipelineStage::Transcribe,
                PipelineStage::Render,
            ]
            .into_iter()
            .map(|stage| StageRecord::new(stage, StageStatus::Executed, None))
            .collect(),
            warnings: Vec::new(),
            remote_data_usage: RemoteDataUsage {
                normalized_text_sent_to_remote_in_current_run: None,
            },
            corrections: Vec::new(),
            artifacts: Vec::new(),
            published_outputs: Vec::new(),
            batch_attempt: Some(BatchAttemptRef {
                batch_id: batch_id.into(),
                item_position: item.position,
                attempt_number,
            }),
        };
        receipt
            .publish(&item.workspace, &["transcript.json", "transcript.txt"])
            .unwrap();
    }

    fn running_attempt(number: u32) -> Attempt {
        Attempt {
            number,
            started_at_ms: 10,
            finished_at_ms: None,
        }
    }

    fn finished_attempt(number: u32) -> Attempt {
        Attempt {
            number,
            started_at_ms: 10,
            finished_at_ms: Some(20),
        }
    }

    fn test_failure() -> PersistentFailure {
        CueError::new(PipelineStage::Transcribe, "transcription failed")
            .remedy("check the transcription provider")
            .persistent_failure()
    }

    fn test_record(states: Vec<ItemState>) -> BatchRecord {
        BatchRecord {
            schema_version: BATCH_SCHEMA_VERSION,
            id: "batch-test".into(),
            cue_version: "0.13.0".into(),
            created_at_ms: 1,
            updated_at_ms: 2,
            cwd: PathBuf::from("/course"),
            intent: ProcessingIntent {
                mode: RecoveryProcessMode::Full,
                language: Some("en".into()),
                subtitle_formats: vec![SubtitleFormat::Srt],
                summary: true,
                stream: false,
                corrections: Some(PathBuf::from("/course/corrections.md")),
            },
            items: states
                .into_iter()
                .enumerate()
                .map(|(position, state)| BatchItem {
                    position: position as u32,
                    source: PathBuf::from(format!("/course/{position}.mp4")),
                    workspace: PathBuf::from(format!("/course/{position}.cue")),
                    published_base: PathBuf::from(format!("/course/{position}")),
                    state,
                })
                .collect(),
        }
    }

    fn absolutize_items(record: &mut BatchRecord, root: &Path) {
        for item in &mut record.items {
            item.source = root.join(format!("{}.mp4", item.position));
            item.workspace = root.join(format!("{}.cue", item.position));
            item.published_base = root.join(item.position.to_string());
        }
        record.intent.corrections = Some(root.join("corrections.md"));
    }

    #[test]
    fn every_item_state_round_trips_and_aggregate_is_derived() {
        let record = test_record(vec![
            ItemState::Pending,
            ItemState::Running {
                attempt: running_attempt(1),
            },
            ItemState::Complete {
                attempt: finished_attempt(1),
            },
            ItemState::Failed {
                attempt: finished_attempt(2),
                failure: test_failure(),
            },
            ItemState::Missing {
                attempt: finished_attempt(1),
                failure: test_failure(),
            },
            ItemState::NeedsReprocessing {
                attempt: finished_attempt(1),
                failure: test_failure(),
            },
        ]);

        let json = serde_json::to_vec(&record).unwrap();
        let loaded = BatchRecord::from_json(&json).unwrap();
        assert_eq!(loaded, record);
        assert_eq!(
            loaded.counts(),
            BatchCounts {
                complete: 1,
                incomplete: 5
            }
        );
        assert!(!String::from_utf8(json).unwrap().contains("\"aggregate\""));
    }

    #[test]
    fn newest_incomplete_is_scoped_to_the_canonical_working_directory() {
        let temp = tempfile::tempdir().unwrap();
        let course = temp.path().join("course");
        let other = temp.path().join("other");
        std::fs::create_dir_all(&course).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));

        let mut older = test_record(vec![ItemState::Pending]);
        older.id = "older".into();
        older.cwd = std::fs::canonicalize(&course).unwrap();
        older.created_at_ms = 10;
        older.updated_at_ms = 10;
        absolutize_items(&mut older, temp.path());
        store.save(&older).unwrap();

        let mut newer = older.clone();
        newer.id = "newer".into();
        newer.created_at_ms = 20;
        newer.updated_at_ms = 20;
        store.save(&newer).unwrap();

        let mut unrelated = newer.clone();
        unrelated.id = "unrelated".into();
        unrelated.cwd = std::fs::canonicalize(other).unwrap();
        unrelated.created_at_ms = 30;
        unrelated.updated_at_ms = 30;
        store.save(&unrelated).unwrap();

        let selected = store.newest_incomplete(&course).unwrap().unwrap();
        assert_eq!(selected.record.id, "newer");
    }

    #[test]
    fn lock_contention_is_busy_and_free_running_state_is_interrupted() {
        let temp = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));
        let mut record = test_record(vec![ItemState::Running {
            attempt: running_attempt(1),
        }]);
        record.cwd = std::fs::canonicalize(temp.path()).unwrap();
        absolutize_items(&mut record, temp.path());
        let path = store.save(&record).unwrap();
        let stored = store.load_path(&path).unwrap();

        assert_eq!(store.activity(&stored).unwrap(), BatchActivity::Interrupted);
        let lock = match BatchLock::try_acquire(&path).unwrap() {
            LockAttempt::Acquired(lock) => lock,
            LockAttempt::Busy => panic!("first lock should be available"),
        };
        assert!(matches!(
            BatchLock::try_acquire(&path).unwrap(),
            LockAttempt::Busy
        ));
        assert_eq!(store.activity(&stored).unwrap(), BatchActivity::Active);
        drop(lock);
        assert_eq!(store.activity(&stored).unwrap(), BatchActivity::Interrupted);
    }

    #[test]
    fn create_and_lock_publishes_state_with_its_execution_lock_held() {
        let temp = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));
        let mut record = test_record(vec![ItemState::Pending]);
        absolutize_items(&mut record, temp.path());

        let (stored, lock) = store
            .create_and_lock(temp.path(), record.intent, record.items)
            .unwrap();

        assert!(matches!(
            BatchLock::try_acquire(&stored.path).unwrap(),
            LockAttempt::Busy
        ));
        drop(lock);
        assert!(matches!(
            BatchLock::try_acquire(&stored.path).unwrap(),
            LockAttempt::Acquired(_)
        ));
    }

    #[test]
    fn strict_loading_rejects_malformed_unsupported_and_contradictory_records() {
        assert!(BatchRecord::from_json(b"not json").is_err());

        let unsupported = test_record(vec![ItemState::Pending]);
        let mut unsupported = serde_json::to_value(unsupported).unwrap();
        unsupported["schema_version"] = serde_json::json!(BATCH_SCHEMA_VERSION + 1);
        assert!(BatchRecord::from_json(&serde_json::to_vec(&unsupported).unwrap()).is_err());

        let mut duplicate = test_record(vec![ItemState::Pending, ItemState::Pending]);
        duplicate.items[1].position = 0;
        assert!(duplicate.to_json().is_err());

        let mut relative = test_record(vec![ItemState::Pending]);
        relative.items[0].source = PathBuf::from("relative.mp4");
        assert!(relative.to_json().is_err());

        let mut contradictory = test_record(vec![ItemState::Running {
            attempt: running_attempt(1),
        }]);
        let ItemState::Running { attempt } = &mut contradictory.items[0].state else {
            unreachable!()
        };
        attempt.finished_at_ms = Some(20);
        assert!(contradictory.to_json().is_err());
    }

    #[test]
    fn corrupt_neighbor_is_reported_without_hiding_valid_record() {
        let temp = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));
        let mut record = test_record(vec![ItemState::Pending]);
        record.cwd = std::fs::canonicalize(temp.path()).unwrap();
        absolutize_items(&mut record, temp.path());
        let valid_path = store.save(&record).unwrap();
        std::fs::write(valid_path.with_file_name("corrupt.json"), b"{").unwrap();

        let listings = store.list_scope(temp.path()).unwrap();
        assert!(listings.iter().any(
            |entry| matches!(entry, BatchListing::Readable(batch) if batch.record.id == record.id)
        ));
        assert!(listings.iter().any(|entry| matches!(entry, BatchListing::Unreadable { path, reason } if path.ends_with("corrupt.json") && reason.contains("parse"))));
    }

    struct ToggleWriter {
        fail: Arc<AtomicBool>,
    }

    impl JournalWriter for ToggleWriter {
        fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
            if self.fail.load(AtomicOrdering::SeqCst) {
                Err(CueError::general("injected journal publication failure"))
            } else {
                crate::run_contract::write_atomic(path, content)
            }
        }
    }

    #[test]
    fn failed_atomic_update_preserves_the_previous_readable_record() {
        let temp = tempfile::tempdir().unwrap();
        let fail = Arc::new(AtomicBool::new(false));
        let store = RecoveryStore::with_writer(
            temp.path().join("state"),
            Arc::new(ToggleWriter { fail: fail.clone() }),
        );
        let mut record = test_record(vec![ItemState::Pending]);
        record.cwd = std::fs::canonicalize(temp.path()).unwrap();
        absolutize_items(&mut record, temp.path());
        let path = store.save(&record).unwrap();

        fail.store(true, AtomicOrdering::SeqCst);
        let error = store.update(&path, |record| {
            record.items[0].state = ItemState::Running {
                attempt: running_attempt(1),
            };
            Ok(())
        });
        assert!(error.is_err());
        let persisted = store.load_path(&path).unwrap();
        assert!(matches!(
            persisted.record.items[0].state,
            ItemState::Pending
        ));
    }

    #[test]
    fn concurrent_terminal_updates_do_not_overwrite_each_other() {
        let temp = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));
        let mut record = test_record(vec![ItemState::Pending, ItemState::Pending]);
        record.cwd = std::fs::canonicalize(temp.path()).unwrap();
        absolutize_items(&mut record, temp.path());
        let path = store.save(&record).unwrap();

        let handles = (0..2)
            .map(|position| {
                let store = store.clone();
                let path = path.clone();
                std::thread::spawn(move || {
                    store
                        .update(&path, |record| {
                            record.items[position].state = ItemState::Complete {
                                attempt: finished_attempt(1),
                            };
                            Ok(())
                        })
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(store.load_path(&path).unwrap().record.counts().complete, 2);
    }

    #[test]
    fn id_and_explicit_path_lookup_are_strict() {
        let temp = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));
        let mut record = test_record(vec![ItemState::Pending]);
        record.cwd = std::fs::canonicalize(temp.path()).unwrap();
        absolutize_items(&mut record, temp.path());
        let path = store.save(&record).unwrap();

        assert_eq!(store.find_by_id(&record.id).unwrap().unwrap().path, path);
        assert_eq!(store.load_path(&path).unwrap().record.id, record.id);
        assert!(store.find_by_id("../escape").is_err());
        assert!(store.load_path(temp.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn explicit_journal_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));
        let mut record = test_record(vec![ItemState::Pending]);
        record.cwd = std::fs::canonicalize(temp.path()).unwrap();
        absolutize_items(&mut record, temp.path());
        let path = store.save(&record).unwrap();
        let alias = temp.path().join("journal.json");
        symlink(path, &alias).unwrap();

        assert!(store.load_path(&alias).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_share_a_canonical_working_directory_scope() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let course = temp.path().join("course");
        let alias = temp.path().join("course-alias");
        std::fs::create_dir(&course).unwrap();
        symlink(&course, &alias).unwrap();
        let store = RecoveryStore::new(temp.path().join("state"));
        let mut record = test_record(vec![ItemState::Pending]);
        record.cwd = std::fs::canonicalize(&course).unwrap();
        absolutize_items(&mut record, temp.path());
        store.save(&record).unwrap();

        assert_eq!(
            store.newest_incomplete(&alias).unwrap().unwrap().record.id,
            record.id
        );
    }

    #[test]
    fn persisted_item_failure_never_contains_error_cause() {
        let secret = "credential=private and spoken customer text";
        let mut record = test_record(vec![ItemState::Failed {
            attempt: finished_attempt(1),
            failure: CueError::new(PipelineStage::Transcribe, "transcription failed")
                .because(secret)
                .remedy("check provider availability")
                .persistent_failure(),
        }]);
        record.intent.corrections = None;

        let json = String::from_utf8(record.to_json().unwrap()).unwrap();
        assert!(!json.contains(secret), "{json}");
        assert!(json.contains("transcription failed"), "{json}");
        assert!(json.contains("check provider availability"), "{json}");
    }

    #[test]
    fn missing_state_root_has_an_actionable_error() {
        let error = match RecoveryStore::from_state_root(None) {
            Ok(_) => panic!("missing state root should fail"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("could not determine"), "{error}");
        assert!(error.contains("CUE_STATE_DIR"), "{error}");
    }

    #[test]
    fn item_transitions_increment_attempts_and_reject_contradictions() {
        let mut item = test_record(vec![ItemState::Pending]).items.remove(0);
        assert_eq!(item.start(10).unwrap(), 1);
        assert!(item.start(11).is_err());
        item.fail(20, test_failure()).unwrap();
        assert_eq!(item.start(30).unwrap(), 2);
        item.complete(40).unwrap();
        assert!(item.start(50).is_err());
        item.mark_needs_reprocessing(test_failure()).unwrap();
        assert_eq!(item.start(60).unwrap(), 3);
    }

    #[test]
    fn exact_attempt_receipt_closes_the_running_to_complete_crash_gap() {
        let temp = tempfile::tempdir().unwrap();
        let mut record = test_record(vec![ItemState::Running {
            attempt: running_attempt(2),
        }]);
        absolutize_items(&mut record, temp.path());
        publish_attempt_receipt(&record.items[0], &record.id, 2);

        let outcome = record.reconcile_item(0, 30).unwrap();

        assert_eq!(outcome, Reconciliation::ReconciledComplete);
        assert!(matches!(
            &record.items[0].state,
            ItemState::Complete { attempt } if attempt.number == 2 && attempt.finished_at_ms == Some(30)
        ));
    }

    #[test]
    fn complete_item_is_reused_only_with_its_exact_attempt_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let mut record = test_record(vec![ItemState::Complete {
            attempt: finished_attempt(2),
        }]);
        absolutize_items(&mut record, temp.path());
        publish_attempt_receipt(&record.items[0], &record.id, 2);

        assert_eq!(
            record.reconcile_item(0, 30).unwrap(),
            Reconciliation::Unchanged
        );
        assert!(matches!(record.items[0].state, ItemState::Complete { .. }));

        publish_attempt_receipt(&record.items[0], &record.id, 1);
        assert_eq!(
            record.reconcile_item(0, 30).unwrap(),
            Reconciliation::NeedsReprocessing
        );
    }

    #[test]
    fn unmatched_receipt_provenance_cannot_close_a_running_attempt() {
        for (batch_id, attempt_number) in [("another-batch", 2), ("batch-test", 1)] {
            let temp = tempfile::tempdir().unwrap();
            let mut record = test_record(vec![ItemState::Running {
                attempt: running_attempt(2),
            }]);
            absolutize_items(&mut record, temp.path());
            publish_attempt_receipt(&record.items[0], batch_id, attempt_number);

            assert_eq!(
                record.reconcile_item(0, 30).unwrap(),
                Reconciliation::NeedsReprocessing
            );
            assert!(matches!(
                record.items[0].state,
                ItemState::NeedsReprocessing { .. }
            ));
        }

        let temp = tempfile::tempdir().unwrap();
        let mut record = test_record(vec![ItemState::Running {
            attempt: running_attempt(2),
        }]);
        absolutize_items(&mut record, temp.path());
        publish_attempt_receipt(&record.items[0], &record.id, 2);
        let receipt_path = record.items[0]
            .workspace
            .join(crate::run_contract::RECEIPT_FILE);
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
        receipt.as_object_mut().unwrap().remove("batch_attempt");
        std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

        assert_eq!(
            record.reconcile_item(0, 30).unwrap(),
            Reconciliation::NeedsReprocessing
        );
    }

    #[test]
    fn changed_complete_output_and_changed_source_require_reprocessing() {
        for changed_path in ["transcript.txt", "../0.mp4"] {
            let temp = tempfile::tempdir().unwrap();
            let mut record = test_record(vec![ItemState::Complete {
                attempt: finished_attempt(1),
            }]);
            absolutize_items(&mut record, temp.path());
            publish_attempt_receipt(&record.items[0], &record.id, 1);
            std::fs::write(record.items[0].workspace.join(changed_path), b"changed").unwrap();

            assert_eq!(
                record.reconcile_item(0, 30).unwrap(),
                Reconciliation::NeedsReprocessing
            );
            assert!(matches!(
                record.items[0].state,
                ItemState::NeedsReprocessing { .. }
            ));
        }
    }

    #[test]
    fn missing_output_or_malformed_receipt_requires_reprocessing() {
        for malformed_receipt in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut record = test_record(vec![ItemState::Complete {
                attempt: finished_attempt(1),
            }]);
            absolutize_items(&mut record, temp.path());
            publish_attempt_receipt(&record.items[0], &record.id, 1);
            if malformed_receipt {
                std::fs::write(
                    record.items[0]
                        .workspace
                        .join(crate::run_contract::RECEIPT_FILE),
                    b"not json\n",
                )
                .unwrap();
            } else {
                std::fs::remove_file(record.items[0].workspace.join("transcript.txt")).unwrap();
            }

            assert_eq!(
                record.reconcile_item(0, 30).unwrap(),
                Reconciliation::NeedsReprocessing
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_tracked_output_requires_reprocessing() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let mut record = test_record(vec![ItemState::Complete {
            attempt: finished_attempt(1),
        }]);
        absolutize_items(&mut record, temp.path());
        publish_attempt_receipt(&record.items[0], &record.id, 1);
        let artifact = record.items[0].workspace.join("transcript.txt");
        let external = temp.path().join("external.txt");
        std::fs::write(&external, b"transcript\n").unwrap();
        std::fs::remove_file(&artifact).unwrap();
        symlink(external, artifact).unwrap();

        assert_eq!(
            record.reconcile_item(0, 30).unwrap(),
            Reconciliation::NeedsReprocessing
        );
    }
}
