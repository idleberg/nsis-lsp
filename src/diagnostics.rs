use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use regex::Regex;
use std::sync::LazyLock;

static WARNING_PATTERN: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"warning: (?<message>.*) \((?<file>.*?):(?<line>\d+)\)").unwrap());

static ERROR_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(
		r#"(?s)(?<message>.*?)\r?\n.*(?:E|e)rror in script:? "(?<file>.*?)" on line (?<line>\d+)"#,
	)
	.unwrap()
});

pub fn parse_warnings(stdout: &str) -> Vec<Diagnostic> {
	stdout
		.lines()
		.filter_map(|line| {
			let caps = WARNING_PATTERN.captures(line)?;
			Some(make_diagnostic(
				&caps["message"],
				caps["line"].parse::<u32>().unwrap_or(1).saturating_sub(1),
				DiagnosticSeverity::WARNING,
			))
		})
		.collect()
}

pub fn parse_error(stderr: &str) -> Option<Diagnostic> {
	let caps = ERROR_PATTERN.captures(stderr)?;
	Some(make_diagnostic(
		caps["message"].trim(),
		caps["line"].parse::<u32>().unwrap_or(1).saturating_sub(1),
		DiagnosticSeverity::ERROR,
	))
}

fn make_diagnostic(message: &str, line: u32, severity: DiagnosticSeverity) -> Diagnostic {
	let pos = Position::new(line, 0);
	Diagnostic {
		range: Range::new(pos, pos),
		severity: Some(severity),
		source: Some("makensis".into()),
		message: message.to_string(),
		..Default::default()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_warning() {
		let stdout = "warning: unknown variable \"NOPE\" (test.nsi:5)";
		let warnings = parse_warnings(stdout);
		assert_eq!(warnings.len(), 1);
		assert_eq!(warnings[0].range.start.line, 4);
		assert_eq!(warnings[0].message, "unknown variable \"NOPE\"");
	}

	#[test]
	fn parses_error() {
		let stderr = "Invalid command: Bogus\nError in script \"test.nsi\" on line 10 -- aborting creation process";
		let err = parse_error(stderr).unwrap();
		assert_eq!(err.range.start.line, 9);
		assert_eq!(err.message, "Invalid command: Bogus");
	}

	#[test]
	fn no_error_on_empty() {
		assert!(parse_error("").is_none());
	}

	#[test]
	fn no_warnings_on_empty() {
		assert!(parse_warnings("").is_empty());
	}
}
