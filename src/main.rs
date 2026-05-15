mod compiler;
mod diagnostics;
mod nsis_data;

use std::collections::HashMap;
use std::sync::LazyLock;

use ardent::{DentOptions, Formatter};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
	CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
	Diagnostic, DiagnosticSeverity, DocumentFormattingParams, GotoDefinitionParams,
	GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability, Location,
	MarkupContent, MarkupKind, MessageType, OneOf, Position, PublishDiagnosticsParams, Range,
	ServerCapabilities, ShowMessageParams, TextDocumentSyncCapability, TextDocumentSyncKind,
	TextEdit,
	notification::{
		DidChangeTextDocument, DidOpenTextDocument, DidSaveTextDocument, Notification as _,
		PublishDiagnostics, ShowMessage,
	},
	request::{Completion, Formatting, GotoDefinition, HoverRequest, Request as _},
};
use serde::Deserialize;

use compiler::PreprocessMode;

#[derive(Debug, Default, Deserialize)]
struct InitOptions {
	#[serde(default)]
	diagnostics: DiagnosticsOptions,
	#[serde(default)]
	makensis: MakensisOptions,
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

	let state = LspState {
		makensis_path: compiler::find_makensis(&options.makensis.path),
		preprocess_mode: PreprocessMode::from_option(
			options.diagnostics.preprocess_mode.as_deref(),
		),
		diagnostics_on_save: options.diagnostics.enabled_on_save,
	};

	let mut documents: HashMap<String, String> = HashMap::new();

	for msg in &connection.receiver {
		match msg {
			Message::Request(req) => {
				if connection.handle_shutdown(&req).unwrap_or(true) {
					break;
				}
				handle_request(&connection, req, &documents);
			}
			Message::Notification(not) => {
				handle_notification(&connection, not, &mut documents, &state);
			}
			Message::Response(_) => {}
		}
	}

	io_threads.join().ok();
}

fn handle_request(connection: &Connection, req: Request, documents: &HashMap<String, String>) {
	match req.method.as_str() {
		Formatting::METHOD => {
			if let Ok((id, params)) = req.extract::<DocumentFormattingParams>(Formatting::METHOD) {
				match handle_formatting(documents, params) {
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
		_ => {}
	}
}

fn handle_notification(
	connection: &Connection,
	not: Notification,
	documents: &mut HashMap<String, String>,
	state: &LspState,
) {
	if let Some(params) = cast_notification::<DidOpenTextDocument>(not.clone()) {
		let uri = params.text_document.uri;
		documents.insert(uri.to_string(), params.text_document.text.clone());
		publish_diagnostics(connection, uri, &params.text_document.text);
	} else if let Some(params) = cast_notification::<DidChangeTextDocument>(not.clone())
		&& let Some(change) = params.content_changes.into_iter().last()
	{
		let uri = params.text_document.uri;
		documents.insert(uri.to_string(), change.text.clone());
		publish_diagnostics(connection, uri, &change.text);
	} else if let Some(params) = cast_notification::<DidSaveTextDocument>(not)
		&& state.diagnostics_on_save
	{
		let uri = params.text_document.uri;
		if let Some(text) = documents.get(&uri.to_string()) {
			run_compiler_diagnostics(connection, state, &uri, text);
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
	documents: &HashMap<String, String>,
	params: DocumentFormattingParams,
) -> Result<Vec<TextEdit>, (lsp_types::Uri, String)> {
	let uri = params.text_document.uri;
	let Some(text) = documents.get(&uri.to_string()) else {
		return Ok(vec![]);
	};

	let options = DentOptions {
		use_tabs: !params.options.insert_spaces,
		indent_size: params.options.tab_size as usize,
		trim_empty_lines: true,
		end_of_lines: None,
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

fn handle_hover(documents: &HashMap<String, String>, params: HoverParams) -> Option<Hover> {
	let uri = params
		.text_document_position_params
		.text_document
		.uri
		.to_string();
	let text = documents.get(&uri)?;
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

static COMPLETION_ITEMS: LazyLock<Vec<CompletionItem>> = LazyLock::new(|| {
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

	for (var, desc) in nsis_data::BUILTIN_VARIABLES {
		let (filter_text, insert_text) = trigger_prefix_texts(var);
		items.push(CompletionItem {
			label: var.to_string(),
			kind: Some(CompletionItemKind::VARIABLE),
			filter_text,
			insert_text,
			detail: Some(desc.to_string()),
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

fn handle_completion(
	_documents: &HashMap<String, String>,
	_params: CompletionParams,
) -> Option<CompletionResponse> {
	Some(CompletionResponse::Array(COMPLETION_ITEMS.clone()))
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
	documents: &HashMap<String, String>,
	params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
	let uri = &params.text_document_position_params.text_document.uri;
	let text = documents.get(&uri.to_string())?;
	let pos = params.text_document_position_params.position;
	let word = word_at_position(text, pos.line, pos.character)?;

	for (line_num, line) in text.lines().enumerate() {
		let lower = line.trim().to_lowercase();

		if lower.starts_with("function ") {
			let name = line.trim()[9..].trim();
			if name.eq_ignore_ascii_case(&word) {
				return Some(GotoDefinitionResponse::Scalar(Location {
					uri: uri.clone(),
					range: line_range(line_num, line),
				}));
			}
		}

		if lower.starts_with("!macro ") {
			let rest = line.trim()[7..].trim();
			let macro_name = rest.split_whitespace().next().unwrap_or("");
			if macro_name.eq_ignore_ascii_case(&word) {
				return Some(GotoDefinitionResponse::Scalar(Location {
					uri: uri.clone(),
					range: line_range(line_num, line),
				}));
			}
		}

		let trimmed = line.trim();
		if trimmed.ends_with(':')
			&& !trimmed.contains(' ')
			&& !trimmed.starts_with(';')
			&& !trimmed.starts_with('#')
		{
			let label = &trimmed[..trimmed.len() - 1];
			if label.eq_ignore_ascii_case(&word) {
				return Some(GotoDefinitionResponse::Scalar(Location {
					uri: uri.clone(),
					range: line_range(line_num, line),
				}));
			}
		}
	}

	None
}

fn line_range(line_num: usize, line: &str) -> Range {
	let leading = line.len() - line.trim_start().len();
	Range::new(
		Position::new(line_num as u32, byte_to_utf16_offset(line, leading)),
		Position::new(
			line_num as u32,
			byte_to_utf16_offset(line, line.trim_end().len()),
		),
	)
}

// ── Utilities ──

fn is_in_comment(text: &str, line: u32, col: usize) -> bool {
	let target = line as usize;
	let mut in_block_comment = false;

	for (i, line_str) in text.lines().enumerate() {
		if i > target && !in_block_comment {
			return false;
		}

		let bytes = line_str.as_bytes();
		let mut j = 0;

		while j < bytes.len() {
			if in_block_comment {
				if j + 1 < bytes.len() && bytes[j] == b'*' && bytes[j + 1] == b'/' {
					if i == target && col <= j + 1 {
						return true;
					}
					in_block_comment = false;
					j += 2;
					continue;
				}
				if i == target && j == col {
					return true;
				}
				j += 1;
				continue;
			}

			if bytes[j] == b'#' || bytes[j] == b';' {
				return i == target && col >= j;
			}

			if j + 1 < bytes.len() && bytes[j] == b'/' && bytes[j + 1] == b'*' {
				in_block_comment = true;
				if i == target && (col == j || col == j + 1) {
					return true;
				}
				j += 2;
				continue;
			}

			j += 1;
		}

		if i == target && !in_block_comment {
			return false;
		}
	}

	in_block_comment
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
