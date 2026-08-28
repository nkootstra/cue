use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

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
    async fn process(&self, input: &ResolvedInput) -> Result<()>;
}

pub(super) struct BatchFailure<'a> {
    pub input: &'a ResolvedInput,
    pub error: CueError,
}

pub(super) struct BatchOutcome<'a> {
    pub succeeded: usize,
    pub failures: Vec<BatchFailure<'a>>,
}

impl<'a> BatchOutcome<'a> {
    pub fn exit_code(&self) -> i32 {
        i32::from(!self.failures.is_empty())
    }
}

pub(super) async fn process_inputs<'a, P: MediaProcessor>(
    inputs: &'a [ResolvedInput],
    is_batch: bool,
    jobs: NonZeroUsize,
    processor: &P,
) -> Result<BatchOutcome<'a>> {
    if !is_batch {
        if let Some(input) = inputs.first() {
            processor.process(input).await?;
        }
        return Ok(BatchOutcome {
            succeeded: inputs.len(),
            failures: Vec::new(),
        });
    }

    let mut results = stream::iter(
        inputs
            .iter()
            .enumerate()
            .map(|(index, input)| async move { (index, input, processor.process(input).await) }),
    )
    .buffer_unordered(jobs.get())
    .collect::<Vec<_>>()
    .await;
    results.sort_unstable_by_key(|(index, _, _)| *index);

    let mut succeeded = 0;
    let mut failures = Vec::new();
    for (_, input, result) in results {
        match result {
            Ok(()) => succeeded += 1,
            Err(error) => failures.push(BatchFailure { input, error }),
        }
    }

    Ok(BatchOutcome {
        succeeded,
        failures,
    })
}

#[cfg(test)]
mod tests {
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
        async fn process(&self, _input: &ResolvedInput) -> Result<()> {
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
            output: format!("{name}.cue").into(),
        }
    }

    #[tokio::test]
    async fn batch_never_exceeds_the_requested_concurrency() {
        let inputs = (0..5)
            .map(|index| resolved(&format!("lesson-{index}.mp4")))
            .collect::<Vec<_>>();
        let processor = ConcurrencyProbe::default();

        let outcome = process_inputs(&inputs, true, NonZeroUsize::new(2).unwrap(), &processor)
            .await
            .unwrap();

        assert_eq!(processor.maximum.load(Ordering::SeqCst), 2);
        assert_eq!(outcome.succeeded, 5);
        assert!(outcome.failures.is_empty());
    }

    struct OutOfOrderProcessor;

    impl MediaProcessor for OutOfOrderProcessor {
        async fn process(&self, input: &ResolvedInput) -> Result<()> {
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
        )
        .await
        .unwrap();

        assert_eq!(outcome.succeeded, 1);
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
        async fn process(&self, _input: &ResolvedInput) -> Result<()> {
            Err(cue_core::CueError::general("single failed"))
        }
    }

    #[tokio::test]
    async fn single_input_failure_is_returned_directly() {
        let inputs = [resolved("only.mp4")];

        let result =
            process_inputs(&inputs, false, NonZeroUsize::new(4).unwrap(), &AlwaysFails).await;
        let Err(error) = result else {
            panic!("single failure unexpectedly succeeded");
        };

        assert!(error.to_string().contains("single failed"));
    }
}
