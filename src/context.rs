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
		let stop_at = if i == target { Some(col) } else { None };
		if let Some(ctx) = scan_line(line_str, &mut in_block_comment, stop_at) {
			return ctx;
		}
		if i == target {
			return SyntaxContext::Code;
		}
	}

	if in_block_comment {
		SyntaxContext::Comment
	} else {
		SyntaxContext::Code
	}
}

/// Scan a single line, updating the block-comment state. When `stop_at` is set,
/// returns the context at that byte offset; otherwise returns `None`.
fn scan_line(
	line: &str,
	in_block_comment: &mut bool,
	stop_at: Option<usize>,
) -> Option<SyntaxContext> {
	let bytes = line.as_bytes();
	let mut in_string: Option<u8> = None;
	let mut j = 0;

	while j < bytes.len() {
		if let Some(stop) = stop_at
			&& j >= stop
		{
			return Some(current_context(*in_block_comment, in_string));
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
			return stop_at.map(|_| SyntaxContext::Comment);
		}

		if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
			*in_block_comment = true;
			j += 2;
			continue;
		}

		if is_quote(bytes[j]) && is_token_boundary(bytes, j) {
			in_string = Some(bytes[j]);
		}
		j += 1;
	}

	stop_at.map(|_| current_context(*in_block_comment, in_string))
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
}
