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

/// Drops the body (and terminator) of a heredoc that feeds a commit or tag
/// message: `git commit -m "$(cat <<'EOF' ... EOF)"` or `git commit -F - <<'EOF'`.
/// Only a quoted tag makes the body literal; an unquoted one is expanded by the
/// shell, so it stays, as does a heredoc that never reaches its terminator and
/// every other heredoc (`sh <<EOF` runs its body). The flag tells whether no
/// other `<<` is left in the result.
fn drop_message_heredocs(cmd: &str) -> (String, bool) {
    let mut out = String::with_capacity(cmd.len());
    let mut held = String::new();
    let mut clean = true;
    let mut skip: Option<(String, bool)> = None;
    for line in cmd.split_inclusive('\n') {
        if let Some((tag, dash)) = &skip {
            held.push_str(line);
            let text = line.strip_suffix('\n').unwrap_or(line);
            if (if *dash {
                text.trim_start_matches('\t')
            } else {
                text
            }) == tag
            {
                skip = None;
                held.clear();
            }
            continue;
        }
        out.push_str(line);
        skip = message_heredoc_tag(line);
        clean &= skip.is_some() || !line.contains("<<");
    }
    if skip.is_some() {
        out.push_str(&held);
        clean = false;
    }
    (out, clean)
}

/// The terminator of the heredoc a line opens, and whether it ends in tabs
/// (`<<-`), when that heredoc is the message of the commit or tag in the same
/// simple command: `cat <<'TAG'` or `- <<'TAG'` after `git commit|tag`, with a
/// quoted tag (`'TAG'`, `"TAG"`, `\TAG`) and nothing but closing quotes and
/// parentheses after it.
fn message_heredoc_tag(line: &str) -> Option<(String, bool)> {
    if line.matches("<<").count() != 1 {
        return None;
    }
    let at = line.find("<<")?;
    let (before, after) = (&line[..at], &line[at + 2..]);
    let (after, dash) = match after.strip_prefix('-') {
        Some(a) => (a, true),
        None => (after, false),
    };
    let after = after.trim_start_matches([' ', '\t']);
    let word = |s: &str| -> String {
        s.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    };
    let (tag, rest) = if let Some(s) = after.strip_prefix('\\') {
        let tag = word(s);
        let rest = &s[tag.len()..];
        (tag, rest)
    } else {
        let q = after.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
        let s = &after[1..];
        let tag = word(s);
        let rest = s[tag.len()..].strip_prefix(q)?;
        (tag, rest)
    };
    if tag.is_empty()
        || !rest
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | '\'' | '"' | ')' | '\r' | '\n'))
    {
        return None;
    }
    let cmd = before.rsplit([';', '&', '|']).next().unwrap_or(before);
    let words: Vec<&str> = cmd
        .split_whitespace()
        .map(|w| w.trim_matches(SHELL_PUNCT))
        .filter(|w| !w.is_empty())
        .collect();
    (matches!(words.last(), Some(&"cat" | &"-")) && is_commit(&words)).then_some((tag, dash))
}

/// Whether the quotes of a command are all ones `drop_message_quotes` models.
/// `$'..'` strings, comments, heredocs and escaped quotes change what is quoted,
/// so a command with any of them is left to the conservative scan.
fn quotes_modelled(cmd: &str) -> bool {
    if cmd.contains("$'") || cmd.contains("\\'") || cmd.contains("\\\"") {
        return false;
    }
    let mut prev = None;
    for c in cmd.chars() {
        let word_start = prev.is_none_or(|p: char| p.is_whitespace() || ";&|()<>`{}".contains(p));
        if c == '#' && word_start {
            return false;
        }
        prev = Some(c);
    }
    true
}

/// Blanks the quoted strings of `git commit|tag`: a message may mention push
/// and span lines. Single quotes are literal; double quotes only when they hold
/// no `$(` or backtick, which run commands. Quotes of any other command stay,
/// so `bash -c '...'` is still read.
fn drop_message_quotes(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut seg = 0;
    let mut chars = cmd.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' | '{' | '}' => {
                out.push(c);
                seg = out.len();
            }
            '\\' => {
                out.push(c);
                if let Some((_, n)) = chars.next() {
                    out.push(n);
                }
            }
            '\'' | '"' => {
                let mut end = cmd.len();
                let mut closed = false;
                let mut esc = false;
                for (j, d) in cmd[i + 1..].char_indices() {
                    if esc {
                        esc = false;
                    } else if d == '\\' && c == '"' {
                        esc = true;
                    } else if d == c {
                        end = i + 1 + j;
                        closed = true;
                        break;
                    }
                }
                let body = &cmd[i + 1..end];
                let words: Vec<&str> = out[seg..]
                    .split_whitespace()
                    .map(|w| w.trim_matches(SHELL_PUNCT))
                    .filter(|w| !w.is_empty())
                    .collect();
                let message = is_commit(&words)
                    && (c == '\'' || !(body.contains("$(") || body.contains('`')));
                if !message {
                    out.push_str(&cmd[i..if closed { end + 1 } else { end }]);
                }
                let next = if closed { end + 1 } else { end };
                while chars.peek().is_some_and(|(k, _)| *k < next) {
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Whether the simple command is `git commit|tag`: `git` is its command word,
/// after `env`, `command` and `VAR=val` prefixes, so `find -name git -name commit`
/// is not one.
fn is_commit(words: &[&str]) -> bool {
    let first = words
        .iter()
        .position(|w| !(matches!(*w, "env" | "command") || w.contains('=')));
    first.is_some_and(|at| basename(words[at]) == "git")
        && matches!(git_sub(words), Some(("commit" | "tag", _)))
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

/// The branch a `git checkout|switch` creates, if it creates one: the word
/// after `-b`/`-B` (checkout), `-c`/`-C`/`--create`/`--force-create` (switch) or
/// `--orphan`, glued (`-bmain`, `--create=main`) or apart. `Some(None)` is a
/// create flag with no name.
fn created_branch<'a>(sub: &str, args: &[&'a str]) -> Option<Option<&'a str>> {
    let short: &[&str] = if sub == "checkout" {
        &["-b", "-B"]
    } else {
        &["-c", "-C"]
    };
    let long = ["--create", "--force-create", "--orphan"];
    args.iter().enumerate().find_map(|(i, a)| {
        if short.contains(a) || long.contains(a) {
            return Some(args.get(i + 1).copied());
        }
        if let Some(v) = long
            .iter()
            .find_map(|l| a.strip_prefix(l).and_then(|r| r.strip_prefix('=')))
        {
            return Some(Some(v));
        }
        short
            .iter()
            .find_map(|f| a.strip_prefix(f).filter(|v| !v.is_empty()))
            .map(Some)
    })
}

/// Whether `git checkout|switch <args>` moves to the base branch. Restoring
/// files from it (`checkout main -- path`) and branching off it (`-b feat main`)
/// leave the current branch alone; creating a branch named like it
/// (`-B main origin/main` resets it) does not.
fn switches_to_base(sub: &str, args: &[&str], base: &str) -> bool {
    if let Some(created) = created_branch(sub, args) {
        return created.is_some_and(|n| names_base(n, base));
    }
    let (refs, paths) = match args.iter().position(|a| *a == "--") {
        Some(at) => (&args[..at], &args[at + 1..]),
        None => (args, &[][..]),
    };
    paths.is_empty()
        && refs
            .iter()
            .any(|a| !a.starts_with('-') && names_base(a, base))
}

/// Policy guard rails; `Some(reason)` denies the tool call.
pub(super) fn guard(level: Level, base: Option<&str>, tool: &str, input: &str) -> Option<String> {
    if tool != "Bash" {
        return None;
    }
    let (input, clean) = drop_message_heredocs(input);
    let input = if clean && quotes_modelled(&input) {
        drop_message_quotes(&input)
    } else {
        input
    };
    let segs = segments(&input);
    if level == Level::Sub {
        // Conservative: git anywhere before push or send-pack, in any spelling
        // the shell would accept. Only the message of a commit or tag may
        // mention them.
        let words: Vec<&str> = segs
            .iter()
            .flat_map(|w| match is_commit(w) {
                true => &w[..=git_pos(w).unwrap_or(0)],
                false => &w[..],
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
