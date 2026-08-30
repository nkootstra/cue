use cue_core::Result;

use crate::batch_recovery::{
    BatchActivity, BatchListing, ItemState, RecoveryProcessMode, RecoveryStore,
};
use crate::cli::BatchesCommand;

pub fn run(command: Option<BatchesCommand>) -> Result<i32> {
    match command {
        Some(BatchesCommand::List) => list(),
        Some(BatchesCommand::Show(args)) => show(&args.target),
        None => {
            println!("Usage: cue batches <COMMAND>\n\nCommands:\n  list\n  show <ID-OR-PATH>");
            Ok(0)
        }
    }
}

fn list() -> Result<i32> {
    let store = RecoveryStore::from_environment()?;
    let listings = store.list_scope(&std::env::current_dir()?)?;
    if listings.is_empty() {
        println!("No batches found for the current directory.");
        return Ok(0);
    }
    for listing in listings {
        match listing {
            BatchListing::Readable(stored) => {
                let counts = stored.record.counts();
                println!(
                    "{}  {}  {}/{} complete",
                    stored.record.id,
                    activity_label(store.activity(&stored)?),
                    counts.complete,
                    stored.record.items.len()
                );
            }
            BatchListing::Unreadable { path, reason } => {
                println!(
                    "{}  unreadable  {}",
                    display_path(&path),
                    terminal_text(&reason)
                );
            }
        }
    }
    Ok(0)
}

fn show(target: &std::path::Path) -> Result<i32> {
    let store = RecoveryStore::from_environment()?;
    let stored = crate::commands::resume::select_explicit(&store, target)?;
    let record = &stored.record;
    let activity = store.activity(&stored)?;
    println!("Batch: {}", record.id);
    println!("Status: {}", activity_label(activity));
    match activity {
        BatchActivity::Complete => println!("Next action: none; this batch is complete"),
        BatchActivity::Active => println!("Next action: wait for the active cue process"),
        BatchActivity::Incomplete | BatchActivity::Interrupted => {
            println!("Next action: cue resume {}", display_path(target));
        }
    }
    println!("Recovery state: {}", display_path(&stored.path));
    println!("Working directory: {}", display_path(&record.cwd));
    println!(
        "Mode: {}",
        match record.intent.mode {
            RecoveryProcessMode::Full => "full",
            RecoveryProcessMode::TranscriptOnly => "transcript-only",
        }
    );
    println!(
        "Language: {}",
        terminal_text(record.intent.language.as_deref().unwrap_or("auto"))
    );
    println!(
        "Subtitle formats: {}",
        record
            .intent
            .subtitle_formats
            .iter()
            .map(|format| format.extension())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("Summary: {}", yes_no(record.intent.summary));
    println!("Stream: {}", yes_no(record.intent.stream));
    println!(
        "Corrections: {}",
        record
            .intent
            .corrections
            .as_deref()
            .map_or_else(|| "automatic".to_owned(), display_path)
    );
    println!("Items:");
    let mut items = record.items.iter().collect::<Vec<_>>();
    items.sort_unstable_by_key(|item| item.position);
    for item in items {
        let attempt = item.state.latest_attempt().map_or_else(
            || "not attempted".to_owned(),
            |attempt| format!("attempt {}", attempt.number),
        );
        let verification = if item.state.is_complete() {
            ", verification required before reuse"
        } else {
            ""
        };
        println!(
            "  {}. {} — {}, {}{}",
            item.position + 1,
            display_path(&item.source),
            item_state_label(&item.state),
            attempt,
            verification
        );
        if let Some(failure) = item_failure(&item.state) {
            let stage = failure
                .stage
                .map_or_else(|| "batch".to_owned(), |stage| stage.to_string());
            println!("     {stage}: {}", terminal_text(&failure.summary));
            if let Some(remedy) = &failure.remedy {
                println!("     Remedy: {}", terminal_text(remedy));
            }
        }
    }
    Ok(0)
}

fn item_state_label(state: &ItemState) -> &'static str {
    match state {
        ItemState::Pending => "pending",
        ItemState::Running { .. } => "running",
        ItemState::Complete { .. } => "complete",
        ItemState::Failed { .. } => "failed",
        ItemState::Missing { .. } => "missing",
        ItemState::NeedsReprocessing { .. } => "needs reprocessing",
    }
}

fn item_failure(state: &ItemState) -> Option<&cue_core::error::PersistentFailure> {
    match state {
        ItemState::Failed { failure, .. }
        | ItemState::Missing { failure, .. }
        | ItemState::NeedsReprocessing { failure, .. } => Some(failure),
        ItemState::Pending | ItemState::Running { .. } | ItemState::Complete { .. } => None,
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn activity_label(activity: BatchActivity) -> &'static str {
    match activity {
        BatchActivity::Complete => "complete",
        BatchActivity::Incomplete => "incomplete",
        BatchActivity::Active => "active",
        BatchActivity::Interrupted => "interrupted",
    }
}

fn display_path(path: &std::path::Path) -> String {
    terminal_text(&path.to_string_lossy())
}

fn terminal_text(value: &str) -> String {
    let mut rendered = String::new();
    for character in value.lines().next().unwrap_or(value).chars() {
        if character.is_control() {
            rendered.extend(character.escape_default());
        } else {
            rendered.push(character);
        }
    }
    rendered
}
