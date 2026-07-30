//! Renaming a name the user declared, everywhere it appears.
//!
//! `prepareRename` and `rename` have to agree: the first decides whether the
//! client offers a rename at all, so both ask the same two questions — is this
//! NSIS's name rather than the user's, and does anything open declare it?

use lsp_types::{PrepareRenameResponse, RenameParams, TextDocumentPositionParams, WorkspaceEdit};

use crate::nsis_data;
use crate::workspace::Workspace;

pub fn handle_prepare_rename(
	workspace: &Workspace,
	params: TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
	let (_, ident) = workspace.identifier_at(&params.text_document.uri, params.position)?;

	if is_builtin(&ident.text) {
		return None;
	}

	workspace.definition(&params.text_document.uri, &ident.bare)?;

	// The rename covers the bare identifier, not the sigil in front of it —
	// renaming `$myVar` rewrites `myVar` and leaves the `$` where it is.
	Some(PrepareRenameResponse::RangeWithPlaceholder {
		range: ident.bare_range,
		placeholder: ident.bare,
	})
}

pub fn handle_rename(workspace: &Workspace, params: RenameParams) -> Option<WorkspaceEdit> {
	let at = &params.text_document_position;
	let (_, ident) = workspace.identifier_at(&at.text_document.uri, at.position)?;

	if is_builtin(&ident.text) {
		return None;
	}

	let kind = workspace
		.definition(&at.text_document.uri, &ident.bare)?
		.symbol
		.kind;

	Some(WorkspaceEdit {
		changes: Some(workspace.rename_edits(&ident.bare, kind, &params.new_name)),
		..Default::default()
	})
}

/// Whether NSIS already defines `word`, and it is therefore not the user's to
/// rename.
fn is_builtin(word: &str) -> bool {
	nsis_data::lookup(word).is_some()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::{
		A_URI, B_URI, position_params, rename_params, two_documents, uri, workspace_with,
	};

	/// Every open document is edited, each under the `Uri` it was opened with.
	#[test]
	fn rename_edits_every_open_document() {
		let edit = handle_rename(&two_documents(), rename_params(A_URI, 0, 10, "PRODUCT")).unwrap();
		#[allow(clippy::mutable_key_type)]
		let changes = edit.changes.unwrap();

		assert_eq!(changes.len(), 2);
		// One deref plus the `!define` itself.
		assert_eq!(changes[&uri(A_URI)].len(), 2);
		assert_eq!(changes[&uri(B_URI)].len(), 2);
		assert!(changes.values().flatten().all(|e| e.new_text == "PRODUCT"));
	}

	/// A use site is where a rename is usually started from, and the definition
	/// is usually in the header the script includes rather than in the script
	/// itself. Resolving across the open documents makes the two starting points
	/// produce the same edit.
	#[test]
	fn rename_from_the_document_without_the_definition() {
		#[allow(clippy::mutable_key_type)]
		let from_use = handle_rename(&two_documents(), rename_params(B_URI, 0, 15, "PRODUCT"))
			.unwrap()
			.changes
			.unwrap();
		#[allow(clippy::mutable_key_type)]
		let from_definition = handle_rename(&two_documents(), rename_params(A_URI, 0, 10, "PRODUCT"))
			.unwrap()
			.changes
			.unwrap();

		assert_eq!(from_use.len(), 2);
		assert_eq!(from_use[&uri(A_URI)].len(), 2);
		assert_eq!(from_use[&uri(B_URI)].len(), 2);
		assert_eq!(from_use, from_definition);
	}

	/// `prepareRename` decides whether the client offers to rename at all, so it
	/// has to agree with `rename` about where a name is defined.
	#[test]
	fn prepare_rename_offers_a_rename_from_a_use_site() {
		match handle_prepare_rename(&two_documents(), position_params(B_URI, 0, 15)) {
			Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) => {
				assert_eq!(placeholder, "APP_NAME");
			}
			other => panic!("unexpected prepare rename response: {other:?}"),
		}
	}

	/// A declared name spelled out in prose is still prose: rewriting it would
	/// edit the text the installer shows.
	#[test]
	fn rename_leaves_a_word_in_a_string_alone() {
		let workspace = workspace_with(&[(A_URI, "Var myVar\nDetailPrint \"about myVar\"")]);

		assert!(handle_prepare_rename(&workspace, position_params(A_URI, 1, 21)).is_none());
		assert!(handle_rename(&workspace, rename_params(A_URI, 1, 21, "other")).is_none());
	}

	#[test]
	fn rename_in_an_unopened_document_is_none() {
		assert!(
			handle_rename(
				&two_documents(),
				rename_params("file:///gone.nsi", 0, 10, "X")
			)
			.is_none()
		);
	}
}
