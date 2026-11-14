//! Invocation parsing for `/no-mistake` and `supercharge no-mistake`.

use std::fmt;
use std::path::PathBuf;

use crate::no_mistake::report::StepName;

/// How the gate should print its final report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputKind {
    #[default]
    Plain,
    Json,
}

impl OutputKind {
    pub fn parse(raw: &str) -> Result<Self, ParseError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "plain" | "text" => Ok(Self::Plain),
            "json" => Ok(Self::Json),
            other => Err(ParseError::new(format!(
                "Unknown output format '{other}'. Use plain or json."
            ))),
        }
    }
}

/// Parsed `/no-mistake` or `supercharge no-mistake` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRequest {
    /// User-supplied intent. Empty means validate already-committed work only.
    pub task: String,
    pub skip: Vec<StepName>,
    pub no_publish: bool,
    pub publish: bool,
    pub base_branch: Option<String>,
    pub yes: bool,
    pub output: OutputKind,
    pub intent_file: Option<PathBuf>,
}

impl Default for GateRequest {
    fn default() -> Self {
        Self {
            task: String::new(),
            skip: Vec::new(),
            no_publish: false,
            publish: false,
            base_branch: None,
            yes: false,
            output: OutputKind::Plain,
            intent_file: None,
        }
    }
}

impl GateRequest {
    pub fn intent_text(&self) -> Result<String, ParseError> {
        if let Some(path) = &self.intent_file {
            let body = std::fs::read_to_string(path).map_err(|e| {
                ParseError::new(format!(
                    "Could not read intent file {}: {e}",
                    path.display()
                ))
            })?;
            let text = body.trim().to_string();
            if text.is_empty() {
                return Err(ParseError::new(
                    "The intent file is empty. Pass a task or a non-empty --intent-file.",
                ));
            }
            if !self.task.is_empty() {
                return Err(ParseError::new(
                    "Pass either a task or --intent-file, not both.",
                ));
            }
            return Ok(text);
        }
        Ok(self.task.trim().to_string())
    }

    pub fn wants_publish(&self) -> bool {
        self.publish && !self.no_publish
    }

    pub fn skipped(&self, step: StepName) -> bool {
        self.skip.contains(&step)
    }
}

/// User-facing parse failure. Never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    message: String,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse the argument string that follows `/no-mistake` or the subcommand name.
pub fn parse_invocation(input: &str) -> Result<GateRequest, ParseError> {
    let tokens = tokenize(input)?;
    parse_tokens(&tokens)
}

pub fn parse_tokens(tokens: &[String]) -> Result<GateRequest, ParseError> {
    let mut request = GateRequest::default();
    let mut task_parts: Vec<String> = Vec::new();
    let mut rest_is_task = false;
    let mut i = 0;
    while i < tokens.len() {
        let Some(token) = tokens.get(i).map(String::as_str) else {
            break;
        };
        if rest_is_task || !token.starts_with('-') {
            if let Some(rest) = tokens.get(i..) {
                task_parts.extend(rest.iter().cloned());
            }
            break;
        }
        if token == "--" {
            rest_is_task = true;
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--skip=") {
            request.skip.extend(parse_skip_list(value)?);
        } else if token == "--skip" {
            i += 1;
            let value = tokens
                .get(i)
                .ok_or_else(|| ParseError::new("--skip needs a comma-separated list of steps."))?;
            if value.starts_with('-') {
                return Err(ParseError::new(
                    "--skip needs a comma-separated list of steps.",
                ));
            }
            request.skip.extend(parse_skip_list(value)?);
        } else if token == "--no-publish" {
            request.no_publish = true;
        } else if token == "--publish" {
            request.publish = true;
        } else if let Some(value) = token.strip_prefix("--base-branch=") {
            set_base_branch(&mut request, value)?;
        } else if token == "--base-branch" {
            i += 1;
            let value = tokens
                .get(i)
                .ok_or_else(|| ParseError::new("--base-branch needs a branch name."))?;
            set_base_branch(&mut request, value)?;
        } else if token == "-y" || token == "--yes" {
            request.yes = true;
        } else if let Some(value) = token.strip_prefix("--output-format=") {
            request.output = OutputKind::parse(value)?;
        } else if token == "--output-format" {
            i += 1;
            let value = tokens
                .get(i)
                .ok_or_else(|| ParseError::new("--output-format needs plain or json."))?;
            request.output = OutputKind::parse(value)?;
        } else if let Some(value) = token.strip_prefix("--intent-file=") {
            request.intent_file = Some(PathBuf::from(value));
        } else if token == "--intent-file" {
            i += 1;
            let value = tokens
                .get(i)
                .ok_or_else(|| ParseError::new("--intent-file needs a path."))?;
            request.intent_file = Some(PathBuf::from(value));
        } else {
            return Err(ParseError::new(format!(
                "Unknown flag '{token}'. See `supercharge no-mistake --help`."
            )));
        }
        i += 1;
    }
    if request.publish && request.no_publish {
        return Err(ParseError::new(
            "Pass only one of --publish or --no-publish.",
        ));
    }
    request.task = join_task(&task_parts);
    Ok(request)
}

fn set_base_branch(request: &mut GateRequest, value: &str) -> Result<(), ParseError> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        return Err(ParseError::new("--base-branch needs a branch name."));
    }
    request.base_branch = Some(value.to_string());
    Ok(())
}

fn parse_skip_list(raw: &str) -> Result<Vec<StepName>, ParseError> {
    let mut steps = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let step = StepName::parse(part).ok_or_else(|| {
            ParseError::new(format!(
                "Unknown step '{part}'. Valid steps: {}.",
                StepName::all_csv()
            ))
        })?;
        if !steps.contains(&step) {
            steps.push(step);
        }
    }
    if steps.is_empty() {
        return Err(ParseError::new(
            "--skip needs a comma-separated list of steps.",
        ));
    }
    Ok(steps)
}

fn join_task(parts: &[String]) -> String {
    parts
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn tokenize(input: &str) -> Result<Vec<String>, ParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else if ch == '\\' && q == '"' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if quote.is_some() {
        return Err(ParseError::new(
            "Unclosed quote in the /no-mistake arguments.",
        ));
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_invocation_is_validate_only() {
        let req = parse_invocation("").unwrap();
        assert!(req.task.is_empty());
        assert!(!req.wants_publish());
        assert!(!req.yes);
    }

    #[test]
    fn task_is_preserved() {
        let req = parse_invocation("add a --json flag to status").unwrap();
        assert_eq!(req.task, "add a --json flag to status");
    }

    #[test]
    fn task_after_double_dash_can_look_like_flags() {
        let req = parse_invocation("-- --totally-a-task").unwrap();
        assert_eq!(req.task, "--totally-a-task");
    }

    #[test]
    fn skip_csv_and_equals() {
        let req = parse_invocation("--skip=lint,document").unwrap();
        assert_eq!(req.skip, vec![StepName::Lint, StepName::Document]);
        let req = parse_invocation("--skip lint,ci").unwrap();
        assert_eq!(req.skip, vec![StepName::Lint, StepName::Ci]);
    }

    #[test]
    fn unknown_step_is_rejected() {
        let err = parse_invocation("--skip=foo").unwrap_err();
        assert!(err.message().contains("Unknown step 'foo'"));
        assert!(err.message().contains("intent"));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let err = parse_invocation("--yolo").unwrap_err();
        assert!(err.message().contains("Unknown flag '--yolo'"));
    }

    #[test]
    fn publish_flags_conflict() {
        let err = parse_invocation("--publish --no-publish").unwrap_err();
        assert!(err.message().contains("--publish"));
    }

    #[test]
    fn base_branch_and_yes_and_json() {
        let req =
            parse_invocation("--base-branch main -y --output-format json fix the typo").unwrap();
        assert_eq!(req.base_branch.as_deref(), Some("main"));
        assert!(req.yes);
        assert_eq!(req.output, OutputKind::Json);
        assert_eq!(req.task, "fix the typo");
    }

    #[test]
    fn quoted_task_keeps_spaces() {
        let req = parse_invocation(r#"--skip lint "fix the typo in README""#).unwrap();
        assert_eq!(req.task, "fix the typo in README");
        assert_eq!(req.skip, vec![StepName::Lint]);
    }

    #[test]
    fn skip_without_value_is_rejected() {
        let err = parse_invocation("--skip").unwrap_err();
        assert!(err.message().contains("--skip needs"));
    }

    #[test]
    fn unclosed_quote_is_rejected() {
        let err = parse_invocation("'oops").unwrap_err();
        assert!(err.message().contains("Unclosed quote"));
    }
}
