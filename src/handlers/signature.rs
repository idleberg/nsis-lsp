//! The parameter list of the command being typed, and which parameter the
//! cursor is on.
//!
//! The syntax comes from the docs as one string — `MessageBox mb_option_list
//! messagebox_text [/SD ret] [ret label]` — so the split into parameters and
//! the count of the arguments already typed are both done here, over the raw
//! line.

use lsp_types::{
	ParameterInformation, ParameterLabel, SignatureHelp, SignatureHelpParams, SignatureInformation,
};

use crate::context::{self, SyntaxContext};
use crate::nsis_data::{self, Known};
use crate::position::{line_at, utf16_to_byte_offset};
use crate::workspace::Workspace;

pub fn handle_signature_help(
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
	let Known::Command(entry) = nsis_data::lookup(command)? else {
		return None;
	};
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

fn is_in_comment(text: &str, line: u32, col: usize) -> bool {
	context::context_at(text, line, col) == SyntaxContext::Comment
}

#[cfg(test)]
mod tests {
	use super::*;

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
}
