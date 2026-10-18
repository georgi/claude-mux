//! Claude Code hooks (architecture 7.2).

use super::*;
use crate::model::{Activity, Level, Life};
use crate::permissions::find_match;

/// Words of a shell command, split into simple commands at `;`, `&`, `|` and newlines.
fn segments(cmd: &str) -> Vec<Vec<&str>> {
    cmd.split([';', '&', '|', '\n'])
        .map(|s| s.split_whitespace().collect::<Vec<_>>())
        .filter(|w| !w.is_empty())
        .collect()
}

/// For `git [-C dir] <sub> args...`, the subcommand and its arguments.
fn git_sub<'a>(words: &'a [&'a str]) -> Option<(&'a str, &'a [&'a str])> {
    let mut i = words.iter().position(|w| *w == "git")? + 1;
    while i < words.len() {
        match words[i] {
            "-C" | "-c" => i += 2,
            w if w.starts_with('-') => i += 1,
            w => return Some((w, &words[i + 1..])),
        }
    }
    None
}

/// Policy guard rails; `Some(reason)` denies the tool call.
fn guard(level: Level, base: Option<&str>, tool: &str, input: &str) -> Option<String> {
    if tool != "Bash" {
        return None;
    }
    for words in segments(input) {
        let Some((sub, args)) = git_sub(&words) else {
            continue;
        };
        if level == Level::Sub && sub == "push" {
            return Some("sub-agents may not run git push; finish with report_result and let the task agent push".into());
        }
        if level != Level::Executor
            && matches!(sub, "checkout" | "switch")
            && let Some(b) = base.filter(|b| args.iter().any(|a| a.trim_matches(['\'', '"']) == *b))
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
