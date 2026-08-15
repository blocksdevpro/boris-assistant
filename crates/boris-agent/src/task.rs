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
    "hi ",
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

const LONG_REQUEST_WORDS: usize = 18;
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
    let greeting = GREETING_NEEDLES.iter().any(|n| t.contains(n)) && words <= 8;
    let time_date = TIME_DATE_NEEDLES.iter().any(|n| t.contains(n))
        || (t.contains("time") && words <= 7)
        || (t.contains("date") && words <= 7 && !t.contains("update"));

    let person = PERSON_FIND_NEEDLES.iter().any(|n| t.contains(n));
    let research = person || RESEARCH_NEEDLES.iter().any(|n| t.contains(n));
    let coding = CODING_NEEDLES.iter().any(|n| t.contains(n))
        || (t.contains("code") && !t.contains("zip code"))
        || t.contains("file")
        || t.contains("project");
    let side_effects = SIDE_EFFECT_NEEDLES.iter().any(|n| t.contains(n));
    let multi_step = MULTI_STEP_NEEDLES.iter().any(|n| t.contains(n)) || words > LONG_REQUEST_WORDS;
    let freshness = FRESHNESS_NEEDLES.iter().any(|n| t.contains(n)) || time_date;

    let research_depth = if person {
        ResearchDepth::Deep
    } else if research {
        ResearchDepth::Light
    } else {
        ResearchDepth::None
    };

    let complexity = if words > COMPLEX_WORDS || person || (coding && multi_step) {
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
}

impl RoundTraits {
    pub fn first(task: TaskTraits) -> Self {
        Self {
            task,
            has_tool_results: false,
            has_error_evidence: false,
            tool_rounds: 0,
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
