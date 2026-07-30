//! Everything the server has to say about a command NSIS no longer documents:
//! where one is used, how to describe it, and how to fix it where a fix exists.
//!
//! The three renderings — the diagnostic, the hover, the quickfix — are one
//! feature seen from three sides, so they live together and read the same table.
//! A diagnostic carries the command's canonical name in [`Diagnostic::data`],
//! which is what [`fix`] reads: the message is for the user, and nothing but the
//! user parses it.

use lsp_types::{
	CodeAction, CodeActionKind, Diagnostic, DiagnosticSeverity, DiagnosticTag, NumberOrString,
	Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::context::CodeScan;
use crate::nsis_data::{self, Deprecated, Known, Replacement};
use crate::position::byte_to_utf16_offset;

/// Marks a diagnostic as this module's. [`fix`] answers for these and nothing
/// else, so another producer's warning can never be mistaken for one.
pub const CODE: &str = "deprecated-command";

const SOURCE: &str = "nsis-lsp";

/// Warn about every deprecated command used in `text`.
///
/// Reads the script through [`CodeScan`], so a command named inside a comment is
/// comment text rather than a use.
pub fn scan(text: &str) -> Vec<Diagnostic> {
	let mut scan = CodeScan::new();
	let mut diagnostics = Vec::new();

	for (line_num, raw) in text.lines().enumerate() {
		let code = scan.code_of(raw);
		let Some(word) = code.split_whitespace().next() else {
			continue;
		};
		let Some(Known::Deprecated(dep)) = nsis_data::lookup(word) else {
			continue;
		};

		// Comment bytes are blanked, not removed, so offsets into the code view
		// address the raw line — but the column the client wants counts UTF-16
		// units of what the user actually typed.
		let start = code.len() - code.trim_start().len();
		let end = start + word.len();

		diagnostics.push(Diagnostic {
			range: Range::new(
				Position::new(line_num as u32, byte_to_utf16_offset(raw, start)),
				Position::new(line_num as u32, byte_to_utf16_offset(raw, end)),
			),
			severity: Some(DiagnosticSeverity::WARNING),
			code: Some(NumberOrString::String(CODE.to_string())),
			source: Some(SOURCE.to_string()),
			message: message(dep),
			tags: Some(vec![DiagnosticTag::DEPRECATED]),
			data: Some(dep.name.into()),
			..Default::default()
		});
	}

	diagnostics
}

/// The quickfix for one diagnostic, or `None` where there is nothing to offer.
///
/// Deliberately answers for a single diagnostic: a request carrying one
/// deprecation that cannot be fixed and one that can has no way to lose the
/// second.
pub fn fix(uri: &Uri, diag: &Diagnostic) -> Option<CodeAction> {
	if diag.code != Some(NumberOrString::String(CODE.to_string())) {
		return None;
	}

	let name = diag.data.as_ref()?.as_str()?;
	let Some(Known::Deprecated(dep)) = nsis_data::lookup(name) else {
		return None;
	};
	let Replacement::Swap(new) = dep.replacement else {
		return None;
	};

	#[allow(clippy::mutable_key_type)]
	let changes = std::iter::once((
		uri.clone(),
		vec![TextEdit {
			range: diag.range,
			new_text: new.to_string(),
		}],
	))
	.collect();

	Some(CodeAction {
		title: format!("Replace with '{new}'"),
		kind: Some(CodeActionKind::QUICKFIX),
		diagnostics: Some(vec![diag.clone()]),
		edit: Some(WorkspaceEdit {
			changes: Some(changes),
			..Default::default()
		}),
		is_preferred: Some(true),
		..Default::default()
	})
}

/// The hover body for a deprecated command.
pub fn hover(dep: &Deprecated) -> String {
	let tail = match dep.replacement {
		Replacement::Swap(new) => format!("Use `{new}` instead."),
		Replacement::Advice(advice) => advice.to_string(),
		Replacement::None => "It is no longer supported.".to_string(),
	};
	format!("**{}** *(deprecated)*\n\n{tail}", dep.name)
}

fn message(dep: &Deprecated) -> String {
	let tail = match dep.replacement {
		Replacement::Swap(new) => format!("Use '{new}' instead."),
		Replacement::Advice(advice) => advice.to_string(),
		Replacement::None => "It is no longer supported.".to_string(),
	};
	format!("'{}' is deprecated. {tail}", dep.name)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn uri() -> Uri {
		"file:///test.nsi".parse().unwrap()
	}

	/// The edit a request over `text` would apply, having gone the whole way
	/// round: scan the script, hand the diagnostics back as a code-action
	/// request would, and collect what comes out.
	fn quickfixes(text: &str) -> Vec<(Range, String)> {
		scan(text)
			.iter()
			.filter_map(|diag| fix(&uri(), diag))
			.map(|action| {
				let edit = action
					.edit
					.unwrap()
					.changes
					.unwrap()
					.remove(&uri())
					.unwrap();
				(edit[0].range, edit[0].new_text.clone())
			})
			.collect()
	}

	// ── scan ──

	#[test]
	fn a_deprecated_command_is_warned_about() {
		let diags = scan("SubSection test");
		assert_eq!(diags.len(), 1);
		assert_eq!(
			diags[0].range,
			Range::new(Position::new(0, 0), Position::new(0, 10))
		);
		assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
		assert_eq!(diags[0].tags, Some(vec![DiagnosticTag::DEPRECATED]));
		assert_eq!(diags[0].code, Some(NumberOrString::String(CODE.into())));
	}

	#[test]
	fn a_current_command_is_not() {
		assert!(scan("Section main\nSectionEnd").is_empty());
	}

	#[test]
	fn the_warning_does_not_care_how_it_was_spelled() {
		let diags = scan("subsection test");
		assert_eq!(diags.len(), 1);
		assert_eq!(diags[0].data.as_ref().unwrap().as_str(), Some("SubSection"));
	}

	/// The name travels in `data`, so a client that localizes or a future
	/// rewording of the message cannot break the quickfix.
	#[test]
	fn the_canonical_name_travels_in_data() {
		let diags = scan("SUBSECTIONEND");
		assert_eq!(
			diags[0].data.as_ref().unwrap().as_str(),
			Some("SubSectionEnd")
		);
	}

	#[test]
	fn the_message_names_the_replacement() {
		assert_eq!(
			scan("SubSection x")[0].message,
			"'SubSection' is deprecated. Use 'SectionGroup' instead."
		);
		assert_eq!(
			scan("DirShow x")[0].message,
			"'DirShow' is deprecated. It does not currently work."
		);
		assert_eq!(
			scan("PackEXEHeader x")[0].message,
			"'PackEXEHeader' is deprecated. It is no longer supported."
		);
	}

	#[test]
	fn an_indented_command_is_warned_about_where_it_sits() {
		let diags = scan("\t\tSubSection test");
		assert_eq!(diags[0].range.start.character, 2);
		assert_eq!(diags[0].range.end.character, 12);
	}

	/// Columns are UTF-16 units, counted over what the user typed — not over the
	/// code view, whose blanked comment bytes are plain ASCII spaces.
	#[test]
	fn columns_count_utf16_past_a_multibyte_comment() {
		let diags = scan("/* ünïcode */ SubSection x");
		assert_eq!(diags[0].range.start.character, 14);
		assert_eq!(diags[0].range.end.character, 24);
	}

	// ── scan reads through CodeScan ──

	#[test]
	fn a_command_inside_a_block_comment_is_not_a_use() {
		assert!(scan("/* aside\nSubSection foo\n*/\n").is_empty());
	}

	#[test]
	fn a_command_inside_a_line_comment_is_not_a_use() {
		assert!(scan("; SubSection foo\n# SubSection foo").is_empty());
	}

	#[test]
	fn a_command_after_a_block_comment_closes_is_a_use() {
		assert_eq!(scan("/* aside */\nSubSection foo").len(), 1);
	}

	// ── fix ──

	#[test]
	fn a_swap_round_trips_from_scan_to_edit() {
		assert_eq!(
			quickfixes("SubSection test"),
			vec![(
				Range::new(Position::new(0, 0), Position::new(0, 10)),
				"SectionGroup".to_string()
			)]
		);
	}

	#[test]
	fn advice_and_removed_commands_offer_no_fix() {
		for dep in nsis_data::deprecated() {
			if matches!(dep.replacement, Replacement::Swap(_)) {
				continue;
			}
			assert!(
				quickfixes(&format!("{} x", dep.name)).is_empty(),
				"{} offered a fix it has no replacement for",
				dep.name
			);
		}
	}

	/// Every `Swap` in the table survives the round trip, so an entry can never
	/// be added to the table and quietly fail to produce a quickfix.
	#[test]
	fn every_swap_in_the_table_produces_its_edit() {
		for dep in nsis_data::deprecated() {
			let Replacement::Swap(new) = dep.replacement else {
				continue;
			};
			let fixes = quickfixes(&format!("{} x", dep.name));
			assert_eq!(fixes.len(), 1, "{} produced no quickfix", dep.name);
			assert_eq!(fixes[0].1, new);
		}
	}

	/// Regression: a request carrying an unfixable deprecation used to discard
	/// every quickfix already collected for it.
	#[test]
	fn an_unfixable_deprecation_does_not_drop_the_fixable_ones() {
		let fixes = quickfixes("DirShow foo\nSubSection bar\nGetParent baz");
		assert_eq!(fixes.len(), 1);
		assert_eq!(fixes[0].1, "SectionGroup");
		assert_eq!(fixes[0].0.start.line, 1);
	}

	#[test]
	fn a_diagnostic_from_elsewhere_is_not_ours_to_fix() {
		let mut foreign = scan("SubSection test").remove(0);
		foreign.code = None;
		assert!(fix(&uri(), &foreign).is_none());
	}

	#[test]
	fn a_diagnostic_with_no_data_is_not_fixed() {
		let mut damaged = scan("SubSection test").remove(0);
		damaged.data = None;
		assert!(fix(&uri(), &damaged).is_none());
	}

	// ── hover ──

	#[test]
	fn hover_renders_the_replacement() {
		for dep in nsis_data::deprecated() {
			let body = hover(dep);
			assert!(body.contains(dep.name));
			assert!(body.contains("deprecated"));
			if let Replacement::Swap(new) = dep.replacement {
				assert!(body.contains(new), "{} does not name {new}", dep.name);
			}
		}
	}
}
