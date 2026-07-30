//! The documents the client has open, and what can be read off a position in
//! one of them.

use std::collections::HashMap;

use lsp_types::{Diagnostic, Position, Range, Uri};

use crate::context::{self, SyntaxContext};
use crate::deprecation;
use crate::position::{byte_to_utf16_offset, is_ident_char, line_at, utf16_to_byte_offset};
use crate::symbols::{self, DocumentIndex};

/// One identifier, read off a position in a document.
///
/// NSIS writes an identifier with a sigil at some call sites and without it at
/// others — `$myVar` is the same name as the `Var myVar` that declared it — so
/// every caller needs both spellings and the place each one sits. Reading the
/// line once and carrying all of it is what stops a second caller re-deriving
/// the half the first one discarded, with a sigil rule that has drifted.
pub struct Identifier {
	/// The identifier as the user wrote it, sigil and all: `$myVar`, `!include`.
	pub text: String,
	/// The name beneath the sigil, which is what definitions are declared under.
	pub bare: String,
	/// Where the bare name sits. A rename rewrites this and leaves the sigil.
	pub bare_range: Range,
}

/// One open document: the `Uri` it arrived under, its current text, and
/// everything derived from that text.
pub struct Document {
	pub uri: Uri,
	pub text: String,
	pub index: DocumentIndex,
	/// The diagnostics that follow from the text alone.
	///
	/// Compiler diagnostics are not here: they need makensis and a file on
	/// disk, so they are merged on top at publish time rather than stored
	/// against the document.
	pub diagnostics: Vec<Diagnostic>,
}

impl Document {
	fn new(uri: Uri, text: String) -> Self {
		let index = symbols::index_document(&text);
		let diagnostics = deprecation::scan(&text);
		Self {
			uri,
			text,
			index,
			diagnostics,
		}
	}

	/// The identifier under `pos`. `None` in a comment, past the end of the
	/// line, or where there is no identifier at all — a lone sigil with no name
	/// after it is not one.
	pub fn identifier_at(&self, pos: Position) -> Option<Identifier> {
		let line_str = line_at(&self.text, pos.line)?;
		let col = utf16_to_byte_offset(line_str, pos.character);
		if col > line_str.len() {
			return None;
		}

		if context::context_at(&self.text, pos.line, col) == SyntaxContext::Comment {
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

		if start == end {
			return None;
		}

		let bare_start = start;
		if start > 0 && (bytes[start - 1] == b'!' || bytes[start - 1] == b'$') {
			start -= 1;
		}

		let at = |offset: usize| Position::new(pos.line, byte_to_utf16_offset(line_str, offset));

		Some(Identifier {
			text: line_str[start..end].to_string(),
			bare: line_str[bare_start..end].to_string(),
			bare_range: Range::new(at(bare_start), at(end)),
		})
	}

	/// The last column of the document, as an LSP position.
	pub fn end_position(&self) -> Position {
		let lines: Vec<&str> = self.text.lines().collect();
		let last_line = lines.len().saturating_sub(1) as u32;

		if self.text.ends_with('\n') || self.text.ends_with("\r\n") {
			Position::new(last_line + 1, 0)
		} else {
			let last_col = lines.last().map_or(0, |l| byte_to_utf16_offset(l, l.len()));
			Position::new(last_line, last_col)
		}
	}
}

/// Every document the client has open.
///
/// Documents are keyed by URI text, but each one keeps the parsed `Uri` it
/// arrived under, so no caller ever has to parse a `Uri` back out of a key —
/// and a request that spans the whole workspace has nothing left to fail on
/// part way through.
#[derive(Default)]
pub struct Workspace {
	documents: HashMap<String, Document>,
}

impl Workspace {
	pub fn new() -> Self {
		Self::default()
	}

	/// Takes `text` as the current content of `uri`, replacing whatever was
	/// there and recomputing everything derived from it. Both `didOpen` and
	/// `didChange` land here: the server holds full document text, so a change
	/// is just a fresh open.
	///
	/// Returns the document it just stored, so a caller that has to publish
	/// what the new text implies has it to hand.
	pub fn open(&mut self, uri: Uri, text: String) -> &Document {
		self.documents
			.entry(uri.as_str().to_string())
			.insert_entry(Document::new(uri, text))
			.into_mut()
	}

	pub fn document(&self, uri: &Uri) -> Option<&Document> {
		self.documents.get(uri.as_str())
	}

	pub fn documents(&self) -> impl Iterator<Item = &Document> {
		self.documents.values()
	}

	/// The document at `uri` together with the identifier under `pos` — the
	/// opening move of every position-based request.
	pub fn identifier_at(&self, uri: &Uri, pos: Position) -> Option<(&Document, Identifier)> {
		let doc = self.document(uri)?;
		let ident = doc.identifier_at(pos)?;
		Some((doc, ident))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const URI: &str = "file:///test.nsi";

	fn uri(s: &str) -> Uri {
		s.parse().unwrap()
	}

	fn document(text: &str) -> Document {
		Document::new(uri(URI), text.to_string())
	}

	// ── identifier_at ──

	fn text_at(doc: &Document, pos: Position) -> Option<String> {
		doc.identifier_at(pos).map(|ident| ident.text)
	}

	#[test]
	fn identifier_at_simple() {
		let doc = document("Section main\n  DetailPrint hello\nSectionEnd");
		assert_eq!(
			text_at(&doc, Position::new(1, 4)),
			Some("DetailPrint".into())
		);
	}

	#[test]
	fn identifier_at_bang_prefix() {
		let doc = document("!include file.nsh");
		assert_eq!(text_at(&doc, Position::new(0, 2)), Some("!include".into()));
	}

	#[test]
	fn identifier_at_dollar_prefix() {
		let doc = document("StrCpy $0 $INSTDIR");
		assert_eq!(text_at(&doc, Position::new(0, 12)), Some("$INSTDIR".into()));
	}

	#[test]
	fn identifier_at_out_of_range() {
		let doc = document("hello");
		assert!(doc.identifier_at(Position::new(5, 0)).is_none());
	}

	#[test]
	fn identifier_at_in_comment_is_none() {
		let doc = document("# DetailPrint hello");
		assert!(doc.identifier_at(Position::new(0, 5)).is_none());
	}

	/// A cursor touching the end of a word still reads that word; only one with
	/// whitespace on both sides has nothing under it.
	#[test]
	fn identifier_at_a_boundary_takes_the_word_before_it() {
		let doc = document("Section  main");
		assert_eq!(text_at(&doc, Position::new(0, 7)), Some("Section".into()));
		assert!(doc.identifier_at(Position::new(0, 8)).is_none());
	}

	/// The sigil is stripped and the name located in the same read, so no caller
	/// has to trim it back off or walk the line again to find where it starts.
	#[test]
	fn an_identifier_carries_the_name_under_its_sigil() {
		let doc = document("StrCpy $0 $INSTDIR");
		let ident = doc.identifier_at(Position::new(0, 12)).unwrap();

		assert_eq!(ident.text, "$INSTDIR");
		assert_eq!(ident.bare, "INSTDIR");
		assert_eq!(
			ident.bare_range,
			Range::new(Position::new(0, 11), Position::new(0, 18))
		);
	}

	/// Without a sigil the two spellings coincide, so a caller can read `bare`
	/// unconditionally.
	#[test]
	fn an_identifier_without_a_sigil_is_its_own_bare_name() {
		let doc = document("Call myFunc");
		let ident = doc.identifier_at(Position::new(0, 7)).unwrap();

		assert_eq!(ident.text, "myFunc");
		assert_eq!(ident.bare, "myFunc");
	}

	/// Columns are UTF-16 units, so the range is still right on a line the
	/// client counts differently from Rust.
	#[test]
	fn identifier_ranges_count_utf16_columns() {
		let doc = document("DetailPrint \"€\" $INSTDIR");
		let ident = doc.identifier_at(Position::new(0, 18)).unwrap();

		assert_eq!(ident.text, "$INSTDIR");
		assert_eq!(ident.bare_range.start.character, 17);
		assert_eq!(ident.bare_range.end.character, 24);
	}

	/// A sigil with no name after it is not an identifier — answering `"$"`
	/// would hand every caller an empty name to look up.
	#[test]
	fn a_lone_sigil_is_not_an_identifier() {
		let doc = document("StrCpy $ 0");
		assert!(doc.identifier_at(Position::new(0, 8)).is_none());
	}

	// ── end_position ──

	#[test]
	fn end_position_without_trailing_newline() {
		let doc = document("Section main\nSectionEnd");
		assert_eq!(doc.end_position(), Position::new(1, 10));
	}

	#[test]
	fn end_position_with_trailing_newline() {
		let doc = document("Section main\nSectionEnd\n");
		assert_eq!(doc.end_position(), Position::new(2, 0));
	}

	#[test]
	fn end_position_counts_utf16_columns() {
		let doc = document("DetailPrint \"€\"");
		assert_eq!(doc.end_position(), Position::new(0, 15));
	}

	// ── Workspace ──

	#[test]
	fn open_indexes_the_document() {
		let mut workspace = Workspace::new();
		workspace.open(uri(URI), "Function myFunc\nFunctionEnd".to_string());

		let doc = workspace.document(&uri(URI)).unwrap();
		assert_eq!(doc.index.roots().len(), 1);
		assert_eq!(doc.uri, uri(URI));
	}

	#[test]
	fn open_replaces_and_reindexes() {
		let mut workspace = Workspace::new();
		workspace.open(uri(URI), "Function old\nFunctionEnd".to_string());
		workspace.open(uri(URI), "Function new\nFunctionEnd".to_string());

		assert_eq!(workspace.documents().count(), 1);
		let doc = workspace.document(&uri(URI)).unwrap();
		assert_eq!(doc.index.roots()[0].name, "new");
	}

	/// The index is not the only thing the text implies — a document knows its
	/// own deprecation warnings too, so nothing has to re-walk it to find them.
	#[test]
	fn open_scans_the_document_as_well_as_indexing_it() {
		let mut workspace = Workspace::new();
		let doc = workspace.open(uri(URI), "SubSection foo".to_string());

		assert_eq!(doc.diagnostics.len(), 1);
		assert_eq!(doc.diagnostics[0].range.start.line, 0);
	}

	#[test]
	fn open_replaces_the_diagnostics_too() {
		let mut workspace = Workspace::new();
		workspace.open(uri(URI), "SubSection foo".to_string());
		let doc = workspace.open(uri(URI), "SectionGroup foo".to_string());

		assert!(doc.diagnostics.is_empty());
	}

	#[test]
	fn document_unknown_uri() {
		let workspace = Workspace::new();
		assert!(workspace.document(&uri(URI)).is_none());
	}

	/// Every document carries the `Uri` it was opened with, so a workspace-wide
	/// request never has to parse one back out of a key.
	#[test]
	fn documents_keep_the_uri_they_were_opened_with() {
		let mut workspace = Workspace::new();
		workspace.open(uri("file:///a%20b.nsi"), "Var one".to_string());
		workspace.open(uri("file:///c.nsi"), "Var two".to_string());

		let mut uris: Vec<&str> = workspace.documents().map(|d| d.uri.as_str()).collect();
		uris.sort_unstable();
		assert_eq!(uris, vec!["file:///a%20b.nsi", "file:///c.nsi"]);
	}

	#[test]
	fn identifier_at_reads_through_the_workspace() {
		let mut workspace = Workspace::new();
		workspace.open(uri(URI), "StrCpy $0 $INSTDIR".to_string());

		let (doc, ident) = workspace
			.identifier_at(&uri(URI), Position::new(0, 12))
			.unwrap();
		assert_eq!(ident.text, "$INSTDIR");
		assert_eq!(doc.uri, uri(URI));
	}

	#[test]
	fn identifier_at_unknown_document_is_none() {
		let workspace = Workspace::new();
		assert!(
			workspace
				.identifier_at(&uri(URI), Position::new(0, 0))
				.is_none()
		);
	}
}
