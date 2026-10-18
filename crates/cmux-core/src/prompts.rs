//! Prompt templates for plan executors, task agents, sub-agents and the
//! kickoff prompts that start them.
//!
//! Templates use `{{name}}` placeholders. `builtin()` returns the text compiled
//! into the binary. The server reads an override from
//! `~/.config/cmuxd/prompts/<file>` itself and passes that text to `render`.
//! This module does no I/O and depends only on `std`.

/// A prompt template that ships inside the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Executor,
    Task,
    Subagent,
    KickoffPlan,
    KickoffExecute,
    KickoffTask,
    KickoffSubagent,
}

impl Template {
    /// The file name under `prompts/`, and under `~/.config/cmuxd/prompts/` for overrides.
    pub fn file_name(self) -> &'static str {
        match self {
            Template::Executor => "executor.md",
            Template::Task => "task.md",
            Template::Subagent => "subagent.md",
            Template::KickoffPlan => "kickoff-plan.md",
            Template::KickoffExecute => "kickoff-execute.md",
            Template::KickoffTask => "kickoff-task.md",
            Template::KickoffSubagent => "kickoff-subagent.md",
        }
    }

    /// The template text compiled into the binary.
    pub fn builtin(self) -> &'static str {
        match self {
            Template::Executor => include_str!("../prompts/executor.md"),
            Template::Task => include_str!("../prompts/task.md"),
            Template::Subagent => include_str!("../prompts/subagent.md"),
            Template::KickoffPlan => include_str!("../prompts/kickoff-plan.md"),
            Template::KickoffExecute => include_str!("../prompts/kickoff-execute.md"),
            Template::KickoffTask => include_str!("../prompts/kickoff-task.md"),
            Template::KickoffSubagent => include_str!("../prompts/kickoff-subagent.md"),
        }
    }
}

/// A piece of a template: literal text, or the name inside a `{{name}}` placeholder.
#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Var(&'a str),
}

/// Splits a template into text and placeholders in one pass. A `{{` with no
/// closing `}}` is kept as literal text.
fn pieces(template_text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = template_text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                if start > 0 {
                    out.push(Piece::Text(&rest[..start]));
                }
                out.push(Piece::Var(&after[..end]));
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

/// Replaces each `{{name}}` with the value given for `name`.
///
/// Every placeholder must have a value. A placeholder with no value is an
/// error that names it. Values that are not used are ignored. Substitution is
/// a single pass, so a value that itself contains `{{x}}` is not expanded again.
pub fn render(template_text: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    let mut out = String::with_capacity(template_text.len());
    for piece in pieces(template_text) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Var(name) => match vars.iter().find(|(key, _)| *key == name) {
                Some((_, value)) => out.push_str(value),
                None => return Err(format!("no value for placeholder {{{{{name}}}}}")),
            },
        }
    }
    Ok(out)
}

/// The distinct placeholder names in a template, sorted.
pub fn placeholders(template_text: &str) -> Vec<String> {
    let mut names: Vec<String> = pieces(template_text)
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Var(name) => Some(name.to_string()),
            Piece::Text(_) => None,
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// One line of the execute kickoff's task list, for example
/// `- T-0003 [todo] ui deps:[T-0002] criteria:0/3`.
///
/// `deps` is joined with commas, and is `deps:[]` when empty. `criteria` shows
/// `checked/total`.
pub fn task_list_line(
    id: &str,
    state: &str,
    title: &str,
    deps: &[&str],
    checked: usize,
    total: usize,
) -> String {
    format!(
        "- {id} [{state}] {title} deps:[{}] criteria:{checked}/{total}",
        deps.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every builtin template with its full, sorted placeholder list.
    const EXPECTED: [(Template, &[&str]); 7] = [
        (
            Template::Executor,
            &["plan_file", "plan_id", "plan_title", "repos", "task_limit"],
        ),
        (
            Template::Task,
            &[
                "branch",
                "plan_id",
                "subagent_limit",
                "task_id",
                "task_title",
                "worktree",
            ],
        ),
        (Template::Subagent, &["agent_path", "task_id", "worktree"]),
        (Template::KickoffPlan, &["plan_file", "request"]),
        (Template::KickoffExecute, &["task_list"]),
        (
            Template::KickoffTask,
            &["criteria", "instructions", "notes", "task_body"],
        ),
        (Template::KickoffSubagent, &["prompt"]),
    ];

    #[test]
    fn every_builtin_renders_with_its_full_placeholder_set() {
        for (template, names) in EXPECTED {
            let values: Vec<(String, String)> = names
                .iter()
                .map(|name| (name.to_string(), format!("<value of {name}>")))
                .collect();
            let vars: Vec<(&str, &str)> = values
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let text = template.builtin();
            let rendered = render(text, &vars)
                .unwrap_or_else(|e| panic!("{} failed: {e}", template.file_name()));
            assert!(
                !rendered.contains("{{"),
                "{} left a placeholder behind",
                template.file_name()
            );
            for (name, value) in &values {
                assert!(
                    rendered.contains(value.as_str()) || !text.contains(&format!("{{{{{name}}}}}")),
                    "{} did not substitute {name}",
                    template.file_name()
                );
            }
        }
    }

    #[test]
    fn builtin_placeholders_match_the_expected_lists() {
        for (template, names) in EXPECTED {
            assert_eq!(
                placeholders(template.builtin()),
                names.to_vec(),
                "placeholders of {}",
                template.file_name()
            );
        }
    }

    #[test]
    fn file_names_are_distinct_markdown_files() {
        let mut names: Vec<&str> = EXPECTED.iter().map(|(t, _)| t.file_name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 7);
        assert!(names.iter().all(|n| n.ends_with(".md")));
        assert_eq!(Template::KickoffTask.file_name(), "kickoff-task.md");
    }

    #[test]
    fn missing_value_is_an_error_naming_the_placeholder() {
        let err = render("hi {{name}}!", &[]).unwrap_err();
        assert!(err.contains("name"), "error was: {err}");
        assert_eq!(err, "no value for placeholder {{name}}");
    }

    #[test]
    fn unused_values_are_fine() {
        assert_eq!(render("hi", &[("x", "y")]).unwrap(), "hi");
    }

    #[test]
    fn values_are_not_expanded_again() {
        let out = render("{{a}} {{b}}", &[("a", "{{b}}"), ("b", "X")]).unwrap();
        assert_eq!(out, "{{b}} X");
    }

    #[test]
    fn unclosed_braces_are_literal_text() {
        assert_eq!(render("a {{ b", &[]).unwrap(), "a {{ b");
        assert_eq!(placeholders("a {{ b"), Vec::<String>::new());
    }

    #[test]
    fn placeholders_are_sorted_and_deduped() {
        assert_eq!(
            placeholders("{{z}} {{a}} {{z}} text {{m}}"),
            vec!["a", "m", "z"]
        );
        assert!(placeholders("no placeholders").is_empty());
    }

    #[test]
    fn task_list_line_has_the_exact_format() {
        assert_eq!(
            task_list_line("T-0003", "todo", "ui", &["T-0002"], 0, 3),
            "- T-0003 [todo] ui deps:[T-0002] criteria:0/3"
        );
    }

    #[test]
    fn task_list_line_with_no_deps() {
        assert_eq!(
            task_list_line("T-0001", "merged", "model", &[], 2, 2),
            "- T-0001 [merged] model deps:[] criteria:2/2"
        );
    }

    #[test]
    fn task_list_line_with_several_deps() {
        assert_eq!(
            task_list_line(
                "T-0009",
                "blocked",
                "wire it up",
                &["T-0004", "T-0007"],
                1,
                5
            ),
            "- T-0009 [blocked] wire it up deps:[T-0004,T-0007] criteria:1/5"
        );
    }
}
