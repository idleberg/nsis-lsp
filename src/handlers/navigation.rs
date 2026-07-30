//! Going to a name's declaration, and finding everywhere it is used.
//!
//! Both start the same way — the identifier under the cursor, then the
//! declaration [`Workspace`] resolves for it across every open document — and
//! differ only in what they report back.

use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Location, ReferenceParams};

use crate::workspace::Workspace;

pub fn handle_goto_definition(
	workspace: &Workspace,
	params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
	let at = params.text_document_position_params;
	let (_, ident) = workspace.identifier_at(&at.text_document.uri, at.position)?;
	let def = workspace.definition(&at.text_document.uri, &ident.bare)?;

	Some(GotoDefinitionResponse::Scalar(def.location()))
}

pub fn handle_references(workspace: &Workspace, params: ReferenceParams) -> Option<Vec<Location>> {
	let at = &params.text_document_position;
	let (_, ident) = workspace.identifier_at(&at.text_document.uri, at.position)?;
	let def = workspace.definition(&at.text_document.uri, &ident.bare)?;

	let mut locations = Vec::new();

	if params.context.include_declaration {
		locations.push(def.location());
	}

	locations.extend(workspace.references(&ident.bare, def.symbol.kind));

	if locations.is_empty() {
		None
	} else {
		Some(locations)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::{
		A_URI, B_URI, position_params, reference_params, two_documents, uri, workspace_with,
	};

	fn definition_params(uri_str: &str, line: u32, character: u32) -> GotoDefinitionParams {
		GotoDefinitionParams {
			text_document_position_params: position_params(uri_str, line, character),
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
		}
	}

	/// The declaration is in another document, and the answer names that
	/// document rather than the one the request came from.
	#[test]
	fn goto_definition_crosses_documents() {
		match handle_goto_definition(&two_documents(), definition_params(B_URI, 0, 15)) {
			Some(GotoDefinitionResponse::Scalar(loc)) => {
				assert_eq!(loc.uri, uri(A_URI));
				assert_eq!(loc.range.start.line, 0);
			}
			other => panic!("unexpected definition response: {other:?}"),
		}
	}

	#[test]
	fn goto_definition_answers_with_the_stored_uri() {
		match handle_goto_definition(&two_documents(), definition_params(A_URI, 1, 15)) {
			Some(GotoDefinitionResponse::Scalar(loc)) => {
				assert_eq!(loc.uri, uri(A_URI));
				assert_eq!(loc.range.start.line, 0);
			}
			other => panic!("unexpected definition response: {other:?}"),
		}
	}

	/// The user points at `$myVar`, but the `Var` line declares it bare. Both
	/// spellings have to reach the same declaration.
	#[test]
	fn goto_definition_follows_a_sigil_to_the_bare_declaration() {
		let workspace = workspace_with(&[(A_URI, "Var myVar\nStrCpy $myVar \"x\"")]);

		match handle_goto_definition(&workspace, definition_params(A_URI, 1, 10)) {
			Some(GotoDefinitionResponse::Scalar(loc)) => assert_eq!(loc.range.start.line, 0),
			other => panic!("unexpected definition response: {other:?}"),
		}
	}

	/// A name mentioned in the text of a string is not a use of it, so there is
	/// nowhere to go from there.
	#[test]
	fn goto_definition_from_a_string_is_none() {
		let workspace = workspace_with(&[(A_URI, "Var myVar\nDetailPrint \"about myVar\"")]);
		assert!(handle_goto_definition(&workspace, definition_params(A_URI, 1, 21)).is_none());
	}

	#[test]
	fn references_span_every_open_document() {
		let locations =
			handle_references(&two_documents(), reference_params(A_URI, 0, 10)).unwrap();

		let from_a = locations.iter().filter(|l| l.uri == uri(A_URI)).count();
		let from_b = locations.iter().filter(|l| l.uri == uri(B_URI)).count();
		assert_eq!(from_a, 2); // the declaration plus one deref
		assert_eq!(from_b, 2);
	}

	#[test]
	fn references_for_an_unopened_document_are_none() {
		assert!(
			handle_references(
				&two_documents(),
				reference_params("file:///gone.nsi", 0, 10)
			)
			.is_none()
		);
	}
}
