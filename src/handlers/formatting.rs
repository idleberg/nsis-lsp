//! Whole-document formatting, and what to say when the formatter refuses.
//!
//! Indentation comes from the request's `FormattingOptions` rather than from
//! settings: the editor already knows what the user's tab looks like, and a
//! server-side answer would fight it.

use ardent::{Formatter, FormatterOptions};
use lsp_types::{
	Diagnostic, DiagnosticSeverity, DocumentFormattingParams, Position, Range, TextEdit, Uri,
};

use crate::client::Client;
use crate::settings::LspState;
use crate::workspace::Workspace;

/// The single edit that replaces the document, or the message to show when the
/// text could not be parsed.
pub fn handle_formatting(
	workspace: &Workspace,
	params: DocumentFormattingParams,
	state: &LspState,
) -> Result<Vec<TextEdit>, (Uri, String)> {
	let uri = params.text_document.uri;
	let Some(doc) = workspace.document(&uri) else {
		return Ok(vec![]);
	};
	let text = &doc.text;

	let options = FormatterOptions {
		use_tabs: !params.options.insert_spaces,
		indent_size: params.options.tab_size as usize,
		trim_empty_lines: state.trim_empty_lines,
		end_of_line: state.end_of_line.clone(),
		print_width: state.print_width,
		single_quote: state.single_quote,
		comment_style: state.comment_style,
	};

	let Ok(formatter) = Formatter::new(options) else {
		return Ok(vec![]);
	};

	let formatted = formatter.format(text).map_err(|msg| (uri.clone(), msg))?;

	Ok(vec![TextEdit {
		range: Range::new(Position::new(0, 0), doc.end_position()),
		new_text: formatted,
	}])
}

/// Show a failed format where it happened, so the user is not left hunting for
/// the line the message names.
pub fn publish_format_error(client: &impl Client, uri: Uri, msg: &str) {
	let (line, col) = parse_error_position(msg).unwrap_or((0, 0));
	let pos = Position::new(line, col);
	client.publish_diagnostics(
		uri,
		vec![Diagnostic {
			range: Range::new(pos, pos),
			severity: Some(DiagnosticSeverity::ERROR),
			source: Some("nsis-lsp".into()),
			message: msg.to_string(),
			..Default::default()
		}],
	);
}

fn parse_error_position(msg: &str) -> Option<(u32, u32)> {
	let at_idx = msg.find("at ")?;
	let rest = &msg[at_idx + 3..];
	let colon = rest.find(':')?;
	let line: u32 = rest[..colon].parse().ok()?;
	let after_colon = &rest[colon + 1..];
	let end = after_colon
		.find(|c: char| !c.is_ascii_digit())
		.unwrap_or(after_colon.len());
	let col: u32 = after_colon[..end].parse().ok()?;
	Some((line.saturating_sub(1), col.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
	use lsp_types::{DocumentFormattingParams, FormattingOptions, TextDocumentIdentifier};

	use super::*;
	use crate::testing::{TEST_URI, quiet_state, uri, workspace_with};

	fn formatting_params() -> DocumentFormattingParams {
		DocumentFormattingParams {
			text_document: TextDocumentIdentifier { uri: uri(TEST_URI) },
			options: FormattingOptions {
				tab_size: 2,
				insert_spaces: false,
				..Default::default()
			},
			work_done_progress_params: Default::default(),
		}
	}

	#[test]
	fn formatting_keeps_comment_markers_by_default() {
		let workspace = workspace_with(&[(TEST_URI, "; semi\n# hash\n")]);

		let edits = handle_formatting(&workspace, formatting_params(), &quiet_state()).unwrap();

		assert!(edits[0].new_text.contains("; semi"));
		assert!(edits[0].new_text.contains("# hash"));
	}

	#[test]
	fn formatting_rewrites_comment_markers_when_a_style_is_set() {
		let workspace = workspace_with(&[(TEST_URI, "; semi\n# hash\n")]);
		let mut state = quiet_state();
		state.comment_style = Some(ardent::CommentStyle::Hash);

		let edits = handle_formatting(&workspace, formatting_params(), &state).unwrap();

		assert!(edits[0].new_text.contains("# semi"));
		assert!(!edits[0].new_text.contains("; semi"));
	}

	#[test]
	fn parse_error_position_valid() {
		assert_eq!(
			parse_error_position("something failed at 10:5 blah"),
			Some((9, 4))
		);
	}

	#[test]
	fn parse_error_position_no_at() {
		assert_eq!(parse_error_position("no position here"), None);
	}
}
