use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cue_core::{CueError, Result};
use futures::{StreamExt, stream};

use crate::commands::inputs::ResolvedInput;

#[derive(Default)]
pub(super) struct KeyedLocks {
    locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl KeyedLocks {
    pub async fn lock(&self, key: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .locks
            .lock()
            .await
            .entry(key.to_owned())
            .or_default()
            .clone();
        lock.lock_owned().await
    }
}

pub(super) trait MediaProcessor: Sync {
    type Output;

    async fn process(
        &self,
        input: &ResolvedInput,
        batch_attempt: Option<crate::run_contract::BatchAttemptRef>,
    ) -> Result<Self::Output>;
}

/// Adds durable attempt transitions around an existing media processor.
///
/// The existing scheduler remains responsible for concurrency and ordering;
/// this adapter only owns the recovery journal boundary for each item.
pub(super) struct RecoverableProcessor<'a, P> {
    inner: &'a P,
    store: crate::batch_recovery::RecoveryStore,
    journal: PathBuf,
    batch_id: String,
    positions: HashMap<PathBuf, u32>,
    journal_failed: AtomicBool,
    start_gate: std::sync::Mutex<()>,
}

impl<'a, P> RecoverableProcessor<'a, P> {
    pub(super) fn new(
        inner: &'a P,
        store: crate::batch_recovery::RecoveryStore,
        journal: PathBuf,
        batch_id: String,
        positions: HashMap<PathBuf, u32>,
    ) -> Self {
        Self {
            inner,
            store,
            journal,
            batch_id,
            positions,
            journal_failed: AtomicBool::new(false),
            start_gate: std::sync::Mutex::new(()),
        }
    }

    pub(super) fn journal_failed(&self) -> bool {
        self.journal_failed.load(Ordering::Acquire)
    }

    fn stop_after_journal_failure(&self) -> CueError {
        CueError::general(
            "batch recovery state could not be updated; no additional media work will start",
        )
        .remedy("fix the recovery state directory and run cue resume")
    }

    fn update_item<F>(&self, position: u32, update: F) -> Result<()>
    where
        F: FnOnce(&mut crate::batch_recovery::BatchItem) -> Result<()>,
    {
        let result = self.store.update(&self.journal, |record| {
            let item = record
                .items
                .iter_mut()
                .find(|item| item.position == position)
                .ok_or_else(|| {
                    CueError::general(format!("batch item {position} does not exist"))
                })?;
            update(item)
        });
        if result.is_err() {
            self.journal_failed.store(true, Ordering::Release);
        }
        result.map(|_| ())
    }

    fn position(&self, input: &ResolvedInput) -> Result<u32> {
        self.positions.get(&input.source).copied().ok_or_else(|| {
            CueError::general(format!(
                "{} is not part of the recovery batch",
                input.source.display()
            ))
        })
    }
}

impl<P> MediaProcessor for RecoverableProcessor<'_, P>
where
    P: MediaProcessor,
{
    type Output = P::Output;

    async fn process(
        &self,
        input: &ResolvedInput,
        _batch_attempt: Option<crate::run_contract::BatchAttemptRef>,
    ) -> Result<Self::Output> {
        let position = self.position(input)?;
        let started_at_ms = crate::batch_recovery::unix_time_ms()?;
        let mut attempt_number = 0;
        {
            let _start = self
                .start_gate
                .lock()
                .map_err(|_| CueError::general("batch recovery start gate was poisoned"))?;
            if self.journal_failed() {
                return Err(self.stop_after_journal_failure());
            }
            self.update_item(position, |item| {
                attempt_number = item.start(started_at_ms)?;
                Ok(())
            })?;
        }
        let attempt = crate::run_contract::BatchAttemptRef {
            batch_id: self.batch_id.clone(),
            item_position: position,
            attempt_number,
        };

        let (result, unavailable) = match crate::run_contract::is_regular_file(&input.source, true)
        {
            Ok(true) => (self.inner.process(input, Some(attempt)).await, false),
            Ok(false) => (Err(non_file_source_error(&input.source)), false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Err(missing_source_error(&input.source)), true)
            }
            Err(error) => (
                Err(CueError::new(
                    cue_core::PipelineStage::Inspect,
                    format!(
                        "could not inspect original source {}",
                        input.source.display()
                    ),
                )
                .because(error.to_string())),
                false,
            ),
        };

        let finished_at_ms = crate::batch_recovery::unix_time_ms()?;
        match result {
            Ok(value) => {
                self.update_item(position, |item| item.complete(finished_at_ms))?;
                Ok(value)
            }
            Err(error) => {
                let persistent = error.persistent_failure();
                let is_missing = unavailable
                    || std::fs::metadata(&input.source)
                        .is_err_and(|cause| cause.kind() == std::io::ErrorKind::NotFound);
                self.update_item(position, |item| {
                    if is_missing {
                        item.mark_missing(finished_at_ms, persistent)
                    } else {
                        item.fail(finished_at_ms, persistent)
                    }
                })?;
                Err(error)
            }
        }
    }
}

fn missing_source_error(source: &Path) -> CueError {
    CueError::new(
        cue_core::PipelineStage::Inspect,
        format!("original source {} is missing", source.display()),
    )
    .remedy("restore the source at its original path and run cue resume")
}

fn non_file_source_error(source: &Path) -> CueError {
    CueError::new(
        cue_core::PipelineStage::Inspect,
        format!("original source {} is not a regular file", source.display()),
    )
    .remedy("restore the original media file at this path and run cue resume")
}

pub(super) struct BatchFailure<'a> {
    pub input: &'a ResolvedInput,
    pub error: CueError,
}

pub(super) struct BatchSuccess<'a, T> {
    pub input: &'a ResolvedInput,
    pub value: T,
}

pub(super) struct BatchOutcome<'a, T> {
    pub succeeded: usize,
    pub successes: Vec<BatchSuccess<'a, T>>,
    pub failures: Vec<BatchFailure<'a>>,
}

impl<T> BatchOutcome<'_, T> {
    pub fn succeeded(&self) -> usize {
        self.succeeded
    }

    pub fn exit_code(&self) -> i32 {
        i32::from(!self.failures.is_empty())
    }
}

pub(super) async fn process_inputs<'a, P, F>(
    inputs: &'a [ResolvedInput],
    is_batch: bool,
    jobs: NonZeroUsize,
    processor: &P,
    mut on_success: F,
) -> Result<BatchOutcome<'a, P::Output>>
where
    P: MediaProcessor,
    F: FnMut(&ResolvedInput, &P::Output) -> bool,
{
    if !is_batch {
        if let Some(input) = inputs.first() {
            let value = processor.process(input, None).await?;
            let retain = on_success(input, &value);
            return Ok(BatchOutcome {
                succeeded: 1,
                successes: retain
                    .then_some(BatchSuccess { input, value })
                    .into_iter()
                    .collect(),
                failures: Vec::new(),
            });
        }
        return Ok(BatchOutcome {
            succeeded: 0,
            successes: Vec::new(),
            failures: Vec::new(),
        });
    }

    let mut results =
        stream::iter(inputs.iter().enumerate().map(|(index, input)| async move {
            (index, input, processor.process(input, None).await)
        }))
        .buffer_unordered(jobs.get());
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut succeeded = 0;
    while let Some((index, input, result)) = results.next().await {
        match result {
            Ok(value) => {
                succeeded += 1;
                if on_success(input, &value) {
                    successes.push((index, BatchSuccess { input, value }));
                }
            }
            Err(error) => failures.push((index, BatchFailure { input, error })),
        }
    }

    successes.sort_unstable_by_key(|(index, _)| *index);
    failures.sort_unstable_by_key(|(index, _)| *index);

    Ok(BatchOutcome {
        succeeded,
        successes: successes.into_iter().map(|(_, success)| success).collect(),
        failures: failures.into_iter().map(|(_, failure)| failure).collect(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use cue_core::Result;

    use super::*;

    #[derive(Default)]
    struct ConcurrencyProbe {
        active: AtomicUsize,
        maximum: AtomicUsize,
    }

    impl MediaProcessor for ConcurrencyProbe {
        type Output = ();

        async fn process(
            &self,
            _input: &ResolvedInput,
            _batch_attempt: Option<crate::run_contract::BatchAttemptRef>,
        ) -> Result<()> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn resolved(name: &str) -> ResolvedInput {
        ResolvedInput {
            source: name.into(),
            workspace: format!("{name}.cue").into(),
            published_base: name.trim_end_matches(".mp4").into(),
        }
    }

    struct ObservesRunning<'a> {
        store: &'a crate::batch_recovery::RecoveryStore,
        journal: &'a std::path::Path,
    }

    impl MediaProcessor for ObservesRunning<'_> {
        type Output = ();

        async fn process(
            &self,
            _input: &ResolvedInput,
            attempt: Option<crate::run_contract::BatchAttemptRef>,
        ) -> Result<()> {
            let attempt = attempt.expect("recoverable work carries attempt provenance");
            let stored = self.store.load_path(self.journal).unwrap();
            assert_eq!(attempt.batch_id, stored.record.id);
            assert!(matches!(
                stored.record.items[0].state,
                crate::batch_recovery::ItemState::Running { .. }
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn recoverable_processor_persists_running_before_work_and_complete_afterward() {
        use crate::batch_recovery::{
            BatchItem, ItemState, ProcessingIntent, RecoveryProcessMode, RecoveryStore,
        };

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        std::fs::write(&source, b"media").unwrap();
        let input = ResolvedInput {
            source: source.clone(),
            workspace: temp.path().join("lesson.cue"),
            published_base: temp.path().join("lesson"),
        };
        let store = RecoveryStore::new(temp.path().join("state"));
        let stored = store
            .create(
                temp.path(),
                ProcessingIntent {
                    mode: RecoveryProcessMode::TranscriptOnly,
                    language: None,
                    subtitle_formats: Vec::new(),
                    summary: false,
                    stream: false,
                    corrections: None,
                },
                vec![BatchItem {
                    position: 0,
                    source,
                    workspace: input.workspace.clone(),
                    published_base: input.published_base.clone(),
                    state: ItemState::Pending,
                }],
            )
            .unwrap();
        let inner = ObservesRunning {
            store: &store,
            journal: &stored.path,
        };
        let processor = RecoverableProcessor::new(
            &inner,
            store.clone(),
            stored.path.clone(),
            stored.record.id.clone(),
            [(input.source.clone(), 0)].into_iter().collect(),
        );

        processor.process(&input, None).await.unwrap();

        let final_record = store.load_path(&stored.path).unwrap().record;
        assert!(matches!(
            final_record.items[0].state,
            ItemState::Complete { .. }
        ));
    }

    struct CountsWork(AtomicUsize);

    impl MediaProcessor for CountsWork {
        type Output = ();

        async fn process(
            &self,
            _input: &ResolvedInput,
            _batch_attempt: Option<crate::run_contract::BatchAttemptRef>,
        ) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn recoverable_batch(
        temp: &tempfile::TempDir,
        store: &crate::batch_recovery::RecoveryStore,
        inputs: &[ResolvedInput],
    ) -> crate::batch_recovery::StoredBatch {
        use crate::batch_recovery::{BatchItem, ItemState, ProcessingIntent, RecoveryProcessMode};

        store
            .create(
                temp.path(),
                ProcessingIntent {
                    mode: RecoveryProcessMode::TranscriptOnly,
                    language: None,
                    subtitle_formats: Vec::new(),
                    summary: false,
                    stream: false,
                    corrections: None,
                },
                inputs
                    .iter()
                    .enumerate()
                    .map(|(position, input)| BatchItem {
                        position: position as u32,
                        source: input.source.clone(),
                        workspace: input.workspace.clone(),
                        published_base: input.published_base.clone(),
                        state: ItemState::Pending,
                    })
                    .collect(),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn missing_source_is_recorded_without_blocking_other_batch_items() {
        use crate::batch_recovery::ItemState;

        let temp = tempfile::tempdir().unwrap();
        let inputs = [
            ResolvedInput {
                source: temp.path().join("missing.mp4"),
                workspace: temp.path().join("missing.cue"),
                published_base: temp.path().join("missing"),
            },
            ResolvedInput {
                source: temp.path().join("available.mp4"),
                workspace: temp.path().join("available.cue"),
                published_base: temp.path().join("available"),
            },
        ];
        std::fs::write(&inputs[1].source, b"media").unwrap();
        let store = crate::batch_recovery::RecoveryStore::new(temp.path().join("state"));
        let stored = recoverable_batch(&temp, &store, &inputs);
        let inner = CountsWork(AtomicUsize::new(0));
        let processor = RecoverableProcessor::new(
            &inner,
            store.clone(),
            stored.path.clone(),
            stored.record.id.clone(),
            inputs
                .iter()
                .enumerate()
                .map(|(position, input)| (input.source.clone(), position as u32))
                .collect(),
        );

        let outcome = process_inputs(
            &inputs,
            true,
            NonZeroUsize::new(2).unwrap(),
            &processor,
            |_, _| false,
        )
        .await
        .unwrap();

        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.succeeded(), 1);
        assert_eq!(outcome.exit_code(), 1);
        let record = store.load_path(&stored.path).unwrap().record;
        assert!(matches!(record.items[0].state, ItemState::Missing { .. }));
        assert!(matches!(record.items[1].state, ItemState::Complete { .. }));
    }

    #[tokio::test]
    async fn restored_missing_source_retries_with_the_next_attempt() {
        use crate::batch_recovery::ItemState;

        let temp = tempfile::tempdir().unwrap();
        let input = ResolvedInput {
            source: temp.path().join("restored.mp4"),
            workspace: temp.path().join("restored.cue"),
            published_base: temp.path().join("restored"),
        };
        let store = crate::batch_recovery::RecoveryStore::new(temp.path().join("state"));
        let stored = recoverable_batch(&temp, &store, std::slice::from_ref(&input));
        let inner = CountsWork(AtomicUsize::new(0));
        let positions = [(input.source.clone(), 0)].into_iter().collect();
        let first = RecoverableProcessor::new(
            &inner,
            store.clone(),
            stored.path.clone(),
            stored.record.id.clone(),
            positions,
        );
        let _ = first.process(&input, None).await.unwrap_err();
        std::fs::write(&input.source, b"restored media").unwrap();
        let second = RecoverableProcessor::new(
            &inner,
            store.clone(),
            stored.path.clone(),
            stored.record.id.clone(),
            [(input.source.clone(), 0)].into_iter().collect(),
        );

        second.process(&input, None).await.unwrap();

        let record = store.load_path(&stored.path).unwrap().record;
        assert!(matches!(
            &record.items[0].state,
            ItemState::Complete { attempt } if attempt.number == 2
        ));
    }

    struct FailsOnWrite {
        calls: AtomicUsize,
        fail_at: usize,
    }

    impl crate::batch_recovery::JournalWriter for FailsOnWrite {
        fn write(&self, path: &std::path::Path, content: &[u8]) -> Result<()> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_at {
                return Err(cue_core::CueError::general("injected journal failure"));
            }
            crate::run_contract::write_atomic(path, content)
        }
    }

    #[tokio::test]
    async fn terminal_journal_failure_prevents_later_pipeline_starts() {
        use crate::batch_recovery::ItemState;

        let temp = tempfile::tempdir().unwrap();
        let writer = Arc::new(FailsOnWrite {
            calls: AtomicUsize::new(0),
            fail_at: 3,
        });
        let store = crate::batch_recovery::RecoveryStore::with_writer(
            temp.path().join("state"),
            writer.clone(),
        );
        let inputs = [
            ResolvedInput {
                source: temp.path().join("first.mp4"),
                workspace: temp.path().join("first.cue"),
                published_base: temp.path().join("first"),
            },
            ResolvedInput {
                source: temp.path().join("second.mp4"),
                workspace: temp.path().join("second.cue"),
                published_base: temp.path().join("second"),
            },
        ];
        for input in &inputs {
            std::fs::write(&input.source, b"media").unwrap();
        }
        let stored = recoverable_batch(&temp, &store, &inputs);
        let inner = CountsWork(AtomicUsize::new(0));
        let processor = RecoverableProcessor::new(
            &inner,
            store.clone(),
            stored.path.clone(),
            stored.record.id.clone(),
            inputs
                .iter()
                .enumerate()
                .map(|(position, input)| (input.source.clone(), position as u32))
                .collect(),
        );

        let outcome = process_inputs(
            &inputs,
            true,
            NonZeroUsize::new(1).unwrap(),
            &processor,
            |_, _| false,
        )
        .await
        .unwrap();

        assert_eq!(outcome.exit_code(), 1);
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
        assert!(processor.journal_failed());
        assert_eq!(writer.calls.load(Ordering::SeqCst), 3);
        let record = store.load_path(&stored.path).unwrap().record;
        assert!(matches!(record.items[0].state, ItemState::Running { .. }));
        assert!(matches!(record.items[1].state, ItemState::Pending));
    }

    #[tokio::test]
    async fn pre_start_journal_failure_starts_no_media_even_with_parallel_jobs() {
        use crate::batch_recovery::ItemState;

        let temp = tempfile::tempdir().unwrap();
        let writer = Arc::new(FailsOnWrite {
            calls: AtomicUsize::new(0),
            fail_at: 2,
        });
        let store = crate::batch_recovery::RecoveryStore::with_writer(
            temp.path().join("state"),
            writer.clone(),
        );
        let inputs = ["first.mp4", "second.mp4"]
            .into_iter()
            .map(|name| ResolvedInput {
                source: temp.path().join(name),
                workspace: temp.path().join(format!("{name}.cue")),
                published_base: temp.path().join(name.trim_end_matches(".mp4")),
            })
            .collect::<Vec<_>>();
        for input in &inputs {
            std::fs::write(&input.source, b"media").unwrap();
        }
        let stored = recoverable_batch(&temp, &store, &inputs);
        let inner = CountsWork(AtomicUsize::new(0));
        let processor = RecoverableProcessor::new(
            &inner,
            store.clone(),
            stored.path.clone(),
            stored.record.id.clone(),
            inputs
                .iter()
                .enumerate()
                .map(|(position, input)| (input.source.clone(), position as u32))
                .collect(),
        );

        let outcome = process_inputs(
            &inputs,
            true,
            NonZeroUsize::new(2).unwrap(),
            &processor,
            |_, _| false,
        )
        .await
        .unwrap();

        assert_eq!(outcome.exit_code(), 1);
        assert_eq!(inner.0.load(Ordering::SeqCst), 0);
        assert_eq!(writer.calls.load(Ordering::SeqCst), 2);
        let record = store.load_path(&stored.path).unwrap().record;
        assert!(
            record
                .items
                .iter()
                .all(|item| matches!(item.state, ItemState::Pending))
        );
    }

    #[tokio::test]
    async fn batch_never_exceeds_the_requested_concurrency() {
        let inputs = (0..5)
            .map(|index| resolved(&format!("lesson-{index}.mp4")))
            .collect::<Vec<_>>();
        let processor = ConcurrencyProbe::default();

        let outcome = process_inputs(
            &inputs,
            true,
            NonZeroUsize::new(2).unwrap(),
            &processor,
            |_, _| false,
        )
        .await
        .unwrap();

        assert_eq!(processor.maximum.load(Ordering::SeqCst), 2);
        assert_eq!(outcome.succeeded(), 5);
        assert!(outcome.failures.is_empty());
    }

    struct OutOfOrderProcessor;

    impl MediaProcessor for OutOfOrderProcessor {
        type Output = ();

        async fn process(
            &self,
            input: &ResolvedInput,
            _batch_attempt: Option<crate::run_contract::BatchAttemptRef>,
        ) -> Result<()> {
            let name = input.source.to_string_lossy();
            let delay = if name.contains("slow") { 30 } else { 1 };
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if name.contains("fail") {
                Err(cue_core::CueError::general(format!("{name} failed")))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn failures_remain_in_input_order_when_work_finishes_out_of_order() {
        let inputs = [
            resolved("slow-fail.mp4"),
            resolved("fast-ok.mp4"),
            resolved("fast-fail.mp4"),
        ];

        let outcome = process_inputs(
            &inputs,
            true,
            NonZeroUsize::new(3).unwrap(),
            &OutOfOrderProcessor,
            |_, _| false,
        )
        .await
        .unwrap();

        assert_eq!(outcome.succeeded(), 1);
        assert_eq!(
            outcome
                .failures
                .iter()
                .map(|failure| failure.input.source.as_path())
                .collect::<Vec<_>>(),
            [
                std::path::Path::new("slow-fail.mp4"),
                std::path::Path::new("fast-fail.mp4")
            ]
        );
        assert_eq!(outcome.exit_code(), 1);
    }

    #[tokio::test]
    async fn success_callback_observes_completion_order() {
        let inputs = [resolved("slow.mp4"), resolved("fast.mp4")];
        let mut completed = Vec::new();

        let outcome = process_inputs(
            &inputs,
            true,
            NonZeroUsize::new(2).unwrap(),
            &OutOfOrderProcessor,
            |input, _| {
                completed.push(input.source.clone());
                true
            },
        )
        .await
        .unwrap();

        assert_eq!(
            completed,
            [PathBuf::from("fast.mp4"), PathBuf::from("slow.mp4")]
        );
        assert_eq!(
            outcome
                .successes
                .iter()
                .map(|success| success.input.source.clone())
                .collect::<Vec<_>>(),
            [PathBuf::from("slow.mp4"), PathBuf::from("fast.mp4")]
        );
    }

    #[tokio::test]
    async fn equal_content_keys_cannot_enter_cache_work_together() {
        let locks = std::sync::Arc::new(KeyedLocks::default());
        let active = std::sync::Arc::new(AtomicUsize::new(0));
        let maximum = std::sync::Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();

        for _ in 0..2 {
            let locks = locks.clone();
            let active = active.clone();
            let maximum = maximum.clone();
            tasks.push(tokio::spawn(async move {
                let _guard = locks.lock("same-content").await;
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }

        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    struct AlwaysFails;

    impl MediaProcessor for AlwaysFails {
        type Output = ();

        async fn process(
            &self,
            _input: &ResolvedInput,
            _batch_attempt: Option<crate::run_contract::BatchAttemptRef>,
        ) -> Result<()> {
            Err(cue_core::CueError::general("single failed"))
        }
    }

    #[tokio::test]
    async fn single_input_failure_is_returned_directly() {
        let inputs = [resolved("only.mp4")];

        let result = process_inputs(
            &inputs,
            false,
            NonZeroUsize::new(4).unwrap(),
            &AlwaysFails,
            |_, _| false,
        )
        .await;
        let Err(error) = result else {
            panic!("single failure unexpectedly succeeded");
        };

        assert!(error.to_string().contains("single failed"));
    }
}
