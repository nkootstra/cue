//! `cue skill` — install the transcribe agent skill for AI agents.
//!
//! The skill is distributed through the skills.sh ecosystem; this command
//! proxies the pinned `skills` CLI so any agent harness (Claude Code,
//! opencode, Codex, Cursor, ...) picks it up.

use crate::cli::{SkillArgs, SkillCommand};
use crate::render::println_line;

/// The repository the skill ships in; also the CLI's `--repo` default.
#[cfg_attr(not(test), allow(dead_code))]
pub const DEFAULT_SKILL_REPO: &str = "nkootstra/cue";

/// Pin the upstream installer until global installs stop selecting PromptScript,
/// which has no global skill directory (vercel-labs/skills#1352).
const SKILLS_CLI_PACKAGE: &str = "skills@1.5.9";

/// Build the exact argv that installs the skill for the given options.
///
/// Pure and unit-tested without executing anything. Installs globally unless
/// `local` is set, and always passes `-y` so the skills CLI never blocks on an
/// interactive prompt. `--agent` targets a specific harness; auto-detection is
/// the default. Telemetry opt-out is carried by the `DISABLE_TELEMETRY` env var
/// (the skills CLI has no flag).
pub fn install_argv(repo: &str, agent: Option<&str>, local: bool) -> Vec<String> {
    let mut argv = vec![
        "npx".to_string(),
        "--yes".to_string(),
        SKILLS_CLI_PACKAGE.to_string(),
        "add".to_string(),
        repo.to_string(),
        "-y".to_string(),
    ];
    if !local {
        argv.push("--global".to_string());
    }
    if let Some(agent) = agent {
        argv.push("--agent".to_string());
        argv.push(agent.to_string());
    }
    argv
}

pub async fn run(args: SkillArgs) -> i32 {
    match args.command {
        None => {
            println_line("Manage the transcribe agent skill.");
            println_line("\nUsage:");
            println_line("    cue skill install            Install globally for AI agents");
            println_line("    cue skill install --local    Install in the current project");
            0
        }
        Some(SkillCommand::Install(install)) => {
            run_install(
                &install.repo,
                install.agent.as_deref(),
                install.local,
                install.no_telemetry,
            )
            .await
        }
    }
}

async fn run_install(repo: &str, agent: Option<&str>, local: bool, no_telemetry: bool) -> i32 {
    let argv = install_argv(repo, agent, local);

    let mut command = tokio::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    if no_telemetry {
        command.env("DISABLE_TELEMETRY", "1");
    }

    let scope = if local { "in this project" } else { "globally" };
    println_line(&format!(
        "Installing the transcribe skill {scope} (proxying: {})...",
        argv.join(" ")
    ));

    match command.status().await {
        Ok(status) if status.success() => {
            println_line("\nDone. The `transcribe` skill is available to your AI agents.");
            0
        }
        Ok(status) => {
            eprintln!(
                "the skills installer exited with {} — see output above.",
                status.code().unwrap_or(-1)
            );
            1
        }
        Err(err) => {
            eprintln!("could not run npx: {err}");
            eprintln!("Install Node.js 20+ (e.g. `brew install node`) and retry.");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_default_install_command() {
        assert_eq!(
            install_argv(DEFAULT_SKILL_REPO, None, false),
            vec![
                "npx",
                "--yes",
                "skills@1.5.9",
                "add",
                "nkootstra/cue",
                "-y",
                "--global"
            ]
        );
    }

    #[test]
    fn custom_repo() {
        assert_eq!(
            install_argv("someone/fork", None, false),
            vec![
                "npx",
                "--yes",
                SKILLS_CLI_PACKAGE,
                "add",
                "someone/fork",
                "-y",
                "--global"
            ]
        );
    }

    #[test]
    fn local_install_omits_global_flag() {
        assert_eq!(
            install_argv(DEFAULT_SKILL_REPO, None, true),
            vec![
                "npx",
                "--yes",
                SKILLS_CLI_PACKAGE,
                "add",
                "nkootstra/cue",
                "-y"
            ]
        );
    }

    #[test]
    fn targets_a_specific_agent() {
        assert_eq!(
            install_argv("nkootstra/cue", Some("opencode"), false),
            vec![
                "npx",
                "--yes",
                SKILLS_CLI_PACKAGE,
                "add",
                "nkootstra/cue",
                "-y",
                "--global",
                "--agent",
                "opencode"
            ]
        );
    }
}

/// Validate the bundled skill metadata that the skills CLI depends on.
#[cfg(test)]
mod skill_files_tests {
    const SKILL_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/transcribe");

    fn read(name: &str) -> String {
        std::fs::read_to_string(format!("{SKILL_DIR}/{name}"))
            .unwrap_or_else(|e| panic!("missing skill file {name}: {e}"))
    }

    /// Extract the YAML frontmatter (between the leading `---` markers).
    fn frontmatter(content: &str) -> &str {
        let rest = content
            .strip_prefix("---")
            .expect("SKILL.md must start with ---");
        let end = rest
            .find("\n---")
            .expect("SKILL.md must close frontmatter with ---");
        &rest[..end]
    }

    #[test]
    fn skill_frontmatter_has_name_and_description() {
        let content = read("SKILL.md");
        let fm = frontmatter(&content);
        assert!(fm.contains("name: transcribe"), "frontmatter:\n{fm}");
        assert!(
            fm.contains("description:"),
            "frontmatter must have a description:\n{fm}"
        );
        // Description must be a string, not null/empty.
        let desc = fm
            .split("description:")
            .nth(1)
            .expect("description present");
        assert!(desc.trim().len() > 50, "description too short:\n{fm}");
    }

    #[test]
    fn evals_json_is_valid_and_has_assertions() {
        let json = read("evals/evals.json");
        let evals: serde_json::Value =
            serde_json::from_str(&json).expect("evals.json must be valid JSON");
        assert_eq!(evals["skill_name"], "transcribe");
        let list = evals["evals"].as_array().expect("evals array");
        assert!(list.len() >= 2, "need at least 2 eval cases");
        for case in list {
            assert!(case["id"].is_number(), "case needs numeric id");
            assert!(case["prompt"].as_str().unwrap_or("").len() > 10);
            assert!(
                case["expected_output"].as_str().unwrap_or("").len() > 10,
                "expected_output must be a real description"
            );
            let assertions = match case["assertions"].as_array() {
                Some(list) => list,
                None => {
                    panic!("case {} needs an assertions array", case["id"]);
                }
            };
            assert!(
                !assertions.is_empty(),
                "each case needs at least one assertion"
            );
        }
    }

    #[test]
    fn no_real_identifiers_in_skill() {
        // The skill must stay anonymous: fictional names only, no real
        // people, courses, or paid platforms.
        for name in ["SKILL.md", "references/context-file.md", "evals/evals.json"] {
            let content = read(name).to_lowercase();
            for banned in ["eastham", "dometrain", "nielskootstra"] {
                assert!(
                    !content.contains(banned),
                    "{name} leaks an identifier: {banned}"
                );
            }
        }
    }
}
