mod cli;
mod client;
mod compiler;
mod context;
mod diagnostics;
mod nsis_data;
mod position;
mod symbols;
mod workspace;

use std::collections::HashMap;
use std::sync::LazyLock;

use ardent::{EndOfLine, Formatter, FormatterOptions};
use clap::Parser;
use lsp_server::{Connection, Message, Notification, Request};
use lsp_types::{
	CodeAction, CodeActionKind, CodeActionParams, CodeActionProviderCapability, CodeActionResponse,
	CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
	CompletionTextEdit, Diagnostic, DiagnosticSeverity, DocumentFormattingParams, DocumentSymbol,
	DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
	Hover, HoverContents, HoverParams, HoverProviderCapability, Location, MarkupContent,
	MarkupKind, MessageType, OneOf, ParameterInformation, ParameterLabel, Position,
	PrepareRenameResponse, Range, ReferenceParams, RenameOptions, RenameParams, ServerCapabilities,
	SignatureHelp, SignatureHelpOptions, SignatureHelpParams, SignatureInformation,
	TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, WorkspaceEdit,
	notification::{
		DidChangeConfiguration, DidChangeTextDocument, DidOpenTextDocument, DidSaveTextDocument,
	},
	request::{
		CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition,
		HoverRequest, PrepareRenameRequest, References, Rename, Request as _, SignatureHelpRequest,
	},
};
use serde::Deserialize;

use cli::Cli;
use client::{Client, Stdio};
use compiler::PreprocessMode;
use context::SyntaxContext;
use position::{byte_to_utf16_offset, is_ident_char, line_at, utf16_to_byte_offset};
use symbols::DocumentIndex;
use workspace::Workspace;

#[derive(Debug, Default, Deserialize)]
struct InitOptions {
	#[serde(default)]
	diagnostics: DiagnosticsOptions,
	#[serde(default)]
	formatter: FormatterInitOptions,
	#[serde(default)]
	makensis: MakensisOptions,
}

#[derive(Debug, Deserialize)]
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

// Mirrors the `serde` defaults above, so an absent `formatter` key and an empty
// one produce the same settings.
impl Default for FormatterInitOptions {
	fn default() -> Self {
		Self {
			end_of_line: None,
			print_width: 0,
			trim_empty_lines: true,
			single_quote: false,
		}
	}
}

fn parse_end_of_line(value: Option<&str>) -> Option<EndOfLine> {
	match value {
		Some("crlf") => Some(EndOfLine::Crlf),
		Some("lf") => Some(EndOfLine::Lf),
		_ => None,
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

impl LspState {
	fn from_options(options: InitOptions) -> Self {
		Self {
			makensis_path: compiler::find_makensis(&options.makensis.path),
			preprocess_mode: PreprocessMode::from_option(
				options.diagnostics.preprocess_mode.as_deref(),
			),
			diagnostics_on_save: options.diagnostics.enabled_on_save,
			end_of_line: parse_end_of_line(options.formatter.end_of_line.as_deref()),
			print_width: options.formatter.print_width,
			trim_empty_lines: options.formatter.trim_empty_lines,
			single_quote: options.formatter.single_quote,
		}
	}
}

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
		let uri = params.text_document.uri;
		let text = params.text_document.text;
		publish_diagnostics(client, uri.clone(), &text);
		workspace.open(uri, text);
	} else if let Some(params) = cast_notification::<DidChangeTextDocument>(not.clone())
		&& let Some(change) = params.content_changes.into_iter().last()
	{
		let uri = params.text_document.uri;
		publish_diagnostics(client, uri.clone(), &change.text);
		workspace.open(uri, change.text);
	} else if let Some(params) = cast_notification::<DidSaveTextDocument>(not)
		&& state.diagnostics_on_save
		&& let Some(doc) = workspace.document(&params.text_document.uri)
	{
		run_compiler_diagnostics(client, state, &doc.uri, &doc.text);
	}
}

enum SettingsUpdate {
	Replace(InitOptions),
	// Clients that hold their settings server-side send `null`, which we cannot
	// resolve without a `workspace/configuration` request.
	Unavailable,
	Unparseable,
}

// Settings take the same shape as `initializationOptions`, optionally wrapped in
// an `nsis` section. They replace the previous set wholesale, so a client has to
// send every option it cares about, not just the one that changed.
fn parse_settings(settings: serde_json::Value) -> SettingsUpdate {
	let settings = match settings.get("nsis") {
		Some(section) => section.clone(),
		None => settings,
	};

	if settings.is_null() {
		return SettingsUpdate::Unavailable;
	}

	match serde_json::from_value(settings) {
		Ok(options) => SettingsUpdate::Replace(options),
		Err(_) => SettingsUpdate::Unparseable,
	}
}

fn handle_configuration_change(
	client: &impl Client,
	settings: serde_json::Value,
	state: &mut LspState,
) {
	let options = match parse_settings(settings) {
		SettingsUpdate::Replace(options) => options,
		SettingsUpdate::Unavailable => return,
		SettingsUpdate::Unparseable => {
			client.log(
				MessageType::WARNING,
				"Ignoring workspace/didChangeConfiguration: settings could not be parsed",
			);
			return;
		}
	};

	let previous_makensis = state.makensis_path.clone();
	*state = LspState::from_options(options);

	client.log(MessageType::INFO, "Configuration reloaded");

	if state.makensis_path != previous_makensis {
		log_makensis_path(client, state.makensis_path.as_deref());
	}
}

/// Tell the client which compiler the server found, or that it found none.
fn log_makensis_path(client: &impl Client, path: Option<&str>) {
	match path {
		Some(path) => client.log(MessageType::INFO, &format!("Using makensis: {path}")),
		None => client.log(
			MessageType::WARNING,
			"makensis not found — diagnostics unavailable",
		),
	}
}

fn run_compiler_diagnostics(
	client: &impl Client,
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

	client.publish_diagnostics(uri.clone(), all_diagnostics);
}

// ── Formatting ──

fn handle_formatting(
	workspace: &Workspace,
	params: DocumentFormattingParams,
	state: &LspState,
) -> Result<Vec<TextEdit>, (lsp_types::Uri, String)> {
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

// ── Hover ──

fn handle_hover(workspace: &Workspace, params: HoverParams) -> Option<Hover> {
	let at = params.text_document_position_params;
	let (_, word) = workspace.word_at(&at.text_document.uri, at.position)?;

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
		.map(|(var, desc)| CompletionItem {
			label: var.to_string(),
			kind: Some(CompletionItemKind::VARIABLE),
			detail: Some(desc.to_string()),
			..Default::default()
		})
		.collect()
});

fn handle_completion(
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

// ── Diagnostics ──

fn publish_format_error(client: &impl Client, uri: lsp_types::Uri, msg: &str) {
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

fn publish_diagnostics(client: &impl Client, uri: lsp_types::Uri, text: &str) {
	client.publish_diagnostics(uri, compute_diagnostics(text));
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
	workspace: &Workspace,
	params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
	let at = params.text_document_position_params;
	let (doc, word) = workspace.word_at(&at.text_document.uri, at.position)?;

	find_symbol_location(&doc.uri, &doc.index, &word)
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
	workspace: &Workspace,
	params: DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
	let doc = workspace.document(&params.text_document.uri)?;
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

fn handle_references(workspace: &Workspace, params: ReferenceParams) -> Option<Vec<Location>> {
	let at = &params.text_document_position;
	let (doc, word) = workspace.word_at(&at.text_document.uri, at.position)?;
	let bare = word.trim_start_matches('$').trim_start_matches('!');
	let kind = symbols::find_symbol_kind(&doc.index, &word)?;

	let mut locations = Vec::new();

	if params.context.include_declaration
		&& let Some(GotoDefinitionResponse::Scalar(loc)) =
			find_symbol_location(&doc.uri, &doc.index, bare)
	{
		locations.push(loc);
	}

	for other in workspace.documents() {
		for range in symbols::find_references(&other.text, bare, kind) {
			locations.push(Location {
				uri: other.uri.clone(),
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
	workspace: &Workspace,
	params: lsp_types::TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
	let pos = params.position;
	let (doc, word) = workspace.word_at(&params.text_document.uri, pos)?;
	let bare = word.trim_start_matches('$').trim_start_matches('!');

	if is_builtin(&word) {
		return None;
	}

	symbols::find_symbol_kind(&doc.index, &word)?;

	// The rename range covers the bare identifier, without the sigil `word_at`
	// keeps — renaming `$myVar` rewrites `myVar` and leaves the `$`.
	let line_str = line_at(&doc.text, pos.line)?;
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

fn handle_rename(workspace: &Workspace, params: RenameParams) -> Option<WorkspaceEdit> {
	let at = &params.text_document_position;
	let (doc, word) = workspace.word_at(&at.text_document.uri, at.position)?;
	let bare = word.trim_start_matches('$').trim_start_matches('!');
	let kind = symbols::find_symbol_kind(&doc.index, &word)?;

	if is_builtin(&word) {
		return None;
	}

	let new_name = &params.new_name;

	#[allow(clippy::mutable_key_type)]
	let mut changes: HashMap<lsp_types::Uri, Vec<TextEdit>> = HashMap::new();

	for other in workspace.documents() {
		let mut edits: Vec<TextEdit> = symbols::find_references(&other.text, bare, kind)
			.into_iter()
			.map(|range| TextEdit {
				range,
				new_text: new_name.clone(),
			})
			.collect();

		for sym in &other.index.symbols {
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
			changes.insert(other.uri.clone(), edits);
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
	workspace: &Workspace,
	params: SignatureHelpParams,
) -> Option<SignatureHelp> {
	let at = &params.text_document_position_params;
	let text = &workspace.document(&at.text_document.uri)?.text;
	let pos = at.position;
	let line_str = line_at(text, pos.line)?;
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

	// ── Test fixtures ──

	const TEST_URI: &str = "file:///test.nsi";

	fn uri(s: &str) -> lsp_types::Uri {
		s.parse().unwrap()
	}

	fn workspace_with(documents: &[(&str, &str)]) -> Workspace {
		let mut workspace = Workspace::new();
		for (uri_str, text) in documents {
			workspace.open(uri(uri_str), text.to_string());
		}
		workspace
	}

	fn position_params(
		uri_str: &str,
		line: u32,
		character: u32,
	) -> lsp_types::TextDocumentPositionParams {
		lsp_types::TextDocumentPositionParams {
			text_document: lsp_types::TextDocumentIdentifier { uri: uri(uri_str) },
			position: Position { line, character },
		}
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

	// ── Requests that span the workspace ──

	const A_URI: &str = "file:///a.nsi";
	const B_URI: &str = "file:///b.nsi";

	fn two_documents() -> Workspace {
		workspace_with(&[
			(A_URI, "!define APP_NAME \"Test\"\nDetailPrint ${APP_NAME}"),
			(B_URI, "DetailPrint ${APP_NAME}\nDetailPrint ${APP_NAME}"),
		])
	}

	fn rename_params(uri_str: &str, line: u32, character: u32, new_name: &str) -> RenameParams {
		RenameParams {
			text_document_position: position_params(uri_str, line, character),
			new_name: new_name.to_string(),
			work_done_progress_params: Default::default(),
		}
	}

	fn reference_params(uri_str: &str, line: u32, character: u32) -> ReferenceParams {
		ReferenceParams {
			text_document_position: position_params(uri_str, line, character),
			context: lsp_types::ReferenceContext {
				include_declaration: true,
			},
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
		}
	}

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

	#[test]
	fn rename_from_the_document_without_the_definition() {
		let edit = handle_rename(&two_documents(), rename_params(B_URI, 0, 15, "PRODUCT"));
		// The kind is only known where the symbol is defined.
		assert!(edit.is_none());
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
	fn goto_definition_answers_with_the_stored_uri() {
		let params = GotoDefinitionParams {
			text_document_position_params: position_params(A_URI, 1, 15),
			work_done_progress_params: Default::default(),
			partial_result_params: Default::default(),
		};

		match handle_goto_definition(&two_documents(), params) {
			Some(GotoDefinitionResponse::Scalar(loc)) => {
				assert_eq!(loc.uri, uri(A_URI));
				assert_eq!(loc.range.start.line, 0);
			}
			other => panic!("unexpected definition response: {other:?}"),
		}
	}

	#[test]
	fn requests_for_an_unopened_document_are_none() {
		let workspace = two_documents();
		assert!(handle_rename(&workspace, rename_params("file:///gone.nsi", 0, 10, "X")).is_none());
		assert!(
			handle_references(&workspace, reference_params("file:///gone.nsi", 0, 10)).is_none()
		);
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

	// ── parse_settings ──

	use serde_json::json;

	fn replaced(settings: serde_json::Value) -> InitOptions {
		match parse_settings(settings) {
			SettingsUpdate::Replace(options) => options,
			_ => panic!("expected settings to be replaced"),
		}
	}

	#[test]
	fn settings_bare_object() {
		let options = replaced(json!({
			"formatter": { "print_width": 100, "single_quote": true },
		}));

		assert_eq!(options.formatter.print_width, 100);
		assert!(options.formatter.single_quote);
	}

	#[test]
	fn settings_nsis_section() {
		let options = replaced(json!({
			"nsis": { "formatter": { "print_width": 80 } },
		}));

		assert_eq!(options.formatter.print_width, 80);
	}

	#[test]
	fn settings_omitted_options_fall_back_to_defaults() {
		let options = replaced(json!({ "formatter": { "single_quote": true } }));

		assert_eq!(options.formatter.print_width, 0);
		assert!(options.formatter.trim_empty_lines);
		assert_eq!(options.diagnostics.preprocess_mode.as_deref(), Some("ppo"));
	}

	#[test]
	fn settings_empty_object_is_all_defaults() {
		let options = replaced(json!({}));

		assert!(options.formatter.trim_empty_lines);
		assert!(options.diagnostics.enabled_on_save);
		assert_eq!(options.makensis.path, "");
	}

	#[test]
	fn settings_null_is_unavailable() {
		assert!(matches!(
			parse_settings(json!(null)),
			SettingsUpdate::Unavailable
		));
	}

	#[test]
	fn settings_null_section_is_unavailable() {
		assert!(matches!(
			parse_settings(json!({ "nsis": null })),
			SettingsUpdate::Unavailable
		));
	}

	#[test]
	fn settings_wrong_type_is_unparseable() {
		assert!(matches!(
			parse_settings(json!({ "formatter": { "print_width": "wide" } })),
			SettingsUpdate::Unparseable
		));
	}

	#[test]
	fn settings_unknown_keys_are_ignored() {
		let options = replaced(json!({
			"formatter": { "printWidth": 100 },
			"nonsense": true,
		}));

		assert_eq!(options.formatter.print_width, 0);
	}

	// ── LspState::from_options ──

	#[test]
	fn state_parses_end_of_line() {
		assert!(matches!(parse_end_of_line(Some("lf")), Some(EndOfLine::Lf)));
		assert!(matches!(
			parse_end_of_line(Some("crlf")),
			Some(EndOfLine::Crlf)
		));
		assert!(parse_end_of_line(Some("auto")).is_none());
		assert!(parse_end_of_line(None).is_none());
	}

	#[test]
	fn state_carries_formatter_options() {
		let state = LspState::from_options(replaced(json!({
			"formatter": {
				"end_of_line": "crlf",
				"print_width": 90,
				"trim_empty_lines": false,
				"single_quote": true,
			},
		})));

		assert!(matches!(state.end_of_line, Some(EndOfLine::Crlf)));
		assert_eq!(state.print_width, 90);
		assert!(!state.trim_empty_lines);
		assert!(state.single_quote);
	}

	// ── The message loop, through a recording client ──

	use client::Recorder;
	use lsp_types::notification::Notification as _;

	/// A server that has found no compiler, so nothing shells out during a test.
	fn quiet_state() -> LspState {
		let mut state = LspState::from_options(InitOptions::default());
		state.makensis_path = None;
		state
	}

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
		assert_eq!(doc.index.symbols.len(), 1);

		let published = client.diagnostics();
		assert_eq!(published.len(), 1);
		assert_eq!(published[0].uri, uri(TEST_URI));
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

		assert_eq!(
			workspace.document(&uri(TEST_URI)).unwrap().text,
			"Function myFunc\nFunctionEnd"
		);
		assert_eq!(client.diagnostics().len(), 2);
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
