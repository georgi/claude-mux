//! Identifier newtypes. Plan, task and agent ids are validated strings on the
//! wire; terminal, item and client ids are plain numbers or strings.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::model::Level;

/// Rejected id text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdError(pub String);

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error: {}", self.0)
    }
}

impl std::error::Error for IdError {}

fn bad<T>(kind: &str, s: &str, want: &str) -> Result<T, IdError> {
    Err(IdError(format!("invalid {kind} {s:?}; expected {want}")))
}

/// Implements `TryFrom<String>` and `From<T> for String` via Display/FromStr,
/// so `#[serde(try_from, into)]` writes the plain string form.
macro_rules! string_serde {
    ($t:ty) => {
        impl TryFrom<String> for $t {
            type Error = IdError;
            fn try_from(s: String) -> Result<Self, IdError> {
                s.parse()
            }
        }
        impl From<$t> for String {
            fn from(v: $t) -> String {
                v.to_string()
            }
        }
    };
}

/// Lowercase ascii slug of at most 40 chars; runs of non-alphanumerics become
/// one `-`, ends are trimmed. Empty result becomes `"plan"`.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(40);
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "plan".into()
    } else {
        out.into()
    }
}

/// `P-YYYYMMDD-slug`, e.g. `P-20261018-auth-refactor`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PlanId(String);

impl PlanId {
    /// Builds an id from a calendar date and a title.
    pub fn new(year: u32, month: u32, day: u32, title: &str) -> Self {
        PlanId(format!("P-{year:04}{month:02}{day:02}-{}", slugify(title)))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PlanId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        let ok = (|| {
            let rest = s.strip_prefix("P-")?;
            let (date, slug) = rest.split_once('-')?;
            let digits = date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit());
            let month: u32 = date.get(4..6)?.parse().ok()?;
            let day: u32 = date.get(6..8)?.parse().ok()?;
            let slug_ok = !slug.is_empty()
                && slug.len() <= 40
                && !slug.starts_with('-')
                && !slug.ends_with('-')
                && !slug.contains("--")
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            (digits && (1..=12).contains(&month) && (1..=31).contains(&day) && slug_ok)
                .then_some(())
        })();
        match ok {
            Some(()) => Ok(PlanId(s.to_string())),
            None => bad("plan id", s, "P-YYYYMMDD-slug"),
        }
    }
}
string_serde!(PlanId);

/// `T-0003`: four or more digits, zero-padded to four.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TaskId(u32);

impl TaskId {
    pub fn new(seq: u32) -> Self {
        TaskId(seq)
    }
    pub fn seq(&self) -> u32 {
        self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "T-{:04}", self.0)
    }
}

impl FromStr for TaskId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        let n = s
            .strip_prefix("T-")
            .filter(|d| d.len() >= 4 && d.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|d| d.parse::<u32>().ok())
            .map(TaskId)
            // Reject non-canonical forms such as "T-00003" so text round-trips.
            .filter(|t| t.to_string() == s);
        match n {
            Some(t) => Ok(t),
            None => bad("task id", s, "T-NNNN"),
        }
    }
}
string_serde!(TaskId);

/// An agent's place in a plan: `executor`, `T-0003` or `T-0003/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum AgentPath {
    Executor,
    Task(TaskId),
    /// Sub-agent name matches `[a-z0-9][a-z0-9-]{0,31}`.
    Sub(TaskId, String),
}

impl AgentPath {
    pub fn sub(task: TaskId, name: &str) -> Result<Self, IdError> {
        if valid_sub_name(name) {
            Ok(AgentPath::Sub(task, name.to_string()))
        } else {
            bad("sub-agent name", name, "[a-z0-9][a-z0-9-]{0,31}")
        }
    }
    pub fn level(&self) -> Level {
        match self {
            AgentPath::Executor => Level::Executor,
            AgentPath::Task(_) => Level::Task,
            AgentPath::Sub(..) => Level::Sub,
        }
    }
    pub fn task_id(&self) -> Option<TaskId> {
        match self {
            AgentPath::Executor => None,
            AgentPath::Task(t) | AgentPath::Sub(t, _) => Some(*t),
        }
    }
    pub fn sub_name(&self) -> Option<&str> {
        match self {
            AgentPath::Sub(_, n) => Some(n),
            _ => None,
        }
    }
    /// The path of the parent agent: a sub's task agent, a task's executor.
    pub fn parent(&self) -> Option<AgentPath> {
        match self {
            AgentPath::Executor => None,
            AgentPath::Task(_) => Some(AgentPath::Executor),
            AgentPath::Sub(t, _) => Some(AgentPath::Task(*t)),
        }
    }
}

fn valid_sub_name(n: &str) -> bool {
    let b = n.as_bytes();
    (1..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

impl fmt::Display for AgentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentPath::Executor => f.write_str("executor"),
            AgentPath::Task(t) => write!(f, "{t}"),
            AgentPath::Sub(t, n) => write!(f, "{t}/{n}"),
        }
    }
}

impl FromStr for AgentPath {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        let parsed = if s == "executor" {
            Some(AgentPath::Executor)
        } else {
            match s.split_once('/') {
                None => s.parse().ok().map(AgentPath::Task),
                Some((t, n)) => t.parse().ok().and_then(|t| AgentPath::sub(t, n).ok()),
            }
        };
        match parsed {
            Some(p) => Ok(p),
            None => bad("agent path", s, "executor | T-NNNN | T-NNNN/<name>"),
        }
    }
}
string_serde!(AgentPath);

/// Globally unique agent id: `<PlanId>/<AgentPath>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentId {
    plan: PlanId,
    path: AgentPath,
}

impl AgentId {
    pub fn new(plan: PlanId, path: AgentPath) -> Self {
        AgentId { plan, path }
    }
    pub fn plan(&self) -> &PlanId {
        &self.plan
    }
    pub fn path(&self) -> &AgentPath {
        &self.path
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.plan, self.path)
    }
}

impl FromStr for AgentId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        let parsed = s.split_once('/').and_then(|(p, rest)| {
            Some(AgentId {
                plan: p.parse().ok()?,
                path: rest.parse().ok()?,
            })
        });
        match parsed {
            Some(a) => Ok(a),
            None => bad("agent id", s, "<plan id>/<agent path>"),
        }
    }
}
string_serde!(AgentId);

/// Terminal id, allocated from `State::next_term_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TermId(pub u32);

impl fmt::Display for TermId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Inbox item id, displayed `I-<n>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(pub u64);

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "I-{}", self.0)
    }
}

impl FromStr for ItemId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        match s
            .strip_prefix("I-")
            .filter(|d| !d.starts_with('+'))
            .and_then(|d| d.parse().ok())
        {
            Some(n) => Ok(ItemId(n)),
            None => bad("item id", s, "I-<n>"),
        }
    }
}

/// Identifies a connected client (TUI, web, CLI).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub String);

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Auth Refactor"), "auth-refactor");
        assert_eq!(slugify("  --Hello,   World!!  "), "hello-world");
        assert_eq!(slugify("Ünï & café"), "n-caf");
        assert_eq!(slugify("***"), "plan");
        assert_eq!(slugify(""), "plan");
        let long = slugify(&"abcdefghi ".repeat(10));
        assert!(long.len() <= 40 && !long.ends_with('-'));
        // Truncation landing on a separator is trimmed.
        assert_eq!(slugify(&format!("{} tail", "a".repeat(39))), "a".repeat(39));
    }

    #[test]
    fn plan_ids() {
        let id = PlanId::new(2026, 10, 18, "Auth Refactor");
        assert_eq!(id.to_string(), "P-20261018-auth-refactor");
        assert_eq!(id.to_string().parse::<PlanId>().unwrap(), id);
        assert_eq!(PlanId::new(2026, 1, 2, "!!").to_string(), "P-20260102-plan");
        for bad in [
            "",
            "P-",
            "P-20261018",
            "P-20261018-",
            "P-2026101-x",
            "P-20261318-x",
            "P-20261032-x",
            "P-20261018-Bad",
            "P-20261018--x",
            "P-20261018-x-",
            "x-20261018-y",
            "P-20261018-a/b",
            "P-20261018-a_b",
        ] {
            assert!(bad.parse::<PlanId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn task_ids() {
        assert_eq!(TaskId::new(3).to_string(), "T-0003");
        assert_eq!(TaskId::new(12345).to_string(), "T-12345");
        assert_eq!("T-0003".parse::<TaskId>().unwrap().seq(), 3);
        assert_eq!("T-12345".parse::<TaskId>().unwrap(), TaskId::new(12345));
        for bad in [
            "T-3", "T-003", "T-00003", "t-0003", "T-", "T-00a3", "T-0003/", "0003", "T--003",
        ] {
            assert!(bad.parse::<TaskId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn agent_paths() {
        let t = TaskId::new(3);
        assert_eq!(
            "executor".parse::<AgentPath>().unwrap(),
            AgentPath::Executor
        );
        let task: AgentPath = "T-0003".parse().unwrap();
        assert_eq!(
            (task.level(), task.task_id(), task.sub_name()),
            (Level::Task, Some(t), None)
        );
        let sub: AgentPath = "T-0003/tests-2".parse().unwrap();
        assert_eq!(sub, AgentPath::Sub(t, "tests-2".into()));
        assert_eq!(
            (sub.level(), sub.task_id(), sub.sub_name()),
            (Level::Sub, Some(t), Some("tests-2"))
        );
        assert_eq!(AgentPath::Executor.level(), Level::Executor);
        assert_eq!(sub.parent(), Some(task.clone()));
        assert_eq!(task.parent(), Some(AgentPath::Executor));
        assert_eq!(AgentPath::Executor.parent(), None);
        for p in ["executor", "T-0003", "T-0003/tests"] {
            assert_eq!(p.parse::<AgentPath>().unwrap().to_string(), p);
        }
        let name33 = format!("T-0003/{}", "a".repeat(33));
        let name32 = format!("T-0003/{}", "a".repeat(32));
        assert!(name32.parse::<AgentPath>().is_ok());
        for bad in [
            "T-3",
            "T-0003/",
            "T-0003/Bad_Name",
            "T-0003/-x",
            "T-0003/a/b",
            "executor/x",
            "Executor",
            "",
            "/",
            "/x",
            "T-0003/a b",
            &name33,
        ] {
            assert!(bad.parse::<AgentPath>().is_err(), "{bad}");
        }
    }

    #[test]
    fn agent_ids() {
        let s = "P-20261018-auth/T-0003/tests";
        let a: AgentId = s.parse().unwrap();
        assert_eq!(a.plan().as_str(), "P-20261018-auth");
        assert_eq!(a.path().to_string(), "T-0003/tests");
        assert_eq!(a.to_string(), s);
        let e: AgentId = "P-20261018-auth/executor".parse().unwrap();
        assert_eq!(e.path(), &AgentPath::Executor);
        assert_eq!(AgentId::new(a.plan().clone(), AgentPath::Executor), e);
        for bad in [
            "executor",
            "P-20261018-auth",
            "P-20261018-auth/",
            "P-20261018-auth/T-3",
            "x/executor",
        ] {
            assert!(bad.parse::<AgentId>().is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_string(&a).unwrap(), format!("\"{s}\""));
        assert_eq!(
            serde_json::from_str::<AgentId>(&format!("\"{s}\"")).unwrap(),
            a
        );
        assert!(serde_json::from_str::<AgentId>("\"nope\"").is_err());
    }

    #[test]
    fn other_ids() {
        assert_eq!(ItemId(42).to_string(), "I-42");
        assert_eq!("I-42".parse::<ItemId>().unwrap(), ItemId(42));
        for bad in ["I-", "42", "I-x", "I--1", "I-+1"] {
            assert!(bad.parse::<ItemId>().is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_string(&TermId(7)).unwrap(), "7");
        assert_eq!(serde_json::to_string(&ItemId(7)).unwrap(), "7");
        assert_eq!(
            serde_json::to_string(&ClientId("web-1".into())).unwrap(),
            "\"web-1\""
        );
        assert_eq!(
            serde_json::to_string(&TaskId::new(3)).unwrap(),
            "\"T-0003\""
        );
        assert!(serde_json::from_str::<TaskId>("\"T-3\"").is_err());
    }
}
