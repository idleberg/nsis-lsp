mod cli;
mod client;
mod compiler;
mod context;
mod deprecation;
mod diagnostics;
mod handlers;
mod nsis_data;
mod position;
mod settings;
mod symbols;
mod workspace;

#[cfg(test)]
mod testing;

use lsp_server::{Connection, Message, Notification, Request};
use lsp_types::{
	CodeActionParams, CodeActionProviderCapability, CompletionOptions, CompletionParams,
	DocumentFormattingParams, DocumentSymbolParams, GotoDefinitionParams, HoverParams,
	HoverProviderCapability, MessageType, OneOf, ReferenceParams, RenameOptions, RenameParams,
	ServerCapabilities, SignatureHelpOptions, SignatureHelpParams, TextDocumentSyncCapability,
	TextDocumentSyncKind, TextEdit,
	notification::{
		DidChangeConfiguration, DidChangeTextDocument, DidOpenTextDocument, DidSaveTextDocument,
	},
	request::{
		CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition,
		HoverRequest, PrepareRenameRequest, References, Rename, Request as _, SignatureHelpRequest,
	},
};

use cli::Cli;
use client::{Client, Stdio};
use handlers::code_action::handle_code_actions;
use handlers::completion::handle_completion;
use handlers::formatting::{handle_formatting, publish_format_error};
use handlers::hover::handle_hover;
use handlers::navigation::{handle_goto_definition, handle_references};
use handlers::rename::{handle_prepare_rename, handle_rename};
use handlers::signature::handle_signature_help;
use handlers::symbols::handle_document_symbols;
use settings::{InitOptions, LspState, handle_configuration_change, log_makensis_path};
use workspace::{Document, Workspace};

use clap::Parser;

fn main() {
	// Exits on --version, --help, or an unrecognised argument.
	Cli::parse();

	let (connection, io_threads) = Connection::stdio();

	let capabilities = ServerCapabilities {
		document_formatting_provider: Some(OneOf::Left(true)),
		hover_provider: Some(HoverProviderCapability::Simple(true)),
		completion_provider: Some(CompletionOptions {
			trigger_characters: Some(vec!["!".into(), "$".into()]),
			..Default::default()
		}),
		definition_provider: Some(OneOf::Left(true)),
		document_symbol_provider: Some(OneOf::Left(true)),
		references_provider: Some(OneOf::Left(true)),
		rename_provider: Some(OneOf::Right(RenameOptions {
			prepare_provider: Some(true),
			work_done_progress_options: Default::default(),
		})),
		signature_help_provider: Some(SignatureHelpOptions {
			trigger_characters: Some(vec![" ".into()]),
			retrigger_characters: None,
			work_done_progress_options: Default::default(),
		}),
		code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
		text_document_sync: Some(TextDocumentSyncCapability::Options(
			lsp_types::TextDocumentSyncOptions {
				open_close: Some(true),
				change: Some(TextDocumentSyncKind::FULL),
				save: Some(lsp_types::TextDocumentSyncSaveOptions::Supported(true)),
				..Default::default()
			},
		)),
		..Default::default()
	};

	let init_result = connection.initialize(serde_json::to_value(&capabilities).unwrap());
	let init_params = match init_result {
		Ok(params) => params,
		Err(e) => {
			if e.channel_is_disconnected() {
				drop(connection);
				io_threads.join().ok();
			}
			return;
		}
	};

	let options: InitOptions = init_params
		.get("initializationOptions")
		.and_then(|v| serde_json::from_value(v.clone()).ok())
		.unwrap_or_default();

	let mut state = LspState::from_options(options);
	let client = Stdio::new(&connection);

	client.log(
		MessageType::INFO,
		&format!("nsis-lsp v{} initialized", env!("CARGO_PKG_VERSION")),
	);
	log_makensis_path(&client, state.makensis_path.as_deref());

	let mut workspace = Workspace::new();

	for msg in &connection.receiver {
		match msg {
			Message::Request(req) => {
				if connection.handle_shutdown(&req).unwrap_or(true) {
					break;
				}
				handle_request(&client, req, &workspace, &state);
			}
			Message::Notification(not) => {
				handle_notification(&client, not, &mut workspace, &mut state);
			}
			Message::Response(_) => {}
		}
	}

	// The writer thread only finishes once every sender is gone, so the
	// connection has to be dropped before joining or `join` blocks forever.
	drop(connection);
	io_threads.join().ok();
}

fn handle_request(client: &impl Client, req: Request, workspace: &Workspace, state: &LspState) {
	match req.method.as_str() {
		Formatting::METHOD => {
			if let Ok((id, params)) = req.extract::<DocumentFormattingParams>(Formatting::METHOD) {
				match handle_formatting(workspace, params, state) {
					Ok(edits) => client.respond(id, edits),
					Err((uri, msg)) => {
						publish_format_error(client, uri, &msg);
						client.show_message(MessageType::ERROR, &msg);
						client.respond(id, Vec::<TextEdit>::new());
					}
				}
			}
		}
		HoverRequest::METHOD => {
			if let Ok((id, params)) = req.extract::<HoverParams>(HoverRequest::METHOD) {
				client.respond(id, handle_hover(workspace, params));
			}
		}
		Completion::METHOD => {
			if let Ok((id, params)) = req.extract::<CompletionParams>(Completion::METHOD) {
				client.respond(id, handle_completion(workspace, params));
			}
		}
		GotoDefinition::METHOD => {
			if let Ok((id, params)) = req.extract::<GotoDefinitionParams>(GotoDefinition::METHOD) {
				client.respond(id, handle_goto_definition(workspace, params));
			}
		}
		DocumentSymbolRequest::METHOD => {
			if let Ok((id, params)) =
				req.extract::<DocumentSymbolParams>(DocumentSymbolRequest::METHOD)
			{
				client.respond(id, handle_document_symbols(workspace, params));
			}
		}
		References::METHOD => {
			if let Ok((id, params)) = req.extract::<ReferenceParams>(References::METHOD) {
				client.respond(id, handle_references(workspace, params));
			}
		}
		PrepareRenameRequest::METHOD => {
			if let Ok((id, params)) =
				req.extract::<lsp_types::TextDocumentPositionParams>(PrepareRenameRequest::METHOD)
			{
				client.respond(id, handle_prepare_rename(workspace, params));
			}
		}
		Rename::METHOD => {
			if let Ok((id, params)) = req.extract::<RenameParams>(Rename::METHOD) {
				client.respond(id, handle_rename(workspace, params));
			}
		}
		SignatureHelpRequest::METHOD => {
			if let Ok((id, params)) =
				req.extract::<SignatureHelpParams>(SignatureHelpRequest::METHOD)
			{
				client.respond(id, handle_signature_help(workspace, params));
			}
		}
		CodeActionRequest::METHOD => {
			if let Ok((id, params)) = req.extract::<CodeActionParams>(CodeActionRequest::METHOD) {
				client.respond(id, handle_code_actions(params));
			}
		}
		_ => {}
	}
}

fn handle_notification(
	client: &impl Client,
	not: Notification,
	workspace: &mut Workspace,
	state: &mut LspState,
) {
	if let Some(params) = cast_notification::<DidChangeConfiguration>(not.clone()) {
		handle_configuration_change(client, params.settings, state);
	} else if let Some(params) = cast_notification::<DidOpenTextDocument>(not.clone()) {
		let doc = workspace.open(params.text_document.uri, params.text_document.text);
		client.publish_diagnostics(doc.uri.clone(), doc.diagnostics.clone());
	} else if let Some(params) = cast_notification::<DidChangeTextDocument>(not.clone())
		&& let Some(change) = params.content_changes.into_iter().last()
	{
		let doc = workspace.open(params.text_document.uri, change.text);
		client.publish_diagnostics(doc.uri.clone(), doc.diagnostics.clone());
	} else if let Some(params) = cast_notification::<DidSaveTextDocument>(not)
		&& state.diagnostics_on_save
		&& let Some(doc) = workspace.document(&params.text_document.uri)
	{
		run_compiler_diagnostics(client, state, doc);
	}
}

/// Republish `doc`'s diagnostics with what the compiler has to say on top.
///
/// The document's own diagnostics come along because the client replaces the
/// whole set for a URI on every publish — dropping them here would clear the
/// deprecation warnings the moment the file was saved.
fn run_compiler_diagnostics(client: &impl Client, state: &LspState, doc: &Document) {
	let Some(makensis_path) = &state.makensis_path else {
		return;
	};

	let Some(file_path) = compiler::uri_to_file_path(&doc.uri.to_string()) else {
		return;
	};

	let Ok(output) = compiler::run_makensis(makensis_path, &file_path, &state.preprocess_mode)
	else {
		return;
	};

	let mut all_diagnostics = doc.diagnostics.clone();
	all_diagnostics.extend(diagnostics::parse_warnings(&output.stdout));
	if let Some(diag) = diagnostics::parse_error(&output.stderr) {
		all_diagnostics.push(diag);
	}

	client.publish_diagnostics(doc.uri.clone(), all_diagnostics);
}

fn cast_notification<N: lsp_types::notification::Notification>(
	not: Notification,
) -> Option<N::Params> {
	not.extract::<N::Params>(N::METHOD).ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	use client::Recorder;
	use lsp_types::Hover;
	use lsp_types::notification::Notification as _;
	use serde_json::json;
	use testing::{TEST_URI, position_params, quiet_state, uri, workspace_with};

	fn did_open(uri_str: &str, text: &str) -> Notification {
		Notification::new(
			DidOpenTextDocument::METHOD.to_string(),
			json!({
				"textDocument": {
					"uri": uri_str,
					"languageId": "nsis",
					"version": 1,
					"text": text,
				},
			}),
		)
	}

	fn did_change(uri_str: &str, text: &str) -> Notification {
		Notification::new(
			DidChangeTextDocument::METHOD.to_string(),
			json!({
				"textDocument": { "uri": uri_str, "version": 2 },
				"contentChanges": [{ "text": text }],
			}),
		)
	}

	fn configuration_change(settings: serde_json::Value) -> Notification {
		Notification::new(
			DidChangeConfiguration::METHOD.to_string(),
			json!({ "settings": settings }),
		)
	}

	#[test]
	fn opening_a_document_indexes_it_and_publishes_diagnostics() {
		let client = Recorder::new();
		let mut workspace = Workspace::new();

		handle_notification(
			&client,
			did_open(TEST_URI, "Function myFunc\nFunctionEnd"),
			&mut workspace,
			&mut quiet_state(),
		);

		let doc = workspace.document(&uri(TEST_URI)).unwrap();
		assert_eq!(doc.index.roots().len(), 1);

		let published = client.diagnostics();
		assert_eq!(published.len(), 1);
		assert_eq!(published[0].uri, uri(TEST_URI));
		assert_eq!(published[0].diagnostics, doc.diagnostics);
	}

	/// A change carries the whole document, so the diagnostics that go with it
	/// are computed from the new text, not the text on disk.
	#[test]
	fn changing_a_document_republishes_diagnostics_for_the_new_text() {
		let client = Recorder::new();
		let mut workspace = Workspace::new();
		let mut state = quiet_state();

		handle_notification(
			&client,
			did_open(TEST_URI, "Section main"),
			&mut workspace,
			&mut state,
		);
		handle_notification(
			&client,
			did_change(TEST_URI, "Function myFunc\nFunctionEnd"),
			&mut workspace,
			&mut state,
		);

		let doc = workspace.document(&uri(TEST_URI)).unwrap();
		assert_eq!(doc.text, "Function myFunc\nFunctionEnd");

		let published = client.diagnostics();
		assert_eq!(published.len(), 2);
		assert_eq!(published[1].diagnostics, doc.diagnostics);
	}

	/// The document is the one source of what its text implies, so a warning
	/// cannot outlive the text that produced it: the edit that fixes the script
	/// is the edit that clears the warning.
	#[test]
	fn fixing_the_text_clears_the_warning_it_produced() {
		let client = Recorder::new();
		let mut workspace = Workspace::new();
		let mut state = quiet_state();

		handle_notification(
			&client,
			did_open(TEST_URI, "SubSection foo"),
			&mut workspace,
			&mut state,
		);
		handle_notification(
			&client,
			did_change(TEST_URI, "SectionGroup foo"),
			&mut workspace,
			&mut state,
		);

		let published = client.diagnostics();
		assert_eq!(published[0].diagnostics.len(), 1);
		assert!(published[1].diagnostics.is_empty());
		assert!(
			workspace
				.document(&uri(TEST_URI))
				.unwrap()
				.diagnostics
				.is_empty()
		);
	}

	/// Without a compiler there is nothing to run on save, and the diagnostics
	/// already published stay as they are.
	#[test]
	fn saving_without_a_compiler_says_nothing() {
		let client = Recorder::new();
		let mut workspace = workspace_with(&[(TEST_URI, "Section main")]);

		handle_notification(
			&client,
			Notification::new(
				DidSaveTextDocument::METHOD.to_string(),
				json!({ "textDocument": { "uri": TEST_URI } }),
			),
			&mut workspace,
			&mut quiet_state(),
		);

		assert!(client.is_silent());
	}

	#[test]
	fn a_configuration_change_reloads_the_state_and_says_so() {
		let client = Recorder::new();
		let mut state = quiet_state();

		handle_notification(
			&client,
			configuration_change(json!({ "nsis": { "formatter": { "print_width": 100 } } })),
			&mut Workspace::new(),
			&mut state,
		);

		assert_eq!(state.print_width, 100);
		assert!(client.logs().iter().any(|l| l == "Configuration reloaded"));
	}

	#[test]
	fn unparseable_settings_are_logged_and_left_alone() {
		let client = Recorder::new();
		let mut state = quiet_state();
		state.print_width = 80;

		handle_notification(
			&client,
			configuration_change(json!({ "formatter": { "print_width": "wide" } })),
			&mut Workspace::new(),
			&mut state,
		);

		assert_eq!(state.print_width, 80);
		assert_eq!(client.logs().len(), 1);
		assert!(client.logs()[0].contains("could not be parsed"));
	}

	/// Clients that keep their settings server-side send `null`. There is
	/// nothing to reload and nothing worth logging.
	#[test]
	fn settings_the_client_will_not_send_are_ignored_silently() {
		let client = Recorder::new();

		handle_notification(
			&client,
			configuration_change(serde_json::Value::Null),
			&mut Workspace::new(),
			&mut quiet_state(),
		);

		assert!(client.is_silent());
	}

	#[test]
	fn a_request_is_answered_once() {
		let client = Recorder::new();
		let workspace = workspace_with(&[(TEST_URI, "!include file.nsh")]);

		handle_request(
			&client,
			Request::new(
				1.into(),
				HoverRequest::METHOD.to_string(),
				position_params(TEST_URI, 0, 2),
			),
			&workspace,
			&quiet_state(),
		);

		assert!(client.response::<Hover>().is_some());
	}

	/// A method the server never advertised is dropped rather than answered.
	#[test]
	fn an_unknown_request_is_left_alone() {
		let client = Recorder::new();

		handle_request(
			&client,
			Request::new(1.into(), "textDocument/inlayHint".to_string(), json!({})),
			&Workspace::new(),
			&quiet_state(),
		);

		assert!(client.is_silent());
	}
}
