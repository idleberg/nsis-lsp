use lsp_types::{Position, Range, SymbolKind};

use crate::context::CodeScan;
use crate::position::{byte_to_utf16_offset, is_ident_char};

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
	let mut scan = CodeScan::new();
	let mut last_end = Position::new(0, 0);

	for (line_num, line) in text.lines().enumerate() {
		let line_n = line_num as u32;

		// Comments are blanked out, so only script text is matched below.
		let code = scan.code_of(line);
		last_end = line_end_position(line_n, line, &code);

		let trimmed = code.trim();
		if trimmed.is_empty() {
			continue;
		}

		let lower = trimmed.to_lowercase();

		// Close containers
		if lower == "functionend" || lower == "sectionend" || lower == "!macroend" {
			if let Some(mut cs) = container.take() {
				cs.symbol.range.end = line_end_position(line_n, line, &code);
				symbols.push(cs.symbol);
			}
			continue;
		}

		// Function <name>
		if let Some(rest) = strip_keyword(&lower, "function ") {
			let name = first_token(&trimmed[lower.len() - rest.len()..]);
			if !name.is_empty() {
				close_container(&mut container, &mut symbols, line_n);
				let sel = name_range(line_n, line, &code, &name);
				container = Some(ContainerState {
					symbol: SymbolDef {
						name: name.to_string(),
						kind: NsisSymbolKind::Function,
						range: Range::new(
							line_start_position(line_n, line, &code),
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
				let sel = name_range(line_n, line, &code, &name);
				container = Some(ContainerState {
					symbol: SymbolDef {
						name: name.to_string(),
						kind: NsisSymbolKind::Macro,
						range: Range::new(
							line_start_position(line_n, line, &code),
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
				let sel = name_range(line_n, line, &code, &name);
				container = Some(ContainerState {
					symbol: SymbolDef {
						name: name.to_string(),
						kind: NsisSymbolKind::Section,
						range: Range::new(
							line_start_position(line_n, line, &code),
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
			let sel = name_range(line_n, line, &code, "Section");
			container = Some(ContainerState {
				symbol: SymbolDef {
					name: String::new(),
					kind: NsisSymbolKind::Section,
					range: Range::new(
						line_start_position(line_n, line, &code),
						Position::new(line_n, 0),
					),
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
				let sel = name_range(line_n, line, &code, &name);
				let sym = SymbolDef {
					name: name.to_string(),
					kind: NsisSymbolKind::Variable,
					range: make_line_range(line_n, line, &code),
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
				let sel = name_range(line_n, line, &code, &name);
				let sym = SymbolDef {
					name: name.to_string(),
					kind: NsisSymbolKind::Define,
					range: make_line_range(line_n, line, &code),
					selection_range: sel,
					children: Vec::new(),
				};
				push_symbol(&mut container, &mut symbols, sym);
			}
			continue;
		}

		// Labels: <name>: (no spaces)
		if trimmed.ends_with(':') && !trimmed.contains(' ') && trimmed.len() > 1 {
			let label = &trimmed[..trimmed.len() - 1];
			let sel = name_range(line_n, line, &code, label);
			let sym = SymbolDef {
				name: label.to_string(),
				kind: NsisSymbolKind::Label,
				range: make_line_range(line_n, line, &code),
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
		cs.symbol.range.end = last_end;
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

// The four helpers below take a line twice: `code` to locate a byte offset,
// `line` to turn that offset into a UTF-16 column. Blanking a comment preserves
// byte offsets but not UTF-16 ones, so the columns must come from the raw line.

fn name_range(line_num: u32, line: &str, code: &str, name: &str) -> Range {
	if let Some(byte_start) = code.find(name) {
		let start = byte_to_utf16_offset(line, byte_start);
		let end = byte_to_utf16_offset(line, byte_start + name.len());
		Range::new(Position::new(line_num, start), Position::new(line_num, end))
	} else {
		make_line_range(line_num, line, code)
	}
}

fn make_line_range(line_num: u32, line: &str, code: &str) -> Range {
	Range::new(
		line_start_position(line_num, line, code),
		line_end_position(line_num, line, code),
	)
}

fn line_start_position(line_num: u32, line: &str, code: &str) -> Position {
	let leading = code.len() - code.trim_start().len();
	Position::new(line_num, byte_to_utf16_offset(line, leading))
}

fn line_end_position(line_num: u32, line: &str, code: &str) -> Position {
	Position::new(line_num, byte_to_utf16_offset(line, code.trim_end().len()))
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
	let mut scan = CodeScan::new();

	for (line_num, line) in text.lines().enumerate() {
		let line_n = line_num as u32;

		// Searching the code view rather than the raw line keeps a name written
		// inside a comment from being reported — and renamed.
		let code = scan.code_of(line);
		if code.trim().is_empty() {
			continue;
		}
		let at = LineRefs {
			line,
			code: &code,
			line_n,
			name,
		};

		match kind {
			NsisSymbolKind::Function => {
				at.find_call_refs(&mut refs);
			}
			NsisSymbolKind::Macro => {
				at.find_insertmacro_refs(&mut refs);
				at.find_deref_refs(&mut refs);
			}
			NsisSymbolKind::Variable => {
				at.find_variable_refs(&mut refs);
			}
			NsisSymbolKind::Define => {
				at.find_deref_refs(&mut refs);
			}
			NsisSymbolKind::Label => {
				at.find_goto_refs(&mut refs);
				at.find_label_jump_refs(&mut refs);
			}
			NsisSymbolKind::Section => {}
		}
	}

	refs
}

/// One line under search: `code` is what the scanners match against, `line` is
/// what byte offsets are turned into UTF-16 columns against.
struct LineRefs<'a> {
	line: &'a str,
	code: &'a str,
	line_n: u32,
	name: &'a str,
}

impl LineRefs<'_> {
	/// The range covering `len` bytes from `byte_offset`, in UTF-16 columns.
	fn range_at(&self, byte_offset: usize, len: usize) -> Range {
		Range::new(
			Position::new(self.line_n, byte_to_utf16_offset(self.line, byte_offset)),
			Position::new(
				self.line_n,
				byte_to_utf16_offset(self.line, byte_offset + len),
			),
		)
	}
}

impl LineRefs<'_> {
	fn find_call_refs(&self, refs: &mut Vec<Range>) {
		let lower = self.code.trim().to_lowercase();
		if let Some(rest) = lower.strip_prefix("call ") {
			let callee = rest.trim();
			if callee.eq_ignore_ascii_case(self.name)
				&& let Some(pos) = self.code.to_lowercase().rfind(&self.name.to_lowercase())
			{
				refs.push(self.range_at(pos, self.name.len()));
			}
		}
	}

	fn find_insertmacro_refs(&self, refs: &mut Vec<Range>) {
		let lower = self.code.trim().to_lowercase();
		if let Some(rest) = lower.strip_prefix("!insertmacro ") {
			let macro_name = rest.split_whitespace().next().unwrap_or("");
			if macro_name.eq_ignore_ascii_case(self.name) {
				let lower_line = self.code.to_lowercase();
				let lower_name = self.name.to_lowercase();
				if let Some(im_pos) = lower_line.find("!insertmacro") {
					let after = im_pos + "!insertmacro".len();
					if let Some(rel) = lower_line[after..].find(&lower_name) {
						refs.push(self.range_at(after + rel, self.name.len()));
					}
				}
			}
		}
	}

	fn find_deref_refs(&self, refs: &mut Vec<Range>) {
		let lower_line = self.code.to_lowercase();
		let pattern = format!("${{{}}}", self.name.to_lowercase());
		let name_offset = 2; // skip "${"
		let mut search_from = 0;
		while let Some(pos) = lower_line[search_from..].find(&pattern) {
			refs.push(self.range_at(search_from + pos + name_offset, self.name.len()));
			search_from += pos + pattern.len();
		}
	}

	fn find_variable_refs(&self, refs: &mut Vec<Range>) {
		let lower_line = self.code.to_lowercase();
		let pattern = format!("${}", self.name.to_lowercase());
		let name_offset = 1; // skip "$"
		let bytes = self.code.as_bytes();
		let mut search_from = 0;
		while let Some(pos) = lower_line[search_from..].find(&pattern) {
			let abs_pos = search_from + pos;
			let after = abs_pos + pattern.len();
			// Check it's not ${name} (that's a define/macro deref)
			if abs_pos + 1 < bytes.len() && bytes[abs_pos + 1] == b'{' {
				search_from = after;
				continue;
			}
			// Check word boundary after
			if after < bytes.len() && is_ident_char(bytes[after]) {
				search_from = after;
				continue;
			}
			refs.push(self.range_at(abs_pos + name_offset, after - abs_pos - name_offset));
			search_from = after;
		}
	}

	fn find_goto_refs(&self, refs: &mut Vec<Range>) {
		let lower = self.code.trim().to_lowercase();
		if let Some(rest) = lower.strip_prefix("goto ") {
			let target = rest.trim();
			if target.eq_ignore_ascii_case(self.name)
				&& let Some(pos) = self.code.to_lowercase().rfind(&self.name.to_lowercase())
			{
				refs.push(self.range_at(pos, self.name.len()));
			}
		}
	}

	fn find_label_jump_refs(&self, refs: &mut Vec<Range>) {
		let lower_line = self.code.to_lowercase();
		let lower_name = self.name.to_lowercase();
		let words: Vec<&str> = self.code.split_whitespace().collect();
		for (i, word) in words.iter().enumerate() {
			if i == 0 {
				continue;
			}
			let w_lower = word.to_lowercase();
			if matches!(
				w_lower.as_str(),
				"idyes" | "idno" | "idok" | "idcancel" | "idabort" | "idretry" | "idignore"
			) && let Some(next) = words.get(i + 1)
				&& next.eq_ignore_ascii_case(self.name)
				&& let Some(pos) = lower_line.rfind(&lower_name)
			{
				refs.push(self.range_at(pos, self.name.len()));
			}
		}
	}
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
	fn refs_skips_trailing_line_comments() {
		let text = "DetailPrint \"hi\" ; see ${APP_NAME} for details\nDetailPrint ${APP_NAME}";
		let refs = find_references(text, "APP_NAME", NsisSymbolKind::Define);
		assert_eq!(refs.len(), 1);
		assert_eq!(refs[0].start.line, 1);
	}

	#[test]
	fn refs_skips_trailing_comments_for_variables() {
		let text = "DetailPrint $myVar # and $myVar again";
		let refs = find_references(text, "myVar", NsisSymbolKind::Variable);
		assert_eq!(refs.len(), 1);
		assert_eq!(refs[0].start.character, 13);
	}

	#[test]
	fn refs_skips_trailing_comments_for_labels() {
		let text = "Goto done ; Goto done\nGoto done";
		let refs = find_references(text, "done", NsisSymbolKind::Label);
		assert_eq!(refs.len(), 2);
		assert_eq!(refs[0].start.line, 0);
		assert_eq!(refs[1].start.line, 1);
	}

	#[test]
	fn refs_skips_inline_block_comments() {
		let text = "DetailPrint /* ${APP_NAME} */ ${APP_NAME}";
		let refs = find_references(text, "APP_NAME", NsisSymbolKind::Define);
		assert_eq!(refs.len(), 1);
		assert_eq!(refs[0].start.character, 32);
	}

	#[test]
	fn refs_inside_a_string_still_count() {
		let text = "DetailPrint \"${APP_NAME} installed\"";
		let refs = find_references(text, "APP_NAME", NsisSymbolKind::Define);
		assert_eq!(refs.len(), 1);
	}

	#[test]
	fn refs_columns_are_utf16_after_multibyte_text() {
		let text = "DetailPrint \"ü\" ${APP_NAME}";
		let refs = find_references(text, "APP_NAME", NsisSymbolKind::Define);
		assert_eq!(refs.len(), 1);
		// "DetailPrint \"ü\" ${" is 18 UTF-16 units, 19 bytes.
		assert_eq!(refs[0].start.character, 18);
	}

	#[test]
	fn refs_empty_for_section() {
		let text = "Section main\nSectionEnd";
		let refs = find_references(text, "main", NsisSymbolKind::Section);
		assert!(refs.is_empty());
	}
}
