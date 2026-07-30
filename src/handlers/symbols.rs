//! The outline of one document, in the nested shape `documentSymbol` wants.
//!
//! The nesting is [`DocumentIndex`]'s: this only renders what it already holds.
//!
//! [`DocumentIndex`]: crate::symbols::DocumentIndex

use lsp_types::{DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse};

use crate::symbols::{NsisSymbolKind, SymbolDef};
use crate::workspace::Workspace;

pub fn handle_document_symbols(
	workspace: &Workspace,
	params: DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
	let doc = workspace.document(&params.text_document.uri)?;
	let symbols = doc
		.index
		.roots()
		.iter()
		.map(symbol_def_to_document_symbol)
		.collect();
	Some(DocumentSymbolResponse::Nested(symbols))
}

fn symbol_def_to_document_symbol(sym: &SymbolDef) -> DocumentSymbol {
	let detail = if sym.kind == NsisSymbolKind::Function && sym.name.starts_with('.') {
		Some("callback".to_string())
	} else {
		None
	};

	#[allow(deprecated)]
	DocumentSymbol {
		name: if sym.name.is_empty() {
			"(unnamed)".to_string()
		} else {
			sym.name.clone()
		},
		detail,
		kind: sym.kind.to_lsp(),
		tags: None,
		deprecated: None,
		range: sym.range,
		selection_range: sym.selection_range,
		children: if sym.children.is_empty() {
			None
		} else {
			Some(
				sym.children
					.iter()
					.map(symbol_def_to_document_symbol)
					.collect(),
			)
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::{TEST_URI, uri, workspace_with};
	use lsp_types::TextDocumentIdentifier;

	fn outline(text: &str) -> Vec<DocumentSymbol> {
		let workspace = workspace_with(&[(TEST_URI, text)]);
		let params = DocumentSymbolParams {
			text_document: TextDocumentIdentifier { uri: uri(TEST_URI) },
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
		};

		match handle_document_symbols(&workspace, params) {
			Some(DocumentSymbolResponse::Nested(symbols)) => symbols,
			other => panic!("unexpected document symbol response: {other:?}"),
		}
	}

	/// Labels hang under the section they sit in, which is the two-deep shape
	/// the client renders as a tree.
	#[test]
	fn labels_are_nested_under_their_container() {
		let symbols = outline("Section main\nstart:\nSectionEnd");

		assert_eq!(symbols.len(), 1);
		assert_eq!(symbols[0].name, "main");
		assert_eq!(symbols[0].children.as_ref().unwrap().len(), 1);
		assert_eq!(symbols[0].children.as_ref().unwrap()[0].name, "start");
	}

	/// A callback is a function whose name the user did not choose, and saying
	/// so is the only thing that tells it apart in the outline.
	#[test]
	fn a_callback_is_labelled_as_one() {
		let symbols = outline("Function .onInit\nFunctionEnd");
		assert_eq!(symbols[0].detail.as_deref(), Some("callback"));
	}

	#[test]
	fn an_unopened_document_has_no_outline() {
		let params = DocumentSymbolParams {
			text_document: TextDocumentIdentifier { uri: uri(TEST_URI) },
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
		};
		assert!(handle_document_symbols(&Workspace::new(), params).is_none());
	}
}
