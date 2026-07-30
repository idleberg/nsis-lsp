//! Fixtures the handler tests share.
//!
//! Each handler is tested through the LSP params a client would actually send,
//! and building those by hand is most of the noise in a handler test. Splitting
//! the handlers apart put the same fixtures in eight modules at once, so they
//! live here instead.

use lsp_types::{
	Position, ReferenceContext, ReferenceParams, RenameParams, TextDocumentIdentifier,
	TextDocumentPositionParams, Uri,
};

use crate::settings::{InitOptions, LspState};
use crate::workspace::Workspace;

pub const TEST_URI: &str = "file:///test.nsi";
pub const A_URI: &str = "file:///a.nsi";
pub const B_URI: &str = "file:///b.nsi";

pub fn uri(s: &str) -> Uri {
	s.parse().unwrap()
}

pub fn workspace_with(documents: &[(&str, &str)]) -> Workspace {
	let mut workspace = Workspace::new();
	for (uri_str, text) in documents {
		workspace.open(uri(uri_str), text.to_string());
	}
	workspace
}

/// A header declaring a name, and a script that only uses it.
pub fn two_documents() -> Workspace {
	workspace_with(&[
		(A_URI, "!define APP_NAME \"Test\"\nDetailPrint ${APP_NAME}"),
		(B_URI, "DetailPrint ${APP_NAME}\nDetailPrint ${APP_NAME}"),
	])
}

pub fn position_params(uri_str: &str, line: u32, character: u32) -> TextDocumentPositionParams {
	TextDocumentPositionParams {
		text_document: TextDocumentIdentifier { uri: uri(uri_str) },
		position: Position { line, character },
	}
}

pub fn rename_params(uri_str: &str, line: u32, character: u32, new_name: &str) -> RenameParams {
	RenameParams {
		text_document_position: position_params(uri_str, line, character),
		new_name: new_name.to_string(),
		work_done_progress_params: Default::default(),
	}
}

pub fn reference_params(uri_str: &str, line: u32, character: u32) -> ReferenceParams {
	ReferenceParams {
		text_document_position: position_params(uri_str, line, character),
		context: ReferenceContext {
			include_declaration: true,
		},
		work_done_progress_params: Default::default(),
		partial_result_params: Default::default(),
	}
}

/// A server that has found no compiler, so nothing shells out during a test.
pub fn quiet_state() -> LspState {
	let mut state = LspState::from_options(InitOptions::default());
	state.makensis_path = None;
	state
}
