//! The documents the client has open, and what can be read off a position in
//! one of them.

use std::collections::HashMap;

use lsp_types::{Position, Uri};

use crate::context::{self, SyntaxContext};
use crate::position::{byte_to_utf16_offset, is_ident_char, line_at, utf16_to_byte_offset};
use crate::symbols::{self, DocumentIndex};

/// One open document: the `Uri` it arrived under, its current text, and the
/// index derived from that text.
pub struct Document {
	pub uri: Uri,
	pub text: String,
	pub index: DocumentIndex,
}

impl Document {
	fn new(uri: Uri, text: String) -> Self {
		let index = symbols::index_document(&text);
		Self { uri, text, index }
	}

	/// The identifier under `pos`, including a leading `!` or `$` if one is
	/// there. `None` in a comment, past the end of the line, or where there is
	/// no identifier at all.
	pub fn word_at(&self, pos: Position) -> Option<String> {
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

		if start > 0 && (bytes[start - 1] == b'!' || bytes[start - 1] == b'$') {
			start -= 1;
		}

		if start == end {
			return None;
		}

		Some(line_str[start..end].to_string())
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
	/// there and reindexing. Both `didOpen` and `didChange` land here: the
	/// server holds full document text, so a change is just a fresh open.
	pub fn open(&mut self, uri: Uri, text: String) {
		self.documents
			.insert(uri.as_str().to_string(), Document::new(uri, text));
	}

	pub fn document(&self, uri: &Uri) -> Option<&Document> {
		self.documents.get(uri.as_str())
	}

	pub fn documents(&self) -> impl Iterator<Item = &Document> {
		self.documents.values()
	}

	/// The document at `uri` together with the identifier under `pos` — the
	/// opening move of every position-based request.
	pub fn word_at(&self, uri: &Uri, pos: Position) -> Option<(&Document, String)> {
		let doc = self.document(uri)?;
		let word = doc.word_at(pos)?;
		Some((doc, word))
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

	// ── word_at ──

	#[test]
	fn word_at_simple() {
		let doc = document("Section main\n  DetailPrint hello\nSectionEnd");
		assert_eq!(doc.word_at(Position::new(1, 4)), Some("DetailPrint".into()));
	}

	#[test]
	fn word_at_bang_prefix() {
		let doc = document("!include file.nsh");
		assert_eq!(doc.word_at(Position::new(0, 2)), Some("!include".into()));
	}

	#[test]
	fn word_at_dollar_prefix() {
		let doc = document("StrCpy $0 $INSTDIR");
		assert_eq!(doc.word_at(Position::new(0, 12)), Some("$INSTDIR".into()));
	}

	#[test]
	fn word_at_out_of_range() {
		let doc = document("hello");
		assert_eq!(doc.word_at(Position::new(5, 0)), None);
	}

	#[test]
	fn word_at_in_comment_is_none() {
		let doc = document("# DetailPrint hello");
		assert_eq!(doc.word_at(Position::new(0, 5)), None);
	}

	/// A cursor touching the end of a word still reads that word; only one with
	/// whitespace on both sides has nothing under it.
	#[test]
	fn word_at_a_boundary_takes_the_word_before_it() {
		let doc = document("Section  main");
		assert_eq!(doc.word_at(Position::new(0, 7)), Some("Section".into()));
		assert_eq!(doc.word_at(Position::new(0, 8)), None);
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
	fn word_at_reads_through_the_workspace() {
		let mut workspace = Workspace::new();
		workspace.open(uri(URI), "StrCpy $0 $INSTDIR".to_string());

		let (doc, word) = workspace.word_at(&uri(URI), Position::new(0, 12)).unwrap();
		assert_eq!(word, "$INSTDIR");
		assert_eq!(doc.uri, uri(URI));
	}

	#[test]
	fn word_at_unknown_document_is_none() {
		let workspace = Workspace::new();
		assert!(workspace.word_at(&uri(URI), Position::new(0, 0)).is_none());
	}
}
