/// Where a position sits in an NSIS script: inside a comment, inside a quoted
/// string, or in ordinary code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxContext {
	Code,
	Comment,
	String,
}

/// Determine the syntax context at `line` / `col` (byte offset into the line).
///
/// Strings are line-local, matching how makensis parses them, and a quote only
/// opens a string at a token boundary so apostrophes inside words stay literal.
/// Comment markers are ignored inside strings, and `$\"` / `$$` escapes do not
/// terminate a string.
pub fn context_at(text: &str, line: u32, col: usize) -> SyntaxContext {
	let target = line as usize;
	let mut in_block_comment = false;

	for (i, line_str) in text.lines().enumerate() {
		if i != target {
			scan_line(line_str, &mut in_block_comment, &mut |_, _, _| {});
			continue;
		}
		let mut found = None;
		let at_end = scan_line(line_str, &mut in_block_comment, &mut |start, end, ctx| {
			if found.is_none() && col >= start && col < end {
				found = Some(ctx);
			}
		});
		// A column past the last byte takes the context the line ended in.
		return found.unwrap_or(at_end);
	}

	if in_block_comment {
		SyntaxContext::Comment
	} else {
		SyntaxContext::Code
	}
}

/// Walks a script line by line, carrying block-comment state across them, and
/// hands back the code of each line with comment text blanked out.
///
/// This is the same scan `context_at` runs, exposed for consumers that search a
/// whole line rather than ask about one position — comment text is replaced by
/// spaces rather than removed, so byte offsets into the result still address
/// the original line.
pub struct CodeScan {
	in_block_comment: bool,
}

impl CodeScan {
	pub fn new() -> Self {
		Self {
			in_block_comment: false,
		}
	}

	/// The code of `line`: every byte that belongs to a comment replaced by a
	/// space. String contents are kept — `"${APP_NAME}"` is real script text.
	pub fn code_of(&mut self, line: &str) -> String {
		let mut bytes = line.as_bytes().to_vec();
		let len = bytes.len();
		scan_line(line, &mut self.in_block_comment, &mut |start, end, ctx| {
			if ctx == SyntaxContext::Comment {
				bytes[start..end.min(len)].fill(b' ');
			}
		});
		String::from_utf8(bytes).unwrap_or_else(|_| line.to_string())
	}
}

impl Default for CodeScan {
	fn default() -> Self {
		Self::new()
	}
}

/// Scan a single line, updating the block-comment state and emitting one span
/// per stretch of same-context bytes. Returns the context the line ends in.
///
/// Spans cover the whole line and only ever break on ASCII markers, so their
/// bounds are always char boundaries.
fn scan_line(
	line: &str,
	in_block_comment: &mut bool,
	on_span: &mut impl FnMut(usize, usize, SyntaxContext),
) -> SyntaxContext {
	let bytes = line.as_bytes();
	let mut in_string: Option<u8> = None;
	let mut span_start = 0;
	let mut span_ctx = current_context(*in_block_comment, None);
	let mut j = 0;

	while j < bytes.len() {
		let here = current_context(*in_block_comment, in_string);
		if here != span_ctx {
			if j > span_start {
				on_span(span_start, j, span_ctx);
			}
			span_start = j;
			span_ctx = here;
		}

		if *in_block_comment {
			if bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/') {
				*in_block_comment = false;
				j += 2;
			} else {
				j += 1;
			}
			continue;
		}

		if let Some(quote) = in_string {
			// `$\<c>` escapes (including `$\"`) and `$$` are string content.
			if bytes[j] == b'$' && matches!(bytes.get(j + 1), Some(b'\\')) {
				j += 3;
				continue;
			}
			if bytes[j] == b'$' && matches!(bytes.get(j + 1), Some(b'$')) {
				j += 2;
				continue;
			}
			if bytes[j] == quote {
				in_string = None;
			}
			j += 1;
			continue;
		}

		if bytes[j] == b';' || bytes[j] == b'#' {
			// Rest of the line is a comment.
			if j > span_start {
				on_span(span_start, j, span_ctx);
			}
			on_span(j, bytes.len(), SyntaxContext::Comment);
			return SyntaxContext::Comment;
		}

		if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
			if j > span_start {
				on_span(span_start, j, span_ctx);
			}
			span_start = j;
			span_ctx = SyntaxContext::Comment;
			*in_block_comment = true;
			j += 2;
			continue;
		}

		if is_quote(bytes[j]) && is_token_boundary(bytes, j) {
			in_string = Some(bytes[j]);
		}
		j += 1;
	}

	if bytes.len() > span_start {
		on_span(span_start, bytes.len(), span_ctx);
	}
	current_context(*in_block_comment, in_string)
}

fn current_context(in_block_comment: bool, in_string: Option<u8>) -> SyntaxContext {
	if in_block_comment {
		SyntaxContext::Comment
	} else if in_string.is_some() {
		SyntaxContext::String
	} else {
		SyntaxContext::Code
	}
}

fn is_quote(b: u8) -> bool {
	b == b'"' || b == b'\'' || b == b'`'
}

fn is_token_boundary(bytes: &[u8], j: usize) -> bool {
	j == 0 || bytes[j - 1].is_ascii_whitespace()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn plain_code() {
		assert_eq!(context_at("Section main", 0, 3), SyntaxContext::Code);
	}

	#[test]
	fn inside_double_quoted_string() {
		let text = r#"MessageBox MB_OK "hello world""#;
		assert_eq!(context_at(text, 0, 20), SyntaxContext::String);
	}

	#[test]
	fn after_closing_quote_is_code() {
		let text = r#"MessageBox MB_OK "hi" "#;
		assert_eq!(context_at(text, 0, 22), SyntaxContext::Code);
	}

	#[test]
	fn single_and_backtick_strings() {
		assert_eq!(context_at("DetailPrint 'hi'", 0, 14), SyntaxContext::String);
		assert_eq!(context_at("DetailPrint `hi`", 0, 14), SyntaxContext::String);
	}

	#[test]
	fn apostrophe_inside_word_is_not_a_string() {
		let text = "DetailPrint dont't";
		assert_eq!(context_at(text, 0, 17), SyntaxContext::Code);
	}

	#[test]
	fn escaped_quote_does_not_close_string() {
		let text = r#"DetailPrint "a $\" b" "#;
		assert_eq!(context_at(text, 0, 19), SyntaxContext::String);
		assert_eq!(context_at(text, 0, 21), SyntaxContext::Code);
	}

	#[test]
	fn comment_marker_inside_string_is_not_a_comment() {
		let text = r#"DetailPrint "a ; b # c""#;
		assert_eq!(context_at(text, 0, 20), SyntaxContext::String);
	}

	#[test]
	fn quote_inside_comment_does_not_open_string() {
		let text = "; say \"hi\"\nSection main";
		assert_eq!(context_at(text, 0, 8), SyntaxContext::Comment);
		assert_eq!(context_at(text, 1, 3), SyntaxContext::Code);
	}

	#[test]
	fn line_comments() {
		assert_eq!(context_at("# a comment", 0, 5), SyntaxContext::Comment);
		assert_eq!(context_at("; a comment", 0, 5), SyntaxContext::Comment);
	}

	#[test]
	fn block_comment_spans_lines() {
		let text = "/* comment\nstill comment */\ncode";
		assert_eq!(context_at(text, 1, 2), SyntaxContext::Comment);
		assert_eq!(context_at(text, 2, 0), SyntaxContext::Code);
	}

	#[test]
	fn string_does_not_leak_to_next_line() {
		let text = "DetailPrint \"unterminated\nSection main";
		assert_eq!(context_at(text, 1, 3), SyntaxContext::Code);
	}

	#[test]
	fn end_of_line_inside_string() {
		let text = "DetailPrint \"abc";
		assert_eq!(context_at(text, 0, 16), SyntaxContext::String);
	}

	// ── CodeScan ──

	/// Every code view has the same byte length as its line, so offsets found
	/// in one address the other.
	fn code_of(line: &str) -> String {
		let out = CodeScan::new().code_of(line);
		assert_eq!(out.len(), line.len());
		out
	}

	#[test]
	fn code_keeps_a_line_without_comments() {
		assert_eq!(
			code_of("DetailPrint ${APP_NAME}"),
			"DetailPrint ${APP_NAME}"
		);
	}

	#[test]
	fn code_blanks_a_trailing_comment() {
		assert_eq!(
			code_of("DetailPrint hi ; see ${APP_NAME}"),
			"DetailPrint hi                  "
		);
	}

	#[test]
	fn code_blanks_a_whole_line_comment() {
		assert_eq!(code_of("# Call myFunc").trim(), "");
		assert_eq!(code_of("  ; Var notReal").trim(), "");
	}

	#[test]
	fn code_keeps_string_contents() {
		assert_eq!(code_of(r#"DetailPrint "a ; b""#), r#"DetailPrint "a ; b""#);
	}

	#[test]
	fn code_blanks_an_inline_block_comment() {
		assert_eq!(
			code_of("Function /* aside */ myFunc"),
			"Function             myFunc"
		);
	}

	#[test]
	fn code_carries_block_comments_across_lines() {
		let mut scan = CodeScan::new();
		assert_eq!(scan.code_of("/* Function fake").trim(), "");
		assert_eq!(scan.code_of("Var notReal */").trim(), "");
		assert_eq!(scan.code_of("Function real"), "Function real");
	}

	#[test]
	fn code_survives_multibyte_comment_text() {
		let line = "DetailPrint hi ; ünïcode";
		let out = code_of(line);
		assert_eq!(out.trim(), "DetailPrint hi");
	}
}
