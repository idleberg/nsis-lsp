use lsp_types::{Position, Range, SymbolKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NsisSymbolKind {
	Function,
	Macro,
	Section,
	Label,
	Variable,
	Define,
}

impl NsisSymbolKind {
	pub fn to_lsp(self) -> SymbolKind {
		match self {
			Self::Function => SymbolKind::FUNCTION,
			Self::Macro => SymbolKind::FUNCTION,
			Self::Section => SymbolKind::MODULE,
			Self::Label => SymbolKind::KEY,
			Self::Variable => SymbolKind::VARIABLE,
			Self::Define => SymbolKind::CONSTANT,
		}
	}
}

#[derive(Debug, Clone)]
pub struct SymbolDef {
	pub name: String,
	pub kind: NsisSymbolKind,
	pub range: Range,
	pub selection_range: Range,
	pub children: Vec<SymbolDef>,
}

pub struct DocumentIndex {
	pub symbols: Vec<SymbolDef>,
}

pub fn index_document(text: &str) -> DocumentIndex {
	let mut symbols: Vec<SymbolDef> = Vec::new();
	let mut container: Option<ContainerState> = None;
	let mut in_block_comment = false;

	for (line_num, line) in text.lines().enumerate() {
		let line_n = line_num as u32;
		let bytes = line.as_bytes();

		// Track block comments
		let (trimmed, still_in_block) = skip_comment_prefix(bytes, in_block_comment);
		in_block_comment = still_in_block;
		if trimmed.is_empty() {
			continue;
		}

		let lower = trimmed.to_lowercase();

		// Close containers
		if lower == "functionend" || lower == "sectionend" || lower == "!macroend" {
			if let Some(mut cs) = container.take() {
				cs.symbol.range.end = line_end_position(line_n, line);
				symbols.push(cs.symbol);
			}
			continue;
		}

		// Function <name>
		if let Some(rest) = strip_keyword(&lower, "function ") {
			let name = first_token(&trimmed[lower.len() - rest.len()..]);
			if !name.is_empty() {
				close_container(&mut container, &mut symbols, line_n);
				let sel = name_range(line_n, line, &name);
				container = Some(ContainerState {
					symbol: SymbolDef {
						name: name.to_string(),
						kind: NsisSymbolKind::Function,
						range: Range::new(
							line_start_position(line_n, line),
							Position::new(line_n, 0),
						),
						selection_range: sel,
						children: Vec::new(),
					},
				});
			}
			continue;
		}

		// !macro <name>
		if let Some(rest) = strip_keyword(&lower, "!macro ") {
			let name = first_token(&trimmed[lower.len() - rest.len()..]);
			if !name.is_empty() {
				close_container(&mut container, &mut symbols, line_n);
				let sel = name_range(line_n, line, &name);
				container = Some(ContainerState {
					symbol: SymbolDef {
						name: name.to_string(),
						kind: NsisSymbolKind::Macro,
						range: Range::new(
							line_start_position(line_n, line),
							Position::new(line_n, 0),
						),
						selection_range: sel,
						children: Vec::new(),
					},
				});
			}
			continue;
		}

		// Section ["Display Name"] [section_id]
		if let Some(rest) = strip_keyword(&lower, "section ") {
			let after_keyword = &trimmed[lower.len() - rest.len()..];
			let (name, _section_id) = parse_section_name(after_keyword);
			if !name.is_empty() {
				close_container(&mut container, &mut symbols, line_n);
				let sel = name_range(line_n, line, &name);
				container = Some(ContainerState {
					symbol: SymbolDef {
						name: name.to_string(),
						kind: NsisSymbolKind::Section,
						range: Range::new(
							line_start_position(line_n, line),
							Position::new(line_n, 0),
						),
						selection_range: sel,
						children: Vec::new(),
					},
				});
			}
			continue;
		}

		// Bare "Section" with no name (e.g. unnamed section)
		if lower == "section" {
			close_container(&mut container, &mut symbols, line_n);
			let sel = name_range(line_n, line, "Section");
			container = Some(ContainerState {
				symbol: SymbolDef {
					name: String::new(),
					kind: NsisSymbolKind::Section,
					range: Range::new(line_start_position(line_n, line), Position::new(line_n, 0)),
					selection_range: sel,
					children: Vec::new(),
				},
			});
			continue;
		}

		// Var [/GLOBAL] <name>
		if let Some(rest) = strip_keyword(&lower, "var ") {
			let after = &trimmed[lower.len() - rest.len()..];
			let name = if after.starts_with("/GLOBAL ") || after.starts_with("/global ") {
				first_token(&after[8..])
			} else {
				first_token(after)
			};
			if !name.is_empty() {
				let sel = name_range(line_n, line, &name);
				let sym = SymbolDef {
					name: name.to_string(),
					kind: NsisSymbolKind::Variable,
					range: make_line_range(line_n, line),
					selection_range: sel,
					children: Vec::new(),
				};
				push_symbol(&mut container, &mut symbols, sym);
			}
			continue;
		}

		// !define <name>
		if let Some(rest) = strip_keyword(&lower, "!define ") {
			let after = &trimmed[lower.len() - rest.len()..];
			let name = first_token(after);
			if !name.is_empty() && !name.starts_with('/') {
				let sel = name_range(line_n, line, &name);
				let sym = SymbolDef {
					name: name.to_string(),
					kind: NsisSymbolKind::Define,
					range: make_line_range(line_n, line),
					selection_range: sel,
					children: Vec::new(),
				};
				push_symbol(&mut container, &mut symbols, sym);
			}
			continue;
		}

		// Labels: <name>: (no spaces, not a comment)
		if trimmed.ends_with(':')
			&& !trimmed.contains(' ')
			&& !trimmed.starts_with(';')
			&& !trimmed.starts_with('#')
			&& trimmed.len() > 1
		{
			let label = &trimmed[..trimmed.len() - 1];
			let sel = name_range(line_n, line, label);
			let sym = SymbolDef {
				name: label.to_string(),
				kind: NsisSymbolKind::Label,
				range: make_line_range(line_n, line),
				selection_range: sel,
				children: Vec::new(),
			};
			if let Some(cs) = &mut container {
				cs.symbol.children.push(sym);
			} else {
				symbols.push(sym);
			}
			continue;
		}
	}

	// Close any unclosed container at end of file
	if let Some(mut cs) = container {
		let last_line = text.lines().count().saturating_sub(1) as u32;
		let last_text = text.lines().last().unwrap_or("");
		cs.symbol.range.end = line_end_position(last_line, last_text);
		symbols.push(cs.symbol);
	}

	DocumentIndex { symbols }
}

struct ContainerState {
	symbol: SymbolDef,
}

fn close_container(
	container: &mut Option<ContainerState>,
	symbols: &mut Vec<SymbolDef>,
	current_line: u32,
) {
	if let Some(mut cs) = container.take() {
		cs.symbol.range.end = Position::new(current_line.saturating_sub(1), 0);
		symbols.push(cs.symbol);
	}
}

fn push_symbol(
	container: &mut Option<ContainerState>,
	symbols: &mut Vec<SymbolDef>,
	sym: SymbolDef,
) {
	if let Some(cs) = container {
		cs.symbol.children.push(sym);
	} else {
		symbols.push(sym);
	}
}

fn strip_keyword<'a>(lower: &'a str, keyword: &str) -> Option<&'a str> {
	lower.strip_prefix(keyword)
}

fn first_token(s: &str) -> String {
	s.split_whitespace().next().unwrap_or("").to_string()
}

fn parse_section_name(after_keyword: &str) -> (String, Option<String>) {
	let s = after_keyword.trim();
	if let Some(stripped) = s.strip_prefix('"')
		&& let Some(end_quote) = stripped.find('"')
	{
		let name = stripped[..end_quote].to_string();
		let rest = stripped[end_quote + 1..].trim();
		let section_id = if rest.is_empty() {
			None
		} else {
			Some(first_token(rest))
		};
		return (name, section_id);
	}
	// Skip flags like /e, then re-parse
	let token = first_token(s);
	if token.starts_with('/') {
		let rest = s[token.len()..].trim();
		return parse_section_name(rest);
	}
	(token, None)
}

fn byte_to_utf16(line: &str, byte_offset: usize) -> u32 {
	let mut count = 0u32;
	for ch in line[..byte_offset].chars() {
		count += ch.len_utf16() as u32;
	}
	count
}

fn name_range(line_num: u32, line: &str, name: &str) -> Range {
	if let Some(byte_start) = line.find(name) {
		let start = byte_to_utf16(line, byte_start);
		let end = byte_to_utf16(line, byte_start + name.len());
		Range::new(Position::new(line_num, start), Position::new(line_num, end))
	} else {
		make_line_range(line_num, line)
	}
}

fn make_line_range(line_num: u32, line: &str) -> Range {
	let leading = line.len() - line.trim_start().len();
	let trailing = line.trim_end().len();
	Range::new(
		Position::new(line_num, byte_to_utf16(line, leading)),
		Position::new(line_num, byte_to_utf16(line, trailing)),
	)
}

fn line_start_position(line_num: u32, line: &str) -> Position {
	let leading = line.len() - line.trim_start().len();
	Position::new(line_num, byte_to_utf16(line, leading))
}

fn line_end_position(line_num: u32, line: &str) -> Position {
	Position::new(line_num, byte_to_utf16(line, line.trim_end().len()))
}

pub fn find_symbol_kind(index: &DocumentIndex, word: &str) -> Option<NsisSymbolKind> {
	let bare = word.trim_start_matches('$').trim_start_matches('!');
	for sym in &index.symbols {
		if sym.name.eq_ignore_ascii_case(bare) || sym.name.eq_ignore_ascii_case(word) {
			return Some(sym.kind);
		}
		for child in &sym.children {
			if child.name.eq_ignore_ascii_case(bare) || child.name.eq_ignore_ascii_case(word) {
				return Some(child.kind);
			}
		}
	}
	None
}

pub fn find_references(text: &str, name: &str, kind: NsisSymbolKind) -> Vec<Range> {
	let mut refs = Vec::new();
	let mut in_block_comment = false;

	for (line_num, line) in text.lines().enumerate() {
		let line_n = line_num as u32;
		let (trimmed, still_in_block) = skip_comment_prefix(line.as_bytes(), in_block_comment);
		in_block_comment = still_in_block;
		if trimmed.is_empty() {
			continue;
		}

		match kind {
			NsisSymbolKind::Function => {
				find_call_refs(line, line_n, name, &mut refs);
			}
			NsisSymbolKind::Macro => {
				find_insertmacro_refs(line, line_n, name, &mut refs);
				find_deref_refs(line, line_n, name, &mut refs);
			}
			NsisSymbolKind::Variable => {
				find_variable_refs(line, line_n, name, &mut refs);
			}
			NsisSymbolKind::Define => {
				find_deref_refs(line, line_n, name, &mut refs);
			}
			NsisSymbolKind::Label => {
				find_goto_refs(line, line_n, name, &mut refs);
				find_label_jump_refs(line, line_n, name, &mut refs);
			}
			NsisSymbolKind::Section => {}
		}
	}

	refs
}

fn find_call_refs(line: &str, line_n: u32, name: &str, refs: &mut Vec<Range>) {
	let lower = line.trim().to_lowercase();
	if let Some(rest) = lower.strip_prefix("call ") {
		let callee = rest.trim();
		if callee.eq_ignore_ascii_case(name)
			&& let Some(pos) = line.to_lowercase().rfind(&name.to_lowercase())
		{
			let start = byte_to_utf16(line, pos);
			let end = byte_to_utf16(line, pos + name.len());
			refs.push(Range::new(
				Position::new(line_n, start),
				Position::new(line_n, end),
			));
		}
	}
}

fn find_insertmacro_refs(line: &str, line_n: u32, name: &str, refs: &mut Vec<Range>) {
	let lower = line.trim().to_lowercase();
	if let Some(rest) = lower.strip_prefix("!insertmacro ") {
		let macro_name = rest.split_whitespace().next().unwrap_or("");
		if macro_name.eq_ignore_ascii_case(name) {
			let lower_line = line.to_lowercase();
			let lower_name = name.to_lowercase();
			if let Some(im_pos) = lower_line.find("!insertmacro") {
				let after = im_pos + "!insertmacro".len();
				if let Some(rel) = lower_line[after..].find(&lower_name) {
					let pos = after + rel;
					let start = byte_to_utf16(line, pos);
					let end = byte_to_utf16(line, pos + name.len());
					refs.push(Range::new(
						Position::new(line_n, start),
						Position::new(line_n, end),
					));
				}
			}
		}
	}
}

fn find_deref_refs(line: &str, line_n: u32, name: &str, refs: &mut Vec<Range>) {
	let lower_line = line.to_lowercase();
	let pattern = format!("${{{}}}", name.to_lowercase());
	let name_offset = 2; // skip "${"
	let mut search_from = 0;
	while let Some(pos) = lower_line[search_from..].find(&pattern) {
		let abs_pos = search_from + pos + name_offset;
		let start = byte_to_utf16(line, abs_pos);
		let end = byte_to_utf16(line, abs_pos + name.len());
		refs.push(Range::new(
			Position::new(line_n, start),
			Position::new(line_n, end),
		));
		search_from += pos + pattern.len();
	}
}

fn find_variable_refs(line: &str, line_n: u32, name: &str, refs: &mut Vec<Range>) {
	let lower_line = line.to_lowercase();
	let pattern = format!("${}", name.to_lowercase());
	let name_offset = 1; // skip "$"
	let mut search_from = 0;
	while let Some(pos) = lower_line[search_from..].find(&pattern) {
		let abs_pos = search_from + pos;
		let after = abs_pos + pattern.len();
		// Check it's not ${name} (that's a define/macro deref)
		if abs_pos + 1 < line.len() && line.as_bytes()[abs_pos + 1] == b'{' {
			search_from = after;
			continue;
		}
		// Check word boundary after
		if after < line.len() && is_ident_byte(line.as_bytes()[after]) {
			search_from = after;
			continue;
		}
		let start = byte_to_utf16(line, abs_pos + name_offset);
		let end = byte_to_utf16(line, after);
		refs.push(Range::new(
			Position::new(line_n, start),
			Position::new(line_n, end),
		));
		search_from = after;
	}
}

fn find_goto_refs(line: &str, line_n: u32, name: &str, refs: &mut Vec<Range>) {
	let lower = line.trim().to_lowercase();
	if let Some(rest) = lower.strip_prefix("goto ") {
		let target = rest.trim();
		if target.eq_ignore_ascii_case(name)
			&& let Some(pos) = line.to_lowercase().rfind(&name.to_lowercase())
		{
			let start = byte_to_utf16(line, pos);
			let end = byte_to_utf16(line, pos + name.len());
			refs.push(Range::new(
				Position::new(line_n, start),
				Position::new(line_n, end),
			));
		}
	}
}

fn find_label_jump_refs(line: &str, line_n: u32, name: &str, refs: &mut Vec<Range>) {
	let lower_line = line.to_lowercase();
	let lower_name = name.to_lowercase();
	let words: Vec<&str> = line.split_whitespace().collect();
	for (i, word) in words.iter().enumerate() {
		if i == 0 {
			continue;
		}
		let w_lower = word.to_lowercase();
		if matches!(
			w_lower.as_str(),
			"idyes" | "idno" | "idok" | "idcancel" | "idabort" | "idretry" | "idignore"
		) && let Some(next) = words.get(i + 1)
			&& next.eq_ignore_ascii_case(name)
			&& let Some(pos) = lower_line.rfind(&lower_name)
		{
			let start = byte_to_utf16(line, pos);
			let end = byte_to_utf16(line, pos + name.len());
			refs.push(Range::new(
				Position::new(line_n, start),
				Position::new(line_n, end),
			));
		}
	}
}

fn is_ident_byte(b: u8) -> bool {
	b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

fn skip_comment_prefix(bytes: &[u8], mut in_block: bool) -> (String, bool) {
	let mut i = 0;
	// Skip leading whitespace
	while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
		i += 1;
	}

	if in_block {
		// Look for end of block comment
		while i + 1 < bytes.len() {
			if bytes[i] == b'*' && bytes[i + 1] == b'/' {
				in_block = false;
				i += 2;
				// Skip whitespace after block comment end
				while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
					i += 1;
				}
				break;
			}
			i += 1;
		}
		if in_block {
			return (String::new(), true);
		}
		if i >= bytes.len() {
			return (String::new(), false);
		}
	}

	// Check for line comments
	if i < bytes.len() && (bytes[i] == b'#' || bytes[i] == b';') {
		return (String::new(), false);
	}

	// Check for block comment start
	if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
		return (String::new(), true);
	}

	let result = std::str::from_utf8(&bytes[i..])
		.unwrap_or("")
		.trim_end()
		.to_string();
	(result, false)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn index_function_with_labels() {
		let text = "Function myFunc\n  label1:\n  label2:\nFunctionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "myFunc");
		assert_eq!(idx.symbols[0].kind, NsisSymbolKind::Function);
		assert_eq!(idx.symbols[0].children.len(), 2);
		assert_eq!(idx.symbols[0].children[0].name, "label1");
		assert_eq!(idx.symbols[0].children[0].kind, NsisSymbolKind::Label);
		assert_eq!(idx.symbols[0].children[1].name, "label2");
	}

	#[test]
	fn index_macro() {
		let text = "!macro MyMacro arg1 arg2\n  DetailPrint hello\n!macroend";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "MyMacro");
		assert_eq!(idx.symbols[0].kind, NsisSymbolKind::Macro);
	}

	#[test]
	fn index_section_with_display_name() {
		let text = "Section \"Install Files\" sec_install\n  File app.exe\nSectionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "Install Files");
		assert_eq!(idx.symbols[0].kind, NsisSymbolKind::Section);
	}

	#[test]
	fn index_section_unquoted() {
		let text = "Section main\nSectionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "main");
	}

	#[test]
	fn index_section_with_flag() {
		let text = "Section /e \"Optional\" sec_opt\nSectionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		// /e flag should be handled, extracting the name after the flag
		assert!(!idx.symbols[0].name.starts_with('/'));
	}

	#[test]
	fn index_variable() {
		let text = "Var myVar\nVar /GLOBAL otherVar";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 2);
		assert_eq!(idx.symbols[0].name, "myVar");
		assert_eq!(idx.symbols[0].kind, NsisSymbolKind::Variable);
		assert_eq!(idx.symbols[1].name, "otherVar");
	}

	#[test]
	fn index_define() {
		let text = "!define APP_NAME \"MyApp\"\n!define VERSION 1.0";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 2);
		assert_eq!(idx.symbols[0].name, "APP_NAME");
		assert_eq!(idx.symbols[0].kind, NsisSymbolKind::Define);
		assert_eq!(idx.symbols[1].name, "VERSION");
	}

	#[test]
	fn index_standalone_label() {
		let text = "start:\n  DetailPrint hello";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "start");
		assert_eq!(idx.symbols[0].kind, NsisSymbolKind::Label);
	}

	#[test]
	fn index_empty_file() {
		let idx = index_document("");
		assert!(idx.symbols.is_empty());
	}

	#[test]
	fn index_skips_comments() {
		let text = "# Function fake\n; Var notReal\nFunction real\nFunctionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "real");
	}

	#[test]
	fn index_skips_block_comments() {
		let text = "/* Function fake\nVar notReal */\nFunction real\nFunctionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "real");
	}

	#[test]
	fn index_callback_function() {
		let text = "Function .onInit\n  Abort\nFunctionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, ".onInit");
		assert_eq!(idx.symbols[0].kind, NsisSymbolKind::Function);
	}

	#[test]
	fn index_mixed_symbols() {
		let text = "\
!define APP_NAME \"Test\"
Var myVar

Function .onInit
  start:
  DetailPrint $myVar
FunctionEnd

Section \"Files\"
SectionEnd

!macro Helper
!macroend";
		let idx = index_document(text);
		let names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
		assert_eq!(
			names,
			vec!["APP_NAME", "myVar", ".onInit", "Files", "Helper"]
		);
		assert_eq!(idx.symbols[2].children.len(), 1);
		assert_eq!(idx.symbols[2].children[0].name, "start");
	}

	#[test]
	fn index_unclosed_function() {
		let text = "Function unclosed\n  label1:";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "unclosed");
		assert_eq!(idx.symbols[0].children.len(), 1);
	}

	#[test]
	fn index_section_flag_e() {
		let text = "Section /e \"Optional Section\"\nSectionEnd";
		let idx = index_document(text);
		assert_eq!(idx.symbols.len(), 1);
		assert_eq!(idx.symbols[0].name, "Optional Section");
	}

	#[test]
	fn selection_range_covers_name_only() {
		let text = "Function myFunc\nFunctionEnd";
		let idx = index_document(text);
		let sel = idx.symbols[0].selection_range;
		assert_eq!(sel.start.character, 9); // "Function " = 9 chars
		assert_eq!(sel.end.character, 15); // "myFunc" = 6 chars
	}

	#[test]
	fn define_skips_flags() {
		let text = "!define /date NOW";
		let idx = index_document(text);
		assert!(idx.symbols.is_empty() || idx.symbols[0].name != "/date");
	}

	#[test]
	fn parse_section_name_quoted() {
		let (name, id) = parse_section_name("\"My Section\" sec_id");
		assert_eq!(name, "My Section");
		assert_eq!(id, Some("sec_id".to_string()));
	}

	#[test]
	fn parse_section_name_unquoted() {
		let (name, id) = parse_section_name("main");
		assert_eq!(name, "main");
		assert_eq!(id, None);
	}

	#[test]
	fn parse_section_name_with_flag() {
		let (name, _) = parse_section_name("/e \"Expanded\"");
		assert_eq!(name, "Expanded");
	}

	// ── find_symbol_kind ──

	#[test]
	fn find_symbol_kind_function() {
		let idx = index_document("Function myFunc\nFunctionEnd");
		assert_eq!(
			find_symbol_kind(&idx, "myFunc"),
			Some(NsisSymbolKind::Function)
		);
	}

	#[test]
	fn find_symbol_kind_child_label() {
		let idx = index_document("Function myFunc\n  start:\nFunctionEnd");
		assert_eq!(find_symbol_kind(&idx, "start"), Some(NsisSymbolKind::Label));
	}

	#[test]
	fn find_symbol_kind_not_found() {
		let idx = index_document("Function myFunc\nFunctionEnd");
		assert_eq!(find_symbol_kind(&idx, "missing"), None);
	}

	#[test]
	fn find_symbol_kind_with_dollar_prefix() {
		let idx = index_document("Var myVar");
		assert_eq!(
			find_symbol_kind(&idx, "$myVar"),
			Some(NsisSymbolKind::Variable)
		);
	}

	// ── find_references ──

	#[test]
	fn refs_function_call() {
		let text = "Function myFunc\nFunctionEnd\nCall myFunc";
		let refs = find_references(text, "myFunc", NsisSymbolKind::Function);
		assert_eq!(refs.len(), 1);
		assert_eq!(refs[0].start.line, 2);
	}

	#[test]
	fn refs_function_call_case_insensitive() {
		let text = "call MYFUNC";
		let refs = find_references(text, "myFunc", NsisSymbolKind::Function);
		assert_eq!(refs.len(), 1);
	}

	#[test]
	fn refs_macro_insertmacro() {
		let text = "!insertmacro MyMacro arg1";
		let refs = find_references(text, "MyMacro", NsisSymbolKind::Macro);
		assert_eq!(refs.len(), 1);
	}

	#[test]
	fn refs_macro_deref() {
		let text = "DetailPrint ${MyMacro}";
		let refs = find_references(text, "MyMacro", NsisSymbolKind::Macro);
		assert_eq!(refs.len(), 1);
	}

	#[test]
	fn refs_variable() {
		let text = "StrCpy $myVar \"hello\"\nDetailPrint $myVar";
		let refs = find_references(text, "myVar", NsisSymbolKind::Variable);
		assert_eq!(refs.len(), 2);
	}

	#[test]
	fn refs_variable_not_deref() {
		let text = "DetailPrint ${myVar}";
		let refs = find_references(text, "myVar", NsisSymbolKind::Variable);
		assert_eq!(refs.is_empty(), true);
	}

	#[test]
	fn refs_define_deref() {
		let text = "DetailPrint ${APP_NAME}\nStrCpy $0 ${APP_NAME}";
		let refs = find_references(text, "APP_NAME", NsisSymbolKind::Define);
		assert_eq!(refs.len(), 2);
	}

	#[test]
	fn refs_label_goto() {
		let text = "Goto myLabel";
		let refs = find_references(text, "myLabel", NsisSymbolKind::Label);
		assert_eq!(refs.len(), 1);
	}

	#[test]
	fn refs_label_messagebox_jump() {
		let text = "MessageBox MB_YESNO \"Continue?\" IDYES accept";
		let refs = find_references(text, "accept", NsisSymbolKind::Label);
		assert_eq!(refs.len(), 1);
	}

	#[test]
	fn refs_skips_comments() {
		let text = "# Call myFunc\nCall myFunc";
		let refs = find_references(text, "myFunc", NsisSymbolKind::Function);
		assert_eq!(refs.len(), 1);
		assert_eq!(refs[0].start.line, 1);
	}

	#[test]
	fn refs_empty_for_section() {
		let text = "Section main\nSectionEnd";
		let refs = find_references(text, "main", NsisSymbolKind::Section);
		assert!(refs.is_empty());
	}
}
