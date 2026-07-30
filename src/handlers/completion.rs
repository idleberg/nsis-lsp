//! What the server offers at the cursor, and the edit that accepting one makes.
//!
//! The two item lists are built once and cloned per request: they come from
//! tables that never change, and rebuilding a few thousand `CompletionItem`s on
//! every keystroke would be the slowest thing the server does.

use std::sync::LazyLock;

use lsp_types::{
	CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, CompletionTextEdit,
	Position, Range, TextEdit,
};

use crate::context::{self, SyntaxContext};
use crate::nsis_data;
use crate::position::{byte_to_utf16_offset, is_ident_char, line_at, utf16_to_byte_offset};
use crate::workspace::Workspace;

/// Items that are only meaningful in code position: commands, preprocessor
/// keywords, callbacks and bare flag constants.
static CODE_ITEMS: LazyLock<Vec<CompletionItem>> = LazyLock::new(|| {
	let mut items = Vec::new();

	for entry in nsis_data::commands() {
		let kind = if entry.name.starts_with('!') {
			CompletionItemKind::KEYWORD
		} else if entry.name.starts_with('.') || entry.name.starts_with("un.") {
			CompletionItemKind::EVENT
		} else {
			CompletionItemKind::FUNCTION
		};

		items.push(CompletionItem {
			label: entry.name.clone(),
			kind: Some(kind),
			detail: if entry.description.is_empty() {
				None
			} else {
				Some(truncate(&entry.description, 100))
			},
			..Default::default()
		});
	}

	for (name, desc) in nsis_data::constants() {
		items.push(CompletionItem {
			label: name.to_string(),
			kind: Some(CompletionItemKind::CONSTANT),
			detail: Some(desc.to_string()),
			..Default::default()
		});
	}

	items
});

/// Items that interpolate inside a quoted string, and are therefore valid in
/// both string and code position.
static INTERPOLATED_ITEMS: LazyLock<Vec<CompletionItem>> = LazyLock::new(|| {
	nsis_data::variables()
		.map(|(var, desc)| CompletionItem {
			label: var.to_string(),
			kind: Some(CompletionItemKind::VARIABLE),
			detail: Some(desc.to_string()),
			..Default::default()
		})
		.collect()
});

pub fn handle_completion(
	workspace: &Workspace,
	params: CompletionParams,
) -> Option<CompletionResponse> {
	let pos = params.text_document_position.position;
	let doc = workspace.document(&params.text_document_position.text_document.uri)?;

	let mut items = match completion_context(&doc.text, pos) {
		// Nothing is code inside a comment.
		SyntaxContext::Comment => Vec::new(),
		// Only `$VAR`, `${DEFINE}` and `$(LangString)` expand inside a string.
		SyntaxContext::String => INTERPOLATED_ITEMS.clone(),
		SyntaxContext::Code => {
			let mut items = CODE_ITEMS.clone();
			items.extend(INTERPOLATED_ITEMS.iter().cloned());
			items
		}
	};

	// Replace the whole partial token, so `include` becomes `!include` and
	// `INSTDIR` becomes `$INSTDIR` instead of keeping the client's word range,
	// which would drop the sigil or double it up.
	let line_str = line_at(&doc.text, pos.line).unwrap_or("");
	let range = completion_range(line_str, pos);
	for item in &mut items {
		item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
			range,
			new_text: item.label.clone(),
		}));
	}

	Some(CompletionResponse::Array(items))
}

/// Range of the partial token before the cursor, including a leading `!`, `$`
/// or `${` the user already typed.
fn completion_range(line_str: &str, pos: Position) -> Range {
	let col = utf16_to_byte_offset(line_str, pos.character).min(line_str.len());
	let bytes = line_str.as_bytes();

	let mut start = col;
	while start > 0 && is_ident_char(bytes[start - 1]) {
		start -= 1;
	}
	if start > 1 && bytes[start - 1] == b'{' && bytes[start - 2] == b'$' {
		start -= 2;
	} else if start > 0 && (bytes[start - 1] == b'!' || bytes[start - 1] == b'$') {
		start -= 1;
	}

	Range::new(
		Position::new(pos.line, byte_to_utf16_offset(line_str, start)),
		Position::new(pos.line, byte_to_utf16_offset(line_str, col)),
	)
}

fn completion_context(text: &str, pos: Position) -> SyntaxContext {
	let line_str = line_at(text, pos.line).unwrap_or("");
	let col = utf16_to_byte_offset(line_str, pos.character);
	context::context_at(text, pos.line, col)
}

fn truncate(s: &str, max: usize) -> String {
	match s.char_indices().nth(max) {
		Some((idx, _)) => format!("{}…", &s[..idx]),
		None => s.to_string(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::{TEST_URI, position_params, workspace_with};

	// ── truncate ──

	#[test]
	fn truncate_short_string() {
		assert_eq!(truncate("hello", 10), "hello");
	}

	#[test]
	fn truncate_long_string() {
		let result = truncate("hello world", 5);
		assert_eq!(result, "hello…");
	}

	// ── completion_range ──

	fn range_cols(line: &str, character: u32) -> (u32, u32) {
		let range = completion_range(line, Position::new(0, character));
		(range.start.character, range.end.character)
	}

	#[test]
	fn completion_range_bare_word() {
		assert_eq!(range_cols("include", 7), (0, 7));
	}

	#[test]
	fn completion_range_includes_bang() {
		assert_eq!(range_cols("!inc", 4), (0, 4));
	}

	#[test]
	fn completion_range_includes_dollar() {
		assert_eq!(range_cols("StrCpy $INST", 12), (7, 12));
	}

	#[test]
	fn completion_range_includes_dollar_brace() {
		assert_eq!(range_cols("StrCpy $0 ${MY", 14), (10, 14));
	}

	#[test]
	fn completion_range_empty_at_whitespace() {
		assert_eq!(range_cols("Section ", 8), (8, 8));
	}

	// ── handle_completion ──

	fn completion_labels(text: &str, line: u32, character: u32) -> Vec<String> {
		completion_items(text, line, character)
			.into_iter()
			.map(|i| i.label)
			.collect()
	}

	fn completion_items(text: &str, line: u32, character: u32) -> Vec<CompletionItem> {
		let workspace = workspace_with(&[(TEST_URI, text)]);

		let params = CompletionParams {
			text_document_position: position_params(TEST_URI, line, character),
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
			context: None,
		};

		match handle_completion(&workspace, params) {
			Some(CompletionResponse::Array(items)) => items,
			other => panic!("unexpected completion response: {other:?}"),
		}
	}

	/// The edit a client would apply when accepting `label` at this position.
	fn completion_edit(text: &str, line: u32, character: u32, label: &str) -> TextEdit {
		let item = completion_items(text, line, character)
			.into_iter()
			.find(|i| i.label == label)
			.unwrap_or_else(|| panic!("no completion item labelled {label}"));

		match item.text_edit {
			Some(CompletionTextEdit::Edit(edit)) => edit,
			other => panic!("unexpected text edit: {other:?}"),
		}
	}

	#[test]
	fn completion_edit_adds_bang_prefix() {
		let edit = completion_edit("include", 0, 7, "!include");
		assert_eq!(edit.new_text, "!include");
		assert_eq!(
			edit.range,
			Range::new(Position::new(0, 0), Position::new(0, 7))
		);
	}

	#[test]
	fn completion_edit_keeps_typed_bang() {
		let edit = completion_edit("!inc", 0, 4, "!include");
		assert_eq!(edit.new_text, "!include");
		assert_eq!(
			edit.range,
			Range::new(Position::new(0, 0), Position::new(0, 4))
		);
	}

	#[test]
	fn completion_edit_adds_dollar_prefix() {
		let edit = completion_edit("StrCpy $0 INSTDIR", 0, 17, "$INSTDIR");
		assert_eq!(edit.new_text, "$INSTDIR");
		assert_eq!(
			edit.range,
			Range::new(Position::new(0, 10), Position::new(0, 17))
		);
	}

	#[test]
	fn completion_edit_keeps_typed_dollar() {
		let edit = completion_edit("StrCpy $0 $INST", 0, 15, "$INSTDIR");
		assert_eq!(edit.new_text, "$INSTDIR");
		assert_eq!(
			edit.range,
			Range::new(Position::new(0, 10), Position::new(0, 15))
		);
	}

	#[test]
	fn completion_in_code_offers_commands_and_variables() {
		let labels = completion_labels("MessageBox MB_OK \"hi\"\n", 1, 0);
		assert!(labels.iter().any(|l| l == "DetailPrint"));
		assert!(labels.iter().any(|l| l == "!define"));
		assert!(labels.iter().any(|l| l == "MB_OK"));
		assert!(labels.iter().any(|l| l == "$INSTDIR"));
	}

	#[test]
	fn completion_in_string_offers_variables_only() {
		let labels = completion_labels("MessageBox MB_OK \"hello \"", 0, 24);
		assert!(labels.iter().any(|l| l == "$INSTDIR"));
		assert!(!labels.iter().any(|l| l == "DetailPrint"));
		assert!(!labels.iter().any(|l| l == "!define"));
		assert!(!labels.iter().any(|l| l == "MB_OK"));
	}

	#[test]
	fn completion_in_comment_is_empty() {
		assert!(completion_labels("; a comment", 0, 5).is_empty());
		assert!(completion_labels("/* block */", 0, 5).is_empty());
	}

	#[test]
	fn completion_after_string_offers_commands_again() {
		let labels = completion_labels("MessageBox MB_OK \"hi\" ", 0, 22);
		assert!(labels.iter().any(|l| l == "DetailPrint"));
	}

	#[test]
	fn completion_for_unknown_document_is_none() {
		let params = CompletionParams {
			text_document_position: position_params(TEST_URI, 0, 0),
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
			context: None,
		};
		assert!(handle_completion(&Workspace::new(), params).is_none());
	}
}
