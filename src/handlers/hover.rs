//! What the server has to say about the word under the cursor.

use lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};

use crate::deprecation;
use crate::nsis_data::{self, Known};
use crate::workspace::Workspace;

pub fn handle_hover(workspace: &Workspace, params: HoverParams) -> Option<Hover> {
	let at = params.text_document_position_params;
	let (_, ident) = workspace.identifier_at(&at.text_document.uri, at.position)?;

	hover_for_word(&ident.text).map(|value| Hover {
		contents: HoverContents::Markup(MarkupContent {
			kind: MarkupKind::Markdown,
			value,
		}),
		range: None,
	})
}

fn hover_for_word(word: &str) -> Option<String> {
	let known = nsis_data::lookup(word)?;
	// Headed with the canonical spelling, not whatever case the user typed.
	let title = known.name();

	Some(match &known {
		Known::Command(entry) => {
			let mut content = format!("**{}**\n\n{}", title, entry.description);
			if let Some(params) = &entry.parameters {
				content.push_str(&format!("\n\n**Parameters:**\n```\n{}\n```", params));
			}
			if let Some(example) = &entry.example {
				content.push_str(&format!("\n\n**Example:**\n```nsis\n{}\n```", example));
			}
			content
		}
		Known::Variable { description, .. } | Known::Constant { description, .. } => {
			format!("**{}**\n\n{}", title, description)
		}
		Known::Deprecated(dep) => deprecation::hover(dep),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::{TEST_URI, position_params, workspace_with};

	#[test]
	fn hover_builtin_variable() {
		let hover = hover_for_word("$INSTDIR");
		assert!(hover.is_some());
		assert!(hover.unwrap().contains("installation directory"));
	}

	#[test]
	fn hover_builtin_variable_without_dollar() {
		let hover = hover_for_word("INSTDIR");
		assert!(hover.is_some());
	}

	#[test]
	fn hover_constant() {
		let hover = hover_for_word("MB_OK");
		assert!(hover.is_some());
		assert!(hover.unwrap().contains("OK button"));
	}

	#[test]
	fn hover_deprecated() {
		let hover = hover_for_word("SubSection");
		assert!(hover.is_some());
		assert!(hover.unwrap().contains("deprecated"));
	}

	#[test]
	fn hover_unknown() {
		assert!(hover_for_word("__nonexistent__").is_none());
	}

	// ── handle_hover ──

	fn hover_at(text: &str, line: u32, character: u32) -> Option<Hover> {
		let workspace = workspace_with(&[(TEST_URI, text)]);
		handle_hover(
			&workspace,
			HoverParams {
				text_document_position_params: position_params(TEST_URI, line, character),
				work_done_progress_params: Default::default(),
			},
		)
	}

	#[test]
	fn hover_on_an_instruction() {
		assert!(hover_at("Name \"Installer\"", 0, 2).is_some());
	}

	/// The same word inside a string is the installer's prose, and NSIS's
	/// documentation for it would be nonsense there.
	#[test]
	fn hover_in_a_string_is_none() {
		assert!(hover_at("DetailPrint \"What's your Name\"", 0, 26).is_none());
	}

	/// What does expand inside a string still hovers.
	#[test]
	fn hover_on_a_variable_in_a_string() {
		let hover = hover_at("DetailPrint \"into $INSTDIR\"", 0, 22);
		assert!(hover.is_some());
	}
}
