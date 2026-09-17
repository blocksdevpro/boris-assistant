//! Explicit task / round traits used by routing, listing, and finish gates.
//!
//! Classification is heuristic but structured: callers should gate on these
//! fields (freshness, research depth, side effects, …) rather than raw
//! keyword-count finish rules.

use serde::{Deserialize, Serialize};

/// How much external research this turn appears to need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ResearchDepth {
    None,
    Light,
    Deep,
}

/// Overall complexity used for model tier and reasoning budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TaskComplexity {
    Simple,
    Moderate,
    Complex,
}

/// Structured traits derived from the latest user text (and optional round hints).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskTraits {
    /// Needs current / live information (time, weather, news, "latest").
    pub freshness: bool,
    pub research_depth: ResearchDepth,
    /// Would mutate files, shell, notes, or other external state.
    pub side_effects: bool,
    /// Multi-step plan, playbook, or "then / after that" chore.
    pub multi_step: bool,
    /// Can be answered from local tools / model knowledge only.
    pub local_only: bool,
    /// Coding, debugging, or project/file work.
    pub coding: bool,
    pub complexity: TaskComplexity,
    /// Short greeting / thanks / yes-no with no work.
    pub greeting: bool,
    /// Time or calendar fact.
    pub time_date: bool,
}

impl TaskTraits {
    pub fn simple_local() -> Self {
        Self {
            freshness: false,
            research_depth: ResearchDepth::None,
            side_effects: false,
            multi_step: false,
            local_only: true,
            coding: false,
            complexity: TaskComplexity::Simple,
            greeting: false,
            time_date: false,
        }
    }

    /// True when the strong model (and high reasoning) should be used.
    pub fn needs_strong(self) -> bool {
        self.coding
            || self.side_effects
            || self.multi_step
            || self.research_depth >= ResearchDepth::Light
            || self.complexity >= TaskComplexity::Complex
    }

    /// Greeting / time / short local fact — fast tier even with tools advertised.
    pub fn is_simple_voice(self) -> bool {
        !self.needs_strong()
            && (self.greeting
                || self.time_date
                || (self.local_only && self.complexity == TaskComplexity::Simple))
    }

    /// Strong enough local-workspace signal to finish-gate on under-tooling.
    ///
    /// Broader than [`Self::coding`] (which also matches the word "function").
    pub fn is_workspace_job(self, user_text: &str) -> bool {
        if self.greeting || self.time_date {
            return false;
        }
        if self.research_depth != ResearchDepth::None {
            return false;
        }
        let t = user_text.to_ascii_lowercase();
        self.coding
            && (LOCAL_WORK_NEEDLES.iter().any(|n| t.contains(n))
                || t.contains("file")
                || t.contains("project")
                || t.contains("debug")
                || t.contains("implement")
                || t.contains("refactor")
                || t.contains("compile")
                || t.contains("stack trace")
                || t.contains("codebase")
                || t.contains(".rs")
                || t.contains("src/"))
    }
}

const RESEARCH_NEEDLES: &[&str] = &[
    "research",
    "look up",
    "look for",
    "find out",
    "find my",
    "find me",
    "who is",
    "linkedin",
    "linked in",
    "github",
    "investigate",
    "search the web",
    "search online",
    "search for",
];

const PERSON_FIND_NEEDLES: &[&str] = &[
    "linkedin",
    "linked in",
    "github",
    "profile",
    "who is",
    "find my",
    "find me",
    "find their",
    "my linkedin",
    "my github",
];

const CODING_NEEDLES: &[&str] = &[
    "implement",
    "debug",
    "refactor",
    "write a",
    "compile",
    "stack trace",
    "function",
    "codebase",
];

/// Local workspace work — must not be routed as web research.
///
/// Strong needles always imply local tool work (grep, test runners, file
/// search). Weak needles (`this file`, `my project`, …) only count as coding
/// when paired with a [`CODING_ACTION_VERBS`] verb, so read-only requests
/// like "summarize this file" stay on the fast tier.
const LOCAL_WORK_NEEDLES: &[&str] = &[
    "grep",
    "glob",
    "ripgrep",
    "in src",
    "in the repo",
    "this file",
    "the file",
    "these files",
    "this folder",
    "this directory",
    "this project",
    "my project",
    "list files",
    "find files",
    "search files",
    "search the code",
    "in the code",
    "compile error",
    "run tests",
    "run the test",
    "cargo test",
    "cargo build",
    "npm test",
    "pytest",
];

/// Local-work needles that imply coding on their own (tool verbs, runners).
const LOCAL_WORK_STRONG_NEEDLES: &[&str] = &[
    "grep",
    "glob",
    "ripgrep",
    "in src",
    "in the repo",
    "list files",
    "find files",
    "search files",
    "search the code",
    "in the code",
    "compile error",
    "run tests",
    "run the test",
    "cargo test",
    "cargo build",
    "npm test",
    "pytest",
];

/// Local-work needles that need a coding action verb to count as coding
/// ("summarize this file" is a read; "edit this file" is code work).
const LOCAL_WORK_WEAK_NEEDLES: &[&str] = &[
    "this file",
    "the file",
    "these files",
    "this folder",
    "this directory",
    "this project",
    "my project",
];

/// Action verbs that turn a bare `file` / `project` / `code` mention into a
/// coding request. Matched on word tokens (not substrings) so "prefix" does
/// not match "fix" and "credit" does not match "edit".
const CODING_ACTION_VERBS: &[&str] = &[
    "edit", "edits", "edited", "editing", "write", "writes", "wrote", "writing", "refactor",
    "refactors", "refactored", "refactoring", "create", "creates", "created", "creating",
    "fix", "fixes", "fixed", "fixing", "debug", "debugs", "debugged", "debugging", "review",
    "reviews", "reviewed", "reviewing", "implement", "implements", "implemented",
    "implementing", "compile", "compiles", "compiled", "compiling",
];

/// Extra task verbs that mark a comma-separated clause as another step
/// ("…, then verify it", "…, and send it to me").
const STEP_CLAUSE_VERBS: &[&str] = &[
    "run", "runs", "check", "checks", "verify", "verifies", "update", "updates", "send",
    "sends", "delete", "deletes", "install", "installs", "test", "tests", "build", "builds",
    "summarize", "summarizes",
];

const SIDE_EFFECT_NEEDLES: &[&str] = &[
    "delete",
    "install",
    "run ",
    "bash",
    "write to",
    "save this",
    "create a file",
    "edit the",
    "open the url",
    "send ",
];

const MULTI_STEP_NEEDLES: &[&str] = &[
    "then ",
    "after that",
    "step by step",
    "plan",
    "multi",
    "handle this",
    "take care",
    "get things done",
    "and then",
];

const FRESHNESS_NEEDLES: &[&str] = &[
    "latest",
    "today",
    "right now",
    "current",
    "news",
    "weather",
    "price of",
];

const GREETING_NEEDLES: &[&str] = &[
    "hello",
    "hey",
    "thanks",
    "thank you",
    "good morning",
    "good night",
    "good afternoon",
    "how are you",
    "what's up",
    "whats up",
];

/// Bare "hi" (any casing / trailing punctuation) as whole words.
///
/// `GREETING_NEEDLES` uses substring matching, which works for "hello" but
/// would match "this"/"which" for "hi" — so "hi" is matched on word tokens
/// instead. The input is already lowercased; punctuation is stripped per
/// token, so "hi", "Hi!", "(hi)" all match while "history" does not.
fn has_bare_hi(lower_trimmed: &str) -> bool {
    lower_trimmed.split_whitespace().any(|w| {
        w.trim_matches(|c: char| c.is_ascii_punctuation() || c == '…' || c == '`') == "hi"
    })
}

const TIME_DATE_NEEDLES: &[&str] = &[
    "what time",
    "what's the time",
    "whats the time",
    "the time",
    "what date",
    "what's the date",
    "whats the date",
    "what day",
    "today's date",
    "todays date",
];

/// Word tokens of an already-lowercased string (splits on anything that is
/// not ASCII alphanumeric, so verbs match with word boundaries).
fn word_tokens(t: &str) -> impl Iterator<Item = &str> {
    t.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
}

fn has_coding_verb(t: &str) -> bool {
    word_tokens(t).any(|w| CODING_ACTION_VERBS.contains(&w))
}

/// True when `code` appears within 3 words of a coding action verb
/// ("review this code", "fix the code", "write code to …").
fn code_near_verb(t: &str) -> bool {
    let words: Vec<&str> = word_tokens(t).collect();
    let code_at: Vec<usize> = words
        .iter()
        .enumerate()
        .filter(|(_, w)| **w == "code" || **w == "codes")
        .map(|(i, _)| i)
        .collect();
    if code_at.is_empty() {
        return false;
    }
    words
        .iter()
        .enumerate()
        .filter(|(_, w)| CODING_ACTION_VERBS.contains(w))
        .any(|(vi, _)| code_at.iter().any(|&ci| vi.abs_diff(ci) <= 3))
}

/// Step markers for long requests: sequencing words or a comma-separated
/// clause starting follow-up work ("…, and send it to me").
fn has_step_markers(t: &str) -> bool {
    if t.contains("then") || t.contains("after") || t.contains("step") {
        return true;
    }
    // "first … then" is covered by "then" above; keep the explicit check for
    // readability of the marker set.
    if t.contains("first") && t.contains("then") {
        return true;
    }
    // Comma + verb: a second clause that does more work.
    t.contains(',')
        && t.split(',').skip(1).any(|clause| {
            word_tokens(clause)
                .any(|w| CODING_ACTION_VERBS.contains(&w) || STEP_CLAUSE_VERBS.contains(&w))
        })
}

const LONG_REQUEST_WORDS: usize = 28;
const COMPLEX_WORDS: usize = 28;

/// Classify a user utterance into structured task traits.
pub fn classify_task(user_text: &str) -> TaskTraits {
    let t = user_text.trim().to_ascii_lowercase();
    if t.is_empty() {
        let mut s = TaskTraits::simple_local();
        s.greeting = true;
        return s;
    }

    let words = t.split_whitespace().count();
    let greeting =
        (GREETING_NEEDLES.iter().any(|n| t.contains(n)) || has_bare_hi(&t)) && words <= 8;
    let time_date = TIME_DATE_NEEDLES.iter().any(|n| t.contains(n))
        || (t.contains("time") && words <= 7)
        || (t.contains("date") && words <= 7 && !t.contains("update"));

    let person = PERSON_FIND_NEEDLES.iter().any(|n| t.contains(n));
    let local_strong = LOCAL_WORK_STRONG_NEEDLES.iter().any(|n| t.contains(n))
        || t.contains(".rs")
        || t.contains(".py")
        || t.contains(".ts")
        || t.contains(".js")
        || t.contains("src/");
    // Bare `file` / `project` / `code` mentions are not coding on their own:
    // they need a coding action verb ("edit this file" is code work,
    // "summarize this file" is a read). `code` additionally counts when it
    // sits within 3 words of an action verb.
    let coding_verb = has_coding_verb(&t);
    let has_file_word = t.contains("file") || t.contains("project");
    let has_code_word = t.contains("code") && !t.contains("zip code");
    let coding = local_strong
        || CODING_NEEDLES.iter().any(|n| t.contains(n))
        || (LOCAL_WORK_WEAK_NEEDLES.iter().any(|n| t.contains(n)) && coding_verb)
        || (has_file_word && coding_verb)
        || (has_code_word && (coding_verb || code_near_verb(&t)));
    // "search for TODO in src" is local work, not a web lookup.
    let web_lookup = person || RESEARCH_NEEDLES.iter().any(|n| t.contains(n));
    let research = person || (web_lookup && !coding);
    let side_effects = SIDE_EFFECT_NEEDLES.iter().any(|n| t.contains(n));
    // Length alone never implies a plan: long requests only count as
    // multi-step when sequencing / step markers are present, so a verbose
    // greeting or rambling single question stays on the fast tier.
    let multi_step = MULTI_STEP_NEEDLES.iter().any(|n| t.contains(n))
        || (words > LONG_REQUEST_WORDS && has_step_markers(&t));
    let freshness = FRESHNESS_NEEDLES.iter().any(|n| t.contains(n)) || time_date;

    let research_depth = if person {
        ResearchDepth::Deep
    } else if research {
        ResearchDepth::Light
    } else {
        ResearchDepth::None
    };

    // Length alone never implies complexity either: a long request without
    // step markers is Moderate at most (via research/coding/side-effects if
    // present), so verbose greetings stay fast.
    let complexity = if person || (coding && multi_step) || (words > COMPLEX_WORDS && multi_step)
    {
        TaskComplexity::Complex
    } else if research || coding || side_effects || multi_step {
        TaskComplexity::Moderate
    } else {
        TaskComplexity::Simple
    };

    let local_only = !research && !freshness.intersection_web() && !coding;
    // time/date is local; weather/news is not
    let local_only = local_only || time_date || greeting;
    let local_only = local_only && !research && !(t.contains("weather") || t.contains("news"));

    TaskTraits {
        freshness: freshness && !greeting,
        research_depth,
        side_effects,
        multi_step,
        local_only,
        coding,
        complexity,
        greeting,
        time_date,
    }
}

trait FreshnessExt {
    fn intersection_web(self) -> bool;
}

impl FreshnessExt for bool {
    fn intersection_web(self) -> bool {
        self
    }
}

/// Round-level hints layered on the user-task traits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundTraits {
    pub task: TaskTraits,
    /// Prior tool observations exist in this turn.
    pub has_tool_results: bool,
    /// A prior observation looked like a failure / invalid args.
    pub has_error_evidence: bool,
    /// Tool-call rounds already completed this turn.
    pub tool_rounds: u32,
    /// Largest current-turn tool observation in chars (for stage budgeting:
    /// long results must be synthesized, never squeezed into SimpleVoice).
    pub max_tool_result_chars: usize,
}

impl RoundTraits {
    pub fn first(task: TaskTraits) -> Self {
        Self {
            task,
            has_tool_results: false,
            has_error_evidence: false,
            tool_rounds: 0,
            max_tool_result_chars: 0,
        }
    }

    /// Escalate to strong after failed tools or once a non-simple turn is mid-loop.
    pub fn should_escalate_strong(self) -> bool {
        self.task.needs_strong()
            || self.has_error_evidence
            || (self.has_tool_results && !self.task.is_simple_voice())
    }
}

/// Evidence quality for research finish-gating (not raw search-call counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceCoverage {
    pub search_calls: u32,
    pub fetch_calls: u32,
    /// Observations that look like real hits (non-empty, not an error).
    pub useful_results: u32,
}

impl EvidenceCoverage {
    pub fn from_tools(tools_used: &[String], observations_ok: u32) -> Self {
        let search_calls = tools_used
            .iter()
            .filter(|t| t.as_str() == "web_search")
            .count() as u32;
        let fetch_calls = tools_used
            .iter()
            .filter(|t| t.as_str() == "web_fetch")
            .count() as u32;
        Self {
            search_calls,
            fetch_calls,
            useful_results: observations_ok,
        }
    }

    /// Enough coverage for the given research depth.
    pub fn meets(self, depth: ResearchDepth) -> bool {
        let useful = self.useful_results;
        match depth {
            ResearchDepth::None => true,
            ResearchDepth::Light => {
                useful >= 1 && self.search_calls >= 1 && (self.search_calls + self.fetch_calls) >= 2
            }
            ResearchDepth::Deep => {
                useful >= 2
                    && self.search_calls >= 2
                    && (self.fetch_calls >= 1 || self.search_calls >= 3)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_is_simple_voice() {
        let t = classify_task("hello");
        assert!(t.greeting);
        assert!(t.is_simple_voice());
        assert!(!t.needs_strong());
    }

    #[test]
    fn time_is_simple_local() {
        let t = classify_task("what time is it");
        assert!(t.time_date);
        assert!(t.local_only);
        assert!(t.is_simple_voice());
        assert!(!t.needs_strong());
    }

    #[test]
    fn research_needs_strong() {
        let t = classify_task("research the latest Rust async runtimes");
        assert!(t.needs_strong());
        assert_eq!(t.research_depth, ResearchDepth::Light);
        assert!(t.freshness);
    }

    #[test]
    fn person_find_is_deep() {
        let t = classify_task("find my linkedin profile Uttam");
        assert_eq!(t.research_depth, ResearchDepth::Deep);
        assert!(t.needs_strong());
    }

    #[test]
    fn coding_needs_strong() {
        let t = classify_task("please debug this");
        assert!(t.coding);
        assert!(t.needs_strong());
    }

    #[test]
    fn find_in_local_file_is_not_web_research() {
        // "find" alone used to trip the research finish-gate.
        let t = classify_task("find the function in src/main.rs");
        assert_eq!(t.research_depth, ResearchDepth::None);
        assert!(t.coding);
    }

    #[test]
    fn search_files_is_local_not_web() {
        let t = classify_task("search files for TODO");
        assert!(t.coding);
        assert_eq!(t.research_depth, ResearchDepth::None);
        let t = classify_task("grep for handle_wake in this project");
        assert!(t.coding);
        assert_eq!(t.research_depth, ResearchDepth::None);
    }

    #[test]
    fn summarize_file_is_not_coding() {
        // Bare file mention without an action verb is a read, not code work.
        let t = classify_task("Summarize this file");
        assert!(!t.coding, "{t:?}");
        assert!(!t.multi_step);
        assert!(!t.needs_strong());
        assert!(t.is_simple_voice());
    }

    #[test]
    fn file_with_action_verb_is_coding() {
        let t = classify_task("please edit this file to fix the bug");
        assert!(t.coding, "{t:?}");
        assert!(t.needs_strong());
        let t = classify_task("please review this code for bugs");
        assert!(t.coding, "{t:?}");
        assert!(t.needs_strong());
        let t = classify_task("write code to parse the config");
        assert!(t.coding, "{t:?}");
        assert!(t.needs_strong());
    }

    #[test]
    fn code_far_from_verb_is_not_coding() {
        // "code" with no action verb anywhere stays fast.
        let t = classify_task("what is the zip code for this area");
        assert!(!t.coding, "{t:?}");
        let t = classify_task("summarize the code style guide");
        assert!(!t.coding, "{t:?}");
        assert!(!t.needs_strong());
    }

    #[test]
    fn bare_hi_variants_are_greetings() {
        for text in ["hi", "Hi", "hi!", "hi.", "(hi)", "hey!", "thanks!"] {
            let t = classify_task(text);
            assert!(t.greeting, "{text:?} -> {t:?}");
            assert!(!t.needs_strong());
        }
    }

    #[test]
    fn hi_substring_is_not_greeting() {
        // Word-token match: "history"/"which" must not trip the "hi" rule.
        assert!(!classify_task("this is about history").greeting);
        assert!(!classify_task("which one is it").greeting);
    }

    #[test]
    fn long_without_markers_is_not_multi_step() {
        // 30 filler words, no sequencing markers: not a plan.
        let long = (0..30)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let t = classify_task(&long);
        assert!(!t.multi_step, "{t:?}");
        assert_eq!(t.complexity, TaskComplexity::Simple);
        assert!(!t.needs_strong());
    }

    #[test]
    fn verbose_greeting_stays_simple() {
        let text = "hello there my friend I hope you are having a wonderful and \
            marvelous day today because I just wanted to stop by and say hello \
            and share how grateful I am for all of your kind and thoughtful help";
        assert!(text.split_whitespace().count() > 28);
        let t = classify_task(text);
        assert!(!t.multi_step, "{t:?}");
        assert!(!t.needs_strong());
        assert_eq!(t.complexity, TaskComplexity::Simple);
    }

    #[test]
    fn long_with_step_markers_is_multi_step() {
        let text = "first collect all of the monthly sales figures from every single \
            regional office report then normalize each of the totals and after that \
            build the summary table step by step for the quarterly review meeting";
        assert!(text.split_whitespace().count() > 28);
        let t = classify_task(text);
        assert!(t.multi_step, "{t:?}");
        assert!(t.needs_strong());
        assert_eq!(t.complexity, TaskComplexity::Complex);
    }

    #[test]
    fn long_with_comma_verb_is_multi_step() {
        let text = "please gather every outstanding invoice from the shared folder since \
            january, including all overdue balances, and send the compiled summary to \
            the finance team for their final approval today please";
        assert!(text.split_whitespace().count() > 28);
        let t = classify_task(text);
        assert!(t.multi_step, "{t:?}");
        assert!(t.needs_strong());
    }

    #[test]
    fn evidence_light_and_deep() {
        let light = EvidenceCoverage {
            search_calls: 2,
            fetch_calls: 0,
            useful_results: 2,
        };
        assert!(light.meets(ResearchDepth::Light));
        assert!(!light.meets(ResearchDepth::Deep));
        let one = EvidenceCoverage {
            search_calls: 1,
            fetch_calls: 0,
            useful_results: 1,
        };
        assert!(!one.meets(ResearchDepth::Light));
        let deep = EvidenceCoverage {
            search_calls: 3,
            fetch_calls: 1,
            useful_results: 2,
        };
        assert!(deep.meets(ResearchDepth::Deep));
        let failed = EvidenceCoverage {
            search_calls: 5,
            fetch_calls: 2,
            useful_results: 0,
        };
        assert!(!failed.meets(ResearchDepth::Light));
        assert!(!failed.meets(ResearchDepth::Deep));
    }
}
