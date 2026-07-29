mod compiler;
mod context;
mod diagnostics;
mod nsis_data;
mod symbols;

use std::collections::HashMap;
use std::sync::LazyLock;

use ardent::{EndOfLine, Formatter, FormatterOptions};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
	CodeAction, CodeActionKind, CodeActionParams, CodeActionProviderCapability, CodeActionResponse,
	CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
	Diagnostic, DiagnosticSeverity, DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams,
	DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
	HoverParams, HoverProviderCapability, Location, LogMessageParams, MarkupContent, MarkupKind,
	MessageType, OneOf, ParameterInformation, ParameterLabel, Position, PrepareRenameResponse,
	PublishDiagnosticsParams, Range, ReferenceParams, RenameOptions, RenameParams,
	ServerCapabilities, ShowMessageParams, SignatureHelp, SignatureHelpOptions,
	SignatureHelpParams, SignatureInformation, TextDocumentSyncCapability, TextDocumentSyncKind,
	TextEdit, WorkspaceEdit,
	notification::{
		DidChangeTextDocument, DidOpenTextDocument, DidSaveTextDocument, LogMessage,
		Notification as _, PublishDiagnostics, ShowMessage,
	},
	request::{
		CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition,
		HoverRequest, PrepareRenameRequest, References, Rename, Request as _, SignatureHelpRequest,
	},
};
use serde::Deserialize;

use compiler::PreprocessMode;
use context::SyntaxContext;
use symbols::DocumentIndex;

struct DocumentState {
	text: String,
	index: DocumentIndex,
}

impl DocumentState {
	fn new(text: String) -> Self {
		let index = symbols::index_document(&text);
		Self { text, index }
	}
}

#[derive(Debug, Default, Deserialize)]
struct InitOptions {
	#[serde(default)]
	diagnostics: DiagnosticsOptions,
	#[serde(default)]
	formatter: FormatterInitOptions,
	#[serde(default)]
	makensis: MakensisOptions,
}

#[derive(Debug, Default, Deserialize)]
struct FormatterInitOptions {
	#[serde(default)]
	end_of_line: Option<String>,
	#[serde(default)]
	print_width: usize,
	#[serde(default = "default_true")]
	trim_empty_lines: bool,
	#[serde(default)]
	single_quote: bool,
}

#[derive(Debug, Deserialize)]
struct DiagnosticsOptions {
	#[serde(default = "default_preprocess_mode")]
	preprocess_mode: Option<String>,
	#[serde(default = "default_true")]
	enabled_on_save: bool,
}

#[derive(Debug, Default, Deserialize)]
struct MakensisOptions {
	#[serde(default)]
	path: String,
}

fn default_preprocess_mode() -> Option<String> {
	Some("ppo".into())
}

fn default_true() -> bool {
	true
}

impl Default for DiagnosticsOptions {
	fn default() -> Self {
		Self {
			preprocess_mode: default_preprocess_mode(),
			enabled_on_save: true,
		}
	}
}

struct LspState {
	makensis_path: Option<String>,
	preprocess_mode: PreprocessMode,
	diagnostics_on_save: bool,
	end_of_line: Option<EndOfLine>,
	print_width: usize,
	trim_empty_lines: bool,
	single_quote: bool,
}

fn main() {
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
				io_threads.join().ok();
			}
			return;
		}
	};

	let options: InitOptions = init_params
		.get("initializationOptions")
		.and_then(|v| serde_json::from_value(v.clone()).ok())
		.unwrap_or_default();

	let end_of_line = match options.formatter.end_of_line.as_deref() {
		Some("crlf") => Some(EndOfLine::Crlf),
		Some("lf") => Some(EndOfLine::Lf),
		_ => None,
	};

	let state = LspState {
		makensis_path: compiler::find_makensis(&options.makensis.path),
		preprocess_mode: PreprocessMode::from_option(
			options.diagnostics.preprocess_mode.as_deref(),
		),
		diagnostics_on_save: options.diagnostics.enabled_on_save,
		end_of_line,
		print_width: options.formatter.print_width,
		trim_empty_lines: options.formatter.trim_empty_lines,
		single_quote: options.formatter.single_quote,
	};

	log_message(
		&connection,
		MessageType::INFO,
		&format!("nsis-lsp v{} initialized", env!("CARGO_PKG_VERSION")),
	);

	if let Some(ref path) = state.makensis_path {
		log_message(
			&connection,
			MessageType::INFO,
			&format!("Using makensis: {path}"),
		);
	} else {
		log_message(
			&connection,
			MessageType::WARNING,
			"makensis not found — diagnostics unavailable",
		);
	}

	let mut documents: HashMap<String, DocumentState> = HashMap::new();

	for msg in &connection.receiver {
		match msg {
			Message::Request(req) => {
				if connection.handle_shutdown(&req).unwrap_or(true) {
					break;
				}
				handle_request(&connection, req, &documents, &state);
			}
			Message::Notification(not) => {
				handle_notification(&connection, not, &mut documents, &state);
			}
			Message::Response(_) => {}
		}
	}

	io_threads.join().ok();
}

fn handle_request(
	connection: &Connection,
	req: Request,
	documents: &HashMap<String, DocumentState>,
	state: &LspState,
) {
	match req.method.as_str() {
		Formatting::METHOD => {
			if let Ok((id, params)) = req.extract::<DocumentFormattingParams>(Formatting::METHOD) {
				match handle_formatting(documents, params, state) {
					Ok(edits) => send_response(connection, id, edits),
					Err((uri, msg)) => {
						publish_format_error(connection, uri, &msg);
						show_message(connection, MessageType::ERROR, &msg);
						send_response(connection, id, Vec::<TextEdit>::new());
					}
				}
			}
		}
		HoverRequest::METHOD => {
			if let Ok((id, params)) = req.extract::<HoverParams>(HoverRequest::METHOD) {
				send_response(connection, id, handle_hover(documents, params));
			}
		}
		Completion::METHOD => {
			if let Ok((id, params)) = req.extract::<CompletionParams>(Completion::METHOD) {
				send_response(connection, id, handle_completion(documents, params));
			}
		}
		GotoDefinition::METHOD => {
			if let Ok((id, params)) = req.extract::<GotoDefinitionParams>(GotoDefinition::METHOD) {
				send_response(connection, id, handle_goto_definition(documents, params));
			}
		}
		DocumentSymbolRequest::METHOD => {
			if let Ok((id, params)) =
				req.extract::<DocumentSymbolParams>(DocumentSymbolRequest::METHOD)
			{
				send_response(connection, id, handle_document_symbols(documents, params));
			}
		}
		References::METHOD => {
			if let Ok((id, params)) = req.extract::<ReferenceParams>(References::METHOD) {
				send_response(connection, id, handle_references(documents, params));
			}
		}
		PrepareRenameRequest::METHOD => {
			if let Ok((id, params)) =
				req.extract::<lsp_types::TextDocumentPositionParams>(PrepareRenameRequest::METHOD)
			{
				send_response(connection, id, handle_prepare_rename(documents, params));
			}
		}
		Rename::METHOD => {
			if let Ok((id, params)) = req.extract::<RenameParams>(Rename::METHOD) {
				send_response(connection, id, handle_rename(documents, params));
			}
		}
		SignatureHelpRequest::METHOD => {
			if let Ok((id, params)) =
				req.extract::<SignatureHelpParams>(SignatureHelpRequest::METHOD)
			{
				send_response(connection, id, handle_signature_help(documents, params));
			}
		}
		CodeActionRequest::METHOD => {
			if let Ok((id, params)) = req.extract::<CodeActionParams>(CodeActionRequest::METHOD) {
				send_response(connection, id, handle_code_actions(params));
			}
		}
		_ => {}
	}
}

fn handle_notification(
	connection: &Connection,
	not: Notification,
	documents: &mut HashMap<String, DocumentState>,
	state: &LspState,
) {
	if let Some(params) = cast_notification::<DidOpenTextDocument>(not.clone()) {
		let uri = params.text_document.uri;
		let text = params.text_document.text;
		publish_diagnostics(connection, uri.clone(), &text);
		documents.insert(uri.to_string(), DocumentState::new(text));
	} else if let Some(params) = cast_notification::<DidChangeTextDocument>(not.clone())
		&& let Some(change) = params.content_changes.into_iter().last()
	{
		let uri = params.text_document.uri;
		publish_diagnostics(connection, uri.clone(), &change.text);
		documents.insert(uri.to_string(), DocumentState::new(change.text));
	} else if let Some(params) = cast_notification::<DidSaveTextDocument>(not)
		&& state.diagnostics_on_save
	{
		let uri = params.text_document.uri;
		if let Some(doc) = documents.get(&uri.to_string()) {
			run_compiler_diagnostics(connection, state, &uri, &doc.text);
		}
	}
}

fn run_compiler_diagnostics(
	connection: &Connection,
	state: &LspState,
	uri: &lsp_types::Uri,
	text: &str,
) {
	let Some(makensis_path) = &state.makensis_path else {
		return;
	};

	let Some(file_path) = uri_to_file_path(&uri.to_string()) else {
		return;
	};

	let mut all_diagnostics = compute_diagnostics(text);

	let Ok(output) = compiler::run_makensis(makensis_path, &file_path, &state.preprocess_mode)
	else {
		return;
	};

	all_diagnostics.extend(diagnostics::parse_warnings(&output.stdout));
	if let Some(diag) = diagnostics::parse_error(&output.stderr) {
		all_diagnostics.push(diag);
	}

	send_diagnostics(connection, uri.clone(), all_diagnostics);
}

// ── Formatting ──

fn handle_formatting(
	documents: &HashMap<String, DocumentState>,
	params: DocumentFormattingParams,
	state: &LspState,
) -> Result<Vec<TextEdit>, (lsp_types::Uri, String)> {
	let uri = params.text_document.uri;
	let Some(doc) = documents.get(&uri.to_string()) else {
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
	};

	let Ok(formatter) = Formatter::new(options) else {
		return Ok(vec![]);
	};

	let formatted = formatter.format(text).map_err(|msg| (uri.clone(), msg))?;

	let lines: Vec<&str> = text.lines().collect();
	let last_line = lines.len().saturating_sub(1) as u32;
	let last_col = lines.last().map_or(0, |l| byte_to_utf16_offset(l, l.len()));

	let end = if text.ends_with('\n') || text.ends_with("\r\n") {
		Position::new(last_line + 1, 0)
	} else {
		Position::new(last_line, last_col)
	};

	Ok(vec![TextEdit {
		range: Range::new(Position::new(0, 0), end),
		new_text: formatted,
	}])
}

// ── Hover ──

fn handle_hover(documents: &HashMap<String, DocumentState>, params: HoverParams) -> Option<Hover> {
	let uri = params
		.text_document_position_params
		.text_document
		.uri
		.to_string();
	let text = &documents.get(&uri)?.text;
	let pos = params.text_document_position_params.position;
	let word = word_at_position(text, pos.line, pos.character)?;

	hover_for_word(&word).map(|value| Hover {
		contents: HoverContents::Markup(MarkupContent {
			kind: MarkupKind::Markdown,
			value,
		}),
		range: None,
	})
}

fn hover_for_word(word: &str) -> Option<String> {
	let bare = word.trim_start_matches('$');

	if let Some(entry) = nsis_data::lookup_doc(word) {
		let mut content = format!("**{}**\n\n{}", entry.name, entry.description);
		if let Some(params) = &entry.parameters {
			content.push_str(&format!("\n\n**Parameters:**\n```\n{}\n```", params));
		}
		if let Some(example) = &entry.example {
			content.push_str(&format!("\n\n**Example:**\n```nsis\n{}\n```", example));
		}
		return Some(content);
	}

	for (var, desc) in nsis_data::BUILTIN_VARIABLES {
		let var_bare = var.trim_start_matches('$');
		if var_bare.eq_ignore_ascii_case(bare) || var.eq_ignore_ascii_case(word) {
			return Some(format!("**{}**\n\n{}", var, desc));
		}
	}

	for (name, desc) in nsis_data::CONSTANTS {
		if name.eq_ignore_ascii_case(word) {
			return Some(format!("**{}**\n\n{}", name, desc));
		}
	}

	for dep in nsis_data::DEPRECATED_COMMANDS {
		if dep.eq_ignore_ascii_case(word) {
			return Some(format!("**{}** *(deprecated)*", dep));
		}
	}

	None
}

// ── Completion ──

/// Items that are only meaningful in code position: commands, preprocessor
/// keywords, callbacks and bare flag constants.
static CODE_ITEMS: LazyLock<Vec<CompletionItem>> = LazyLock::new(|| {
	let mut items = Vec::new();

	for entry in nsis_data::DOCS.values() {
		let kind = if entry.name.starts_with('!') {
			CompletionItemKind::KEYWORD
		} else if entry.name.starts_with('.') || entry.name.starts_with("un.") {
			CompletionItemKind::EVENT
		} else {
			CompletionItemKind::FUNCTION
		};

		let (filter_text, insert_text) = trigger_prefix_texts(&entry.name);
		items.push(CompletionItem {
			label: entry.name.clone(),
			kind: Some(kind),
			filter_text,
			insert_text,
			detail: if entry.description.is_empty() {
				None
			} else {
				Some(truncate(&entry.description, 100))
			},
			..Default::default()
		});
	}

	for (name, desc) in nsis_data::CONSTANTS {
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
	nsis_data::BUILTIN_VARIABLES
		.iter()
		.map(|(var, desc)| {
			let (filter_text, insert_text) = trigger_prefix_texts(var);
			CompletionItem {
				label: var.to_string(),
				kind: Some(CompletionItemKind::VARIABLE),
				filter_text,
				insert_text,
				detail: Some(desc.to_string()),
				..Default::default()
			}
		})
		.collect()
});

fn handle_completion(
	documents: &HashMap<String, DocumentState>,
	params: CompletionParams,
) -> Option<CompletionResponse> {
	let uri = params.text_document_position.text_document.uri.as_str();
	let pos = params.text_document_position.position;
	let doc = documents.get(uri)?;

	let items = match completion_context(&doc.text, pos) {
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

	Some(CompletionResponse::Array(items))
}

fn completion_context(text: &str, pos: Position) -> SyntaxContext {
	let line_str = text.lines().nth(pos.line as usize).unwrap_or("");
	let col = utf16_to_byte_offset(line_str, pos.character);
	context::context_at(text, pos.line, col)
}

fn trigger_prefix_texts(name: &str) -> (Option<String>, Option<String>) {
	let stripped = name.strip_prefix('!').or_else(|| name.strip_prefix('$'));
	match stripped {
		Some(rest) => (Some(rest.to_string()), Some(rest.to_string())),
		None => (None, None),
	}
}

fn truncate(s: &str, max: usize) -> String {
	match s.char_indices().nth(max) {
		Some((idx, _)) => format!("{}…", &s[..idx]),
		None => s.to_string(),
	}
}

// ── Diagnostics ──

fn send_diagnostics(connection: &Connection, uri: lsp_types::Uri, diagnostics: Vec<Diagnostic>) {
	let params = PublishDiagnosticsParams {
		uri,
		diagnostics,
		version: None,
	};
	let not = Notification::new(
		PublishDiagnostics::METHOD.to_string(),
		serde_json::to_value(params).unwrap(),
	);
	connection.sender.send(Message::Notification(not)).ok();
}

fn log_message(connection: &Connection, typ: MessageType, msg: &str) {
	let params = LogMessageParams {
		typ,
		message: msg.to_string(),
	};
	let not = Notification::new(
		LogMessage::METHOD.to_string(),
		serde_json::to_value(params).unwrap(),
	);
	connection.sender.send(Message::Notification(not)).ok();
}

fn show_message(connection: &Connection, typ: MessageType, msg: &str) {
	let params = ShowMessageParams {
		typ,
		message: msg.to_string(),
	};
	let not = Notification::new(
		ShowMessage::METHOD.to_string(),
		serde_json::to_value(params).unwrap(),
	);
	connection.sender.send(Message::Notification(not)).ok();
}

fn publish_format_error(connection: &Connection, uri: lsp_types::Uri, msg: &str) {
	let (line, col) = parse_error_position(msg).unwrap_or((0, 0));
	let pos = Position::new(line, col);
	send_diagnostics(
		connection,
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

fn publish_diagnostics(connection: &Connection, uri: lsp_types::Uri, text: &str) {
	send_diagnostics(connection, uri, compute_diagnostics(text));
}

fn compute_diagnostics(text: &str) -> Vec<Diagnostic> {
	let mut diagnostics = Vec::new();

	for (line_num, line) in text.lines().enumerate() {
		let trimmed = line.trim();
		let first_word = match trimmed.split_whitespace().next() {
			Some(w) => w,
			None => continue,
		};

		for deprecated in nsis_data::DEPRECATED_COMMANDS {
			if first_word.eq_ignore_ascii_case(deprecated) {
				let col = line.len() - line.trim_start().len();
				let start_utf16 = byte_to_utf16_offset(line, col);
				let end_utf16 = byte_to_utf16_offset(line, col + first_word.len());
				diagnostics.push(Diagnostic {
					range: Range::new(
						Position::new(line_num as u32, start_utf16),
						Position::new(line_num as u32, end_utf16),
					),
					severity: Some(DiagnosticSeverity::WARNING),
					source: Some("nsis-lsp".into()),
					message: format!("'{}' is deprecated", deprecated),
					..Default::default()
				});
				break;
			}
		}
	}

	diagnostics
}

// ── Go to Definition ──

fn handle_goto_definition(
	documents: &HashMap<String, DocumentState>,
	params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
	let uri = &params.text_document_position_params.text_document.uri;
	let doc = documents.get(&uri.to_string())?;
	let pos = params.text_document_position_params.position;
	let word = word_at_position(&doc.text, pos.line, pos.character)?;

	find_symbol_location(uri, &doc.index, &word)
}

fn find_symbol_location(
	uri: &lsp_types::Uri,
	index: &DocumentIndex,
	word: &str,
) -> Option<GotoDefinitionResponse> {
	for sym in &index.symbols {
		if sym.name.eq_ignore_ascii_case(word) {
			return Some(GotoDefinitionResponse::Scalar(Location {
				uri: uri.clone(),
				range: sym.selection_range,
			}));
		}
		for child in &sym.children {
			if child.name.eq_ignore_ascii_case(word) {
				return Some(GotoDefinitionResponse::Scalar(Location {
					uri: uri.clone(),
					range: child.selection_range,
				}));
			}
		}
	}
	None
}

// ── Document Symbols ──

fn handle_document_symbols(
	documents: &HashMap<String, DocumentState>,
	params: DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
	let doc = documents.get(&params.text_document.uri.to_string())?;
	let symbols = doc
		.index
		.symbols
		.iter()
		.map(symbol_def_to_document_symbol)
		.collect();
	Some(DocumentSymbolResponse::Nested(symbols))
}

fn symbol_def_to_document_symbol(sym: &symbols::SymbolDef) -> DocumentSymbol {
	let detail = if sym.kind == symbols::NsisSymbolKind::Function && sym.name.starts_with('.') {
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

// ── Find References ──

fn handle_references(
	documents: &HashMap<String, DocumentState>,
	params: ReferenceParams,
) -> Option<Vec<Location>> {
	let uri = &params.text_document_position.text_document.uri;
	let doc = documents.get(&uri.to_string())?;
	let pos = params.text_document_position.position;
	let word = word_at_position(&doc.text, pos.line, pos.character)?;
	let bare = word.trim_start_matches('$').trim_start_matches('!');
	let kind = symbols::find_symbol_kind(&doc.index, &word)?;

	let mut locations = Vec::new();

	if params.context.include_declaration
		&& let Some(GotoDefinitionResponse::Scalar(loc)) =
			find_symbol_location(uri, &doc.index, bare)
	{
		locations.push(loc);
	}

	for (doc_uri, doc_state) in documents {
		let parsed_uri: lsp_types::Uri = doc_uri.parse().ok()?;
		for range in symbols::find_references(&doc_state.text, bare, kind) {
			locations.push(Location {
				uri: parsed_uri.clone(),
				range,
			});
		}
	}

	if locations.is_empty() {
		None
	} else {
		Some(locations)
	}
}

// ── Rename ──

fn handle_prepare_rename(
	documents: &HashMap<String, DocumentState>,
	params: lsp_types::TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
	let uri = &params.text_document.uri;
	let doc = documents.get(&uri.to_string())?;
	let pos = params.position;
	let word = word_at_position(&doc.text, pos.line, pos.character)?;
	let bare = word.trim_start_matches('$').trim_start_matches('!');

	if is_builtin(&word) {
		return None;
	}

	symbols::find_symbol_kind(&doc.index, &word)?;

	let line_str = doc.text.lines().nth(pos.line as usize)?;
	let col = utf16_to_byte_offset(line_str, pos.character);
	let bytes = line_str.as_bytes();
	let mut start = col;
	let mut end = col;
	while start > 0 && is_ident_char(bytes[start - 1]) {
		start -= 1;
	}
	while end < bytes.len() && is_ident_char(bytes[end]) {
		end += 1;
	}

	let range = Range::new(
		Position::new(pos.line, byte_to_utf16_offset(line_str, start)),
		Position::new(pos.line, byte_to_utf16_offset(line_str, end)),
	);

	Some(PrepareRenameResponse::RangeWithPlaceholder {
		range,
		placeholder: bare.to_string(),
	})
}

fn handle_rename(
	documents: &HashMap<String, DocumentState>,
	params: RenameParams,
) -> Option<WorkspaceEdit> {
	let uri = &params.text_document_position.text_document.uri;
	let doc = documents.get(&uri.to_string())?;
	let pos = params.text_document_position.position;
	let word = word_at_position(&doc.text, pos.line, pos.character)?;
	let bare = word.trim_start_matches('$').trim_start_matches('!');
	let kind = symbols::find_symbol_kind(&doc.index, &word)?;

	if is_builtin(&word) {
		return None;
	}

	let new_name = &params.new_name;

	#[allow(clippy::mutable_key_type)]
	let mut changes: HashMap<lsp_types::Uri, Vec<TextEdit>> = HashMap::new();

	for (doc_uri, doc_state) in documents {
		let parsed_uri: lsp_types::Uri = doc_uri.parse().ok()?;

		let mut edits: Vec<TextEdit> = symbols::find_references(&doc_state.text, bare, kind)
			.into_iter()
			.map(|range| TextEdit {
				range,
				new_text: new_name.clone(),
			})
			.collect();

		for sym in &doc_state.index.symbols {
			if sym.name.eq_ignore_ascii_case(bare) && sym.kind == kind {
				edits.push(TextEdit {
					range: sym.selection_range,
					new_text: new_name.clone(),
				});
			}
			for child in &sym.children {
				if child.name.eq_ignore_ascii_case(bare) && child.kind == kind {
					edits.push(TextEdit {
						range: child.selection_range,
						new_text: new_name.clone(),
					});
				}
			}
		}

		if !edits.is_empty() {
			changes.insert(parsed_uri, edits);
		}
	}

	Some(WorkspaceEdit {
		changes: Some(changes),
		..Default::default()
	})
}

fn is_builtin(word: &str) -> bool {
	let bare = word.trim_start_matches('$');
	for (var, _) in nsis_data::BUILTIN_VARIABLES {
		let v = var.trim_start_matches('$');
		if v.eq_ignore_ascii_case(bare) {
			return true;
		}
	}
	for (name, _) in nsis_data::CONSTANTS {
		if name.eq_ignore_ascii_case(word) {
			return true;
		}
	}
	nsis_data::lookup_doc(word).is_some()
}

// ── Signature Help ──

fn handle_signature_help(
	documents: &HashMap<String, DocumentState>,
	params: SignatureHelpParams,
) -> Option<SignatureHelp> {
	let uri = params
		.text_document_position_params
		.text_document
		.uri
		.to_string();
	let text = &documents.get(&uri)?.text;
	let pos = params.text_document_position_params.position;
	let line_str = text.lines().nth(pos.line as usize)?;
	let col = utf16_to_byte_offset(line_str, pos.character);

	if is_in_comment(text, pos.line, col) {
		return None;
	}

	let trimmed = line_str.trim();
	let command = trimmed.split_whitespace().next()?;
	let entry = nsis_data::lookup_doc(command)?;
	let params_str = entry.parameters.as_deref()?;

	let parameters = parse_parameters(params_str);
	if parameters.is_empty() {
		return None;
	}

	let label = format!("{} {}", entry.name, params_str);

	let param_infos: Vec<ParameterInformation> = parameters
		.iter()
		.map(|p| ParameterInformation {
			label: ParameterLabel::Simple(p.clone()),
			documentation: None,
		})
		.collect();

	let active_param = count_active_parameter(line_str, col, command);

	Some(SignatureHelp {
		signatures: vec![SignatureInformation {
			label,
			documentation: None,
			parameters: Some(param_infos),
			active_parameter: Some(active_param),
		}],
		active_signature: Some(0),
		active_parameter: None,
	})
}

fn parse_parameters(params_str: &str) -> Vec<String> {
	let mut params = Vec::new();
	let mut chars = params_str.chars().peekable();
	let mut current = String::new();

	while let Some(&ch) = chars.peek() {
		if ch == '[' {
			let mut bracket = String::new();
			let mut depth = 0;
			for c in chars.by_ref() {
				bracket.push(c);
				if c == '[' {
					depth += 1;
				} else if c == ']' {
					depth -= 1;
					if depth == 0 {
						break;
					}
				}
			}
			if !current.trim().is_empty() {
				params.push(current.trim().to_string());
				current = String::new();
			}
			params.push(bracket.trim().to_string());
		} else if ch == '(' {
			let mut paren = String::new();
			let mut depth = 0;
			for c in chars.by_ref() {
				paren.push(c);
				if c == '(' {
					depth += 1;
				} else if c == ')' {
					depth -= 1;
					if depth == 0 {
						break;
					}
				}
			}
			if !current.trim().is_empty() {
				params.push(current.trim().to_string());
				current = String::new();
			}
			params.push(paren.trim().to_string());
		} else if ch == ' ' || ch == '\t' {
			if !current.trim().is_empty() {
				params.push(current.trim().to_string());
				current = String::new();
			}
			chars.next();
		} else {
			current.push(ch);
			chars.next();
		}
	}
	if !current.trim().is_empty() {
		params.push(current.trim().to_string());
	}
	params
}

fn count_active_parameter(line: &str, col: usize, command: &str) -> u32 {
	let trimmed = line.trim_start();
	let leading = line.len() - trimmed.len();
	let after_cmd = leading + command.len();
	if col <= after_cmd {
		return 0;
	}
	let args_portion = &line[after_cmd..col];
	let mut count = 0u32;
	let mut in_quote = false;
	let mut prev_space = true;
	for b in args_portion.bytes() {
		if b == b'"' || b == b'\'' || b == b'`' {
			in_quote = !in_quote;
			if prev_space {
				count += 1;
				prev_space = false;
			}
		} else if !in_quote && (b == b' ' || b == b'\t') {
			prev_space = true;
		} else if prev_space {
			count += 1;
			prev_space = false;
		}
	}
	count.saturating_sub(1)
}

// ── Code Actions ──

const DEPRECATED_REPLACEMENTS: &[(&str, &str)] = &[
	("SubSection", "SectionGroup"),
	("SubSectionEnd", "SectionGroupEnd"),
];

fn handle_code_actions(params: CodeActionParams) -> Option<CodeActionResponse> {
	let uri = params.text_document.uri;
	let mut actions = Vec::new();

	for diag in &params.context.diagnostics {
		if diag.source.as_deref() != Some("nsis-lsp") || !diag.message.contains("deprecated") {
			continue;
		}

		let deprecated = diag
			.message
			.strip_prefix("'")
			.and_then(|s| s.strip_suffix("' is deprecated"))?;

		let replacement = DEPRECATED_REPLACEMENTS
			.iter()
			.find(|(old, _)| old.eq_ignore_ascii_case(deprecated))
			.map(|(_, new)| *new)?;

		#[allow(clippy::mutable_key_type)]
		let mut changes = HashMap::new();
		changes.insert(
			uri.clone(),
			vec![TextEdit {
				range: diag.range,
				new_text: replacement.to_string(),
			}],
		);

		actions.push(CodeAction {
			title: format!("Replace with '{replacement}'"),
			kind: Some(CodeActionKind::QUICKFIX),
			diagnostics: Some(vec![diag.clone()]),
			edit: Some(WorkspaceEdit {
				changes: Some(changes),
				..Default::default()
			}),
			is_preferred: Some(true),
			..Default::default()
		});
	}

	Some(
		actions
			.into_iter()
			.map(lsp_types::CodeActionOrCommand::CodeAction)
			.collect(),
	)
}

// ── Utilities ──

fn is_in_comment(text: &str, line: u32, col: usize) -> bool {
	context::context_at(text, line, col) == SyntaxContext::Comment
}

fn word_at_position(text: &str, line: u32, character: u32) -> Option<String> {
	let line_str = text.lines().nth(line as usize)?;
	let col = utf16_to_byte_offset(line_str, character);
	if col > line_str.len() {
		return None;
	}

	if is_in_comment(text, line, col) {
		return None;
	}

	let bytes = line_str.as_bytes();
	let mut start = col;
	let mut end = col;

	while start > 0 && is_ident_char(bytes[start - 1]) {
		start -= 1;
	}
	while end < bytes.len() && is_ident_char(bytes[end]) {
		end += 1;
	}

	if start > 0 && (bytes[start - 1] == b'!' || bytes[start - 1] == b'$') {
		start -= 1;
	}

	if start == end {
		return None;
	}

	Some(line_str[start..end].to_string())
}

fn is_ident_char(b: u8) -> bool {
	b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

fn utf16_to_byte_offset(line: &str, utf16_col: u32) -> usize {
	let mut utf16_count = 0u32;
	for (byte_idx, ch) in line.char_indices() {
		if utf16_count >= utf16_col {
			return byte_idx;
		}
		utf16_count += ch.len_utf16() as u32;
	}
	line.len()
}

fn byte_to_utf16_offset(line: &str, byte_offset: usize) -> u32 {
	let mut count = 0u32;
	for ch in line[..byte_offset].chars() {
		count += ch.len_utf16() as u32;
	}
	count
}

fn send_response(connection: &Connection, id: RequestId, result: impl serde::Serialize) {
	let resp = Response::new_ok(id, result);
	connection.sender.send(Message::Response(resp)).ok();
}

fn uri_to_file_path(uri_str: &str) -> Option<String> {
	let stripped = uri_str.strip_prefix("file://")?;
	Some(percent_decode(stripped))
}

fn percent_decode(s: &str) -> String {
	let mut result = Vec::new();
	let bytes = s.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'%'
			&& i + 2 < bytes.len()
			&& let Ok(byte) =
				u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
		{
			result.push(byte);
			i += 3;
			continue;
		}
		result.push(bytes[i]);
		i += 1;
	}
	String::from_utf8_lossy(&result).into_owned()
}

fn cast_notification<N: lsp_types::notification::Notification>(
	not: Notification,
) -> Option<N::Params> {
	not.extract::<N::Params>(N::METHOD).ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	// ── percent_decode / uri_to_file_path ──

	#[test]
	fn percent_decode_no_encoding() {
		assert_eq!(percent_decode("/foo/bar.nsi"), "/foo/bar.nsi");
	}

	#[test]
	fn percent_decode_spaces() {
		assert_eq!(
			percent_decode("/my%20path/file%20name"),
			"/my path/file name"
		);
	}

	#[test]
	fn percent_decode_mixed() {
		assert_eq!(percent_decode("a%2Fb%25c"), "a/b%c");
	}

	#[test]
	fn uri_to_file_path_valid() {
		assert_eq!(
			uri_to_file_path("file:///home/user/test.nsi"),
			Some("/home/user/test.nsi".into())
		);
	}

	#[test]
	fn uri_to_file_path_not_file_uri() {
		assert_eq!(uri_to_file_path("https://example.com"), None);
	}

	// ── parse_error_position ──

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

	// ── trigger_prefix_texts ──

	#[test]
	fn trigger_prefix_bang() {
		let (f, i) = trigger_prefix_texts("!include");
		assert_eq!(f, Some("include".into()));
		assert_eq!(i, Some("include".into()));
	}

	#[test]
	fn trigger_prefix_dollar() {
		let (f, i) = trigger_prefix_texts("$INSTDIR");
		assert_eq!(f, Some("INSTDIR".into()));
		assert_eq!(i, Some("INSTDIR".into()));
	}

	#[test]
	fn trigger_prefix_none() {
		let (f, i) = trigger_prefix_texts("Section");
		assert_eq!(f, None);
		assert_eq!(i, None);
	}

	// ── is_ident_char ──

	#[test]
	fn ident_char_alphanumeric() {
		assert!(is_ident_char(b'a'));
		assert!(is_ident_char(b'Z'));
		assert!(is_ident_char(b'5'));
	}

	#[test]
	fn ident_char_underscore_dot() {
		assert!(is_ident_char(b'_'));
		assert!(is_ident_char(b'.'));
	}

	#[test]
	fn ident_char_rejects_special() {
		assert!(!is_ident_char(b' '));
		assert!(!is_ident_char(b'!'));
		assert!(!is_ident_char(b'$'));
	}

	// ── utf16 <-> byte offset ──

	#[test]
	fn utf16_to_byte_ascii() {
		assert_eq!(utf16_to_byte_offset("hello", 3), 3);
	}

	#[test]
	fn byte_to_utf16_ascii() {
		assert_eq!(byte_to_utf16_offset("hello", 3), 3);
	}

	#[test]
	fn utf16_roundtrip_multibyte() {
		let line = "aé€b";
		let byte_off = utf16_to_byte_offset(line, 3);
		let utf16_off = byte_to_utf16_offset(line, byte_off);
		assert_eq!(utf16_off, 3);
	}

	// ── word_at_position ──

	#[test]
	fn word_at_position_simple() {
		let text = "Section main\n  DetailPrint hello\nSectionEnd";
		assert_eq!(word_at_position(text, 1, 4), Some("DetailPrint".into()));
	}

	#[test]
	fn word_at_position_bang_prefix() {
		let text = "!include file.nsh";
		assert_eq!(word_at_position(text, 0, 2), Some("!include".into()));
	}

	#[test]
	fn word_at_position_dollar_prefix() {
		let text = "StrCpy $0 $INSTDIR";
		assert_eq!(word_at_position(text, 0, 12), Some("$INSTDIR".into()));
	}

	#[test]
	fn word_at_position_out_of_range() {
		assert_eq!(word_at_position("hello", 5, 0), None);
	}

	// ── is_in_comment ──

	#[test]
	fn line_comment_hash() {
		let text = "# this is a comment";
		assert!(is_in_comment(text, 0, 5));
	}

	#[test]
	fn line_comment_semicolon() {
		let text = "; this is a comment";
		assert!(is_in_comment(text, 0, 5));
	}

	#[test]
	fn not_in_comment() {
		let text = "Section main";
		assert!(!is_in_comment(text, 0, 3));
	}

	#[test]
	fn block_comment() {
		let text = "/* comment\nstill comment */\ncode";
		assert!(is_in_comment(text, 1, 2));
		assert!(!is_in_comment(text, 2, 0));
	}

	#[test]
	fn word_at_position_in_comment_returns_none() {
		let text = "# DetailPrint hello";
		assert_eq!(word_at_position(text, 0, 5), None);
	}

	// ── handle_completion ──

	const TEST_URI: &str = "file:///test.nsi";

	fn completion_labels(text: &str, line: u32, character: u32) -> Vec<String> {
		let mut documents = HashMap::new();
		documents.insert(TEST_URI.to_string(), DocumentState::new(text.to_string()));

		let params = CompletionParams {
			text_document_position: lsp_types::TextDocumentPositionParams {
				text_document: lsp_types::TextDocumentIdentifier {
					uri: TEST_URI.parse().unwrap(),
				},
				position: Position { line, character },
			},
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
			context: None,
		};

		match handle_completion(&documents, params) {
			Some(CompletionResponse::Array(items)) => items.into_iter().map(|i| i.label).collect(),
			other => panic!("unexpected completion response: {other:?}"),
		}
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
		let documents: HashMap<String, DocumentState> = HashMap::new();
		let params = CompletionParams {
			text_document_position: lsp_types::TextDocumentPositionParams {
				text_document: lsp_types::TextDocumentIdentifier {
					uri: TEST_URI.parse().unwrap(),
				},
				position: Position {
					line: 0,
					character: 0,
				},
			},
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
			context: None,
		};
		assert!(handle_completion(&documents, params).is_none());
	}

	// ── compute_diagnostics ──

	#[test]
	fn compute_diagnostics_deprecated_command() {
		let text = "SubSection test";
		let diags = compute_diagnostics(text);
		assert_eq!(diags.len(), 1);
		assert!(diags[0].message.contains("deprecated"));
	}

	#[test]
	fn compute_diagnostics_no_deprecated() {
		let text = "Section main\nSectionEnd";
		let diags = compute_diagnostics(text);
		assert!(diags.is_empty());
	}

	#[test]
	fn compute_diagnostics_case_insensitive() {
		let text = "subsection test";
		let diags = compute_diagnostics(text);
		assert_eq!(diags.len(), 1);
	}

	// ── hover_for_word ──

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

	// ── parse_parameters ──

	#[test]
	fn parse_params_simple() {
		let params = parse_parameters("user_message");
		assert_eq!(params, vec!["user_message"]);
	}

	#[test]
	fn parse_params_with_brackets() {
		let params = parse_parameters("(left|right|top|bottom) (width|height) [padding]");
		assert_eq!(
			params,
			vec!["(left|right|top|bottom)", "(width|height)", "[padding]"]
		);
	}

	#[test]
	fn parse_params_multiple_tokens() {
		let params = parse_parameters("hwnd dialog_id");
		assert_eq!(params, vec!["hwnd", "dialog_id"]);
	}

	#[test]
	fn parse_params_nested_brackets() {
		let params = parse_parameters("command [options [sub_options]]");
		assert_eq!(params, vec!["command", "[options [sub_options]]"]);
	}

	// ── count_active_parameter ──

	#[test]
	fn active_param_on_command() {
		assert_eq!(count_active_parameter("  MessageBox", 12, "MessageBox"), 0);
	}

	#[test]
	fn active_param_first_arg() {
		assert_eq!(
			count_active_parameter("  MessageBox MB_OK", 18, "MessageBox"),
			0
		);
	}

	#[test]
	fn active_param_second_arg() {
		assert_eq!(
			count_active_parameter("  MessageBox MB_OK \"hello\"", 26, "MessageBox"),
			1
		);
	}

	#[test]
	fn active_param_quoted_string_as_one() {
		assert_eq!(
			count_active_parameter("  File \"my file.exe\"", 20, "File"),
			0
		);
	}
}
