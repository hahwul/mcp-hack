/*!
`subject.rs`

Defines the `Subject` enum used across the CLI subcommands (`list`, `get`, `exec`).
Separated to keep `mod.rs` small and to avoid pulling full command implementation details
where only the subject classification is needed.

Subjects (current semantics):
  - `tools`     : plural – enumerate or show details for all tools
  - `tool`      : singular – show (or interactively select) one tool
  - `resources` : placeholder (future: MCP resources enumeration)
  - `prompts`   : placeholder (future: MCP prompt templates)

Parsing / Display:
  - Implements `clap::ValueEnum` for CLI usage.
  - Implements `Display` for user‑friendly formatting.
  - Provides a custom case‑insensitive `from_str_ci` for internal (non‑clap) parsing.

Extension Guidance:
  - If you later add more subjects, ensure they are reflected in:
       * The `Display` implementation
       * The `variants()` accessor
       * The `from_str_ci` helper
  - Consider a feature gate if certain subjects become optional.

*/

use std::fmt;

/// Enumeration of top-level subjects the user can target with commands.
#[derive(clap::ValueEnum, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Subject {
    /// All tools (plural)
    Tools,
    /// A single tool (singular)
    Tool,
    /// Placeholder for future MCP "resources"
    Resources,
    /// Placeholder for future MCP "prompts"
    Prompts,
}

impl Subject {
    /// Return a static slice of all variants (order matters for help display).
    pub const fn variants() -> &'static [Subject] {
        &[
            Subject::Tools,
            Subject::Tool,
            Subject::Resources,
            Subject::Prompts,
        ]
    }

    /// Case-insensitive parser not relying on `clap`, for internal conversions.
    pub fn from_str_ci(s: &str) -> Option<Self> {
        let norm = s.trim().to_ascii_lowercase();
        match norm.as_str() {
            "tools" => Some(Subject::Tools),
            "tool" => Some(Subject::Tool),
            "resources" => Some(Subject::Resources),
            "prompts" => Some(Subject::Prompts),
            _ => None,
        }
    }

    /// Whether this subject is currently implemented beyond placeholder behavior.
    pub fn is_implemented(&self) -> bool {
        matches!(self, Subject::Tools | Subject::Tool)
    }

    /// Singularity helper: returns true if this is the singular `tool`.
    pub fn is_singular_tool(&self) -> bool {
        matches!(self, Subject::Tool)
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Subject::Tools => "tools",
            Subject::Tool => "tool",
            Subject::Resources => "resources",
            Subject::Prompts => "prompts",
        };
        f.write_str(s)
    }
}

/* --------------------------------- Tests ---------------------------------- */

#[cfg(test)]
mod tests {
    use super::Subject;

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(Subject::from_str_ci("TOOLS"), Some(Subject::Tools));
        assert_eq!(Subject::from_str_ci("tool"), Some(Subject::Tool));
        assert_eq!(
            Subject::from_str_ci(" Resources "),
            Some(Subject::Resources)
        );
        assert_eq!(Subject::from_str_ci("prompts"), Some(Subject::Prompts));
        assert_eq!(Subject::from_str_ci("unknown"), None);
    }

    #[test]
    fn implemented_flags() {
        assert!(Subject::Tools.is_implemented());
        assert!(Subject::Tool.is_implemented());
        assert!(!Subject::Resources.is_implemented());
        assert!(!Subject::Prompts.is_implemented());
    }

    #[test]
    fn singular_helper() {
        assert!(Subject::Tool.is_singular_tool());
        assert!(!Subject::Tools.is_singular_tool());
    }

    #[test]
    fn display_output() {
        assert_eq!(Subject::Tools.to_string(), "tools");
        assert_eq!(Subject::Tool.to_string(), "tool");
    }
}
