//! The fixes on offer for the diagnostics the client sent back.
//!
//! Which fixes those are is each diagnostic's own module's business — this only
//! asks every producer in turn and collects what comes back.

use lsp_types::{CodeActionOrCommand, CodeActionParams, CodeActionResponse};

use crate::deprecation;

/// Every fix on offer for the diagnostics the client sent back.
///
/// One diagnostic that nothing can be done about is one diagnostic skipped —
/// it says nothing about the rest of the request.
pub fn handle_code_actions(params: CodeActionParams) -> Option<CodeActionResponse> {
	let uri = params.text_document.uri;

	Some(
		params
			.context
			.diagnostics
			.iter()
			.filter_map(|diag| deprecation::fix(&uri, diag))
			.map(CodeActionOrCommand::CodeAction)
			.collect(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::{TEST_URI, uri};
	use lsp_types::{CodeActionContext, Range, TextDocumentIdentifier};

	fn actions(diagnostics: Vec<lsp_types::Diagnostic>) -> CodeActionResponse {
		handle_code_actions(CodeActionParams {
			text_document: TextDocumentIdentifier { uri: uri(TEST_URI) },
			range: Range::default(),
			context: CodeActionContext {
				diagnostics,
				..Default::default()
			},
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
		})
		.unwrap()
	}

	/// The diagnostics come back from the client the way it received them, so a
	/// deprecation warning still carries what its quickfix needs.
	#[test]
	fn a_deprecation_the_client_returns_is_offered_a_fix() {
		let offered = actions(deprecation::scan("SubSection foo"));
		assert_eq!(offered.len(), 1);
	}

	#[test]
	fn a_request_with_no_diagnostics_offers_nothing() {
		assert!(actions(vec![]).is_empty());
	}
}
