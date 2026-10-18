//! Claude Code hooks (architecture 7.2).

use super::*;
use crate::model::{Activity, Level, Life};
use crate::permissions::find_match;

/// Characters that wrap or glue commands: subshells, command substitution,
/// groups and quotes. Words are cut of them; they also end a simple command.
const SHELL_PUNCT: [char; 9] = ['(', ')', '`', '{', '}', '\'', '"', '$', '\\'];

/// Words of a shell command, split into simple commands at `;`, `&`, `|`,
/// newlines, subshells and substitutions. Quotes are cut off the words, so the
/// argument of `sh -c '...'` reads like the commands in it.
fn segments(cmd: &str) -> Vec<Vec<&str>> {
    cmd.split([';', '&', '|', '\n', '(', ')', '`', '{', '}'])
        .map(|s| {
            s.split_whitespace()
                .map(|w| w.trim_matches(SHELL_PUNCT))
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// The last path component: `/usr/bin/git` is `git`.
fn basename(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

/// Options of `git` itself that take their value as the next word.
const GIT_VALUE_OPTS: [&str; 8] = [
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--exec-path",
    "--config-env",
    "--super-prefix",
];

/// Where `git` is among the words of a simple command.
fn git_pos(words: &[&str]) -> Option<usize> {
    words.iter().position(|w| basename(w) == "git")
}

/// For `git [-C dir] <sub> args...`, the subcommand and its arguments.
fn git_sub<'a>(words: &'a [&'a str]) -> Option<(&'a str, &'a [&'a str])> {
    let mut i = git_pos(words)? + 1;
    while i < words.len() {
        match words[i] {
            w if GIT_VALUE_OPTS.contains(&w) => i += 2,
            w if w.starts_with('-') => i += 1,
            w => return Some((w, &words[i + 1..])),
        }
    }
    None
}

/// Whether a ref names the base branch: `main`, `origin/main`, `refs/heads/main`,
/// `heads/main`, with or without a `^`/`~` suffix.
fn names_base(arg: &str, base: &str) -> bool {
    let name = arg.split(['^', '~']).next().unwrap_or(arg);
    let name = name.trim_end_matches('@');
    let name = ["refs/remotes/", "remotes/", "refs/heads/", "heads/"]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name);
    name == base || name.strip_prefix("origin/") == Some(base)
}

/// Whether `git checkout|switch <args>` moves to the base branch. Restoring
/// files from it (`checkout main -- path`) and branching off it (`-b feat main`)
/// leave the current branch alone.
fn switches_to_base(sub: &str, args: &[&str], base: &str) -> bool {
    let create: &[&str] = if sub == "checkout" {
        &["-b", "-B"]
    } else {
        &["-c", "-C", "--create", "--force-create"]
    };
    let (refs, paths) = match args.iter().position(|a| *a == "--") {
        Some(at) => (&args[..at], &args[at + 1..]),
        None => (args, &[][..]),
    };
    !args.iter().any(|a| create.contains(a))
        && paths.is_empty()
        && refs
            .iter()
            .any(|a| !a.starts_with('-') && names_base(a, base))
}

/// Policy guard rails; `Some(reason)` denies the tool call.
pub(super) fn guard(level: Level, base: Option<&str>, tool: &str, input: &str) -> Option<String> {
    if tool != "Bash" {
        return None;
    }
    let segs = segments(input);
    if level == Level::Sub {
        // Conservative: git anywhere before push or send-pack, in any spelling
        // the shell would accept. Only the message of a commit or tag may
        // mention them.
        let words: Vec<&str> = segs
            .iter()
            .flat_map(|w| match git_sub(w) {
                Some(("commit" | "tag", _)) => &w[..=git_pos(w).unwrap_or(0)],
                _ => &w[..],
            })
            .copied()
            .collect();
        let pushes = |w: &str| {
            matches!(w, "push" | "send-pack") || matches!(basename(w), "git-push" | "git-send-pack")
        };
        let git = words
            .iter()
            .position(|w| matches!(basename(w), "git" | "git-push" | "git-send-pack"));
        if git.is_some_and(|at| words[at..].iter().any(|w| pushes(w))) {
            return Some("sub-agents may not run git push; finish with report_result and let the task agent push".into());
        }
    }
    if level == Level::Executor {
        return None;
    }
    for words in segs {
        let Some((sub, args)) = git_sub(&words) else {
            continue;
        };
        if matches!(sub, "checkout" | "switch")
            && let Some(b) = base.filter(|b| switches_to_base(sub, args, b))
        {
            return Some(format!(
                "do not check out the base branch {b} in a task worktree; stay on your task branch"
            ));
        }
    }
    None
}

impl Ctx<'_> {
    pub fn hook(&mut self, pi: usize, ai: usize, hook: Hook) -> Result<Reply, EngineError> {
        let id = self.agent(pi, ai).id.clone();
        let reply = match hook {
            Hook::SessionStart { session_id } => {
                let a = self.agent(pi, ai);
                if a.session_id != session_id || a.life != Life::Running {
                    let a = self.agent_mut(pi, ai);
                    a.session_id = session_id;
                    a.life = Life::Running;
                }
                HookReply::None
            }
            Hook::UserPromptSubmit { prompt } => {
                let a = self.agent(pi, ai);
                if a.activity != Activity::Working
                    || a.idle_since.is_some()
                    || a.consecutive_stop_blocks != 0
                {
                    let a = self.agent_mut(pi, ai);
                    a.activity = Activity::Working;
                    a.idle_since = None;
                    a.consecutive_stop_blocks = 0;
                }
                self.confirm_pastes(pi, ai, &prompt);
                self.on_prompt_submit(pi, ai)
            }
            Hook::Stop { stop_hook_active } => {
                self.release_held(&id);
                self.on_stop(pi, ai, stop_hook_active)
            }
            Hook::Notification { kind, .. } => {
                if kind == NotificationKind::PermissionPrompt
                    && !self.has_held(&id)
                    && self.agent(pi, ai).activity != Activity::NeedsPermission
                {
                    self.agent_mut(pi, ai).activity = Activity::NeedsPermission;
                }
                HookReply::None
            }
            Hook::PreToolUse {
                tool, match_input, ..
            } => {
                self.release_held(&id);
                let a = self.agent(pi, ai);
                let base = a
                    .task
                    .and_then(|t| self.plan(pi).task(t))
                    .and_then(|t| self.plan(pi).repos.get(t.repo))
                    .map(|r| r.base_branch.as_str());
                let denied = guard(a.level, base, &tool, &match_input);
                if a.activity != Activity::Working
                    || a.consecutive_stop_blocks != 0
                    || a.idle_since.is_some()
                {
                    let a = self.agent_mut(pi, ai);
                    a.activity = Activity::Working;
                    a.consecutive_stop_blocks = 0;
                    a.idle_since = None;
                }
                match denied {
                    Some(reason) => HookReply::Deny {
                        reason: format!("error: {reason}"),
                    },
                    None => HookReply::None,
                }
            }
            Hook::PermissionRequest {
                request_id,
                tool,
                input_summary,
                match_input,
            } => {
                self.release_held(&id);
                let rules = self
                    .agent(pi, ai)
                    .task
                    .and_then(|t| self.plan(pi).task(t))
                    .map_or(&[][..], |t| &t.allow_rules[..]);
                if find_match(rules, &tool, &match_input).is_some() {
                    return Ok(Reply::Hook(HookReply::Allow));
                }
                let item = self
                    .post(NewItem {
                        to: Recipient::Human,
                        kind: ItemKind::Permission {
                            tool: tool.clone(),
                            input_summary: input_summary.clone(),
                        },
                        from: Some(id),
                        body: format!("{tool}: {input_summary}"),
                        dedupe: None,
                    })
                    .expect("no dedupe key");
                self.st
                    .held_permissions
                    .insert(item, (request_id, match_input));
                if self.agent(pi, ai).activity != Activity::NeedsPermission {
                    self.agent_mut(pi, ai).activity = Activity::NeedsPermission;
                }
                HookReply::Held { item }
            }
            Hook::SessionEnd { clear } => {
                self.release_held(&id);
                if !clear && self.agent(pi, ai).life != Life::Stopped {
                    self.end_agent(
                        pi,
                        ai,
                        Life::Exited { code: None },
                        "exited without a report",
                    );
                }
                HookReply::None
            }
        };
        Ok(Reply::Hook(reply))
    }
}
