//! The documents the client has open, and what can be read off a position in
//! one of them.

use std::collections::HashMap;

use lsp_types::{Diagnostic, Location, Position, Range, TextEdit, Uri};

use crate::context::{self, SyntaxContext};
use crate::deprecation;
use crate::position::{byte_to_utf16_offset, is_ident_char, line_at, utf16_to_byte_offset};
use crate::symbols::{self, DocumentIndex, NsisSymbolKind, SymbolDef};

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

	/// The identifier under `pos`. `None` in a comment, in the prose of a quoted
	/// string, past the end of the line, or where there is no identifier at all —
	/// a lone sigil with no name after it is not one.
	pub fn identifier_at(&self, pos: Position) -> Option<Identifier> {
		let line_str = line_at(&self.text, pos.line)?;
		let col = utf16_to_byte_offset(line_str, pos.character);
		if col > line_str.len() {
			return None;
		}

		let syntax = context::context_at(&self.text, pos.line, col);
		if syntax == SyntaxContext::Comment {
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

		// A string is prose with holes in it: only `$VAR`, `${DEFINE}` and
		// `$(LangString)` are script, so a word without a `$` in front of it is
		// something the user wrote for the installer to say, not a name the
		// server knows anything about.
		if syntax == SyntaxContext::String && !is_interpolation(bytes, bare_start) {
			return None;
		}

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

/// Whether the name starting at `start` is expanded rather than printed: a `$`
/// directly in front of it, or one through the `{` or `(` that opens a deref.
fn is_interpolation(bytes: &[u8], start: usize) -> bool {
	match start {
		0 => false,
		_ if bytes[start - 1] == b'$' => true,
		1 => false,
		_ => matches!(bytes[start - 1], b'{' | b'(') && bytes[start - 2] == b'$',
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

	/// Every document in `Uri` order.
	///
	/// The map hands them back in whatever order it likes, which would make a
	/// resolution that stops at the first answer depend on where a document
	/// happened to hash to.
	fn ordered(&self) -> Vec<&Document> {
		let mut docs: Vec<&Document> = self.documents().collect();
		docs.sort_by(|a, b| a.uri.as_str().cmp(b.uri.as_str()));
		docs
	}

	/// Where `name` is defined, searched over every open document.
	///
	/// A name is looked up from somewhere — `from` is the document the user is
	/// in — and that document wins, because a script that declares a name of
	/// its own means that one. Everything else is searched in `Uri` order, so
	/// two scripts declaring the same name resolve the same way every time.
	///
	/// Resolving across documents rather than within one is what lets a request
	/// be answered from a use site: `${APP_NAME}` in an installer script is the
	/// `!define` in the header it includes, and until this looked past the
	/// current document a rename from the use site had nothing to rename.
	pub fn definition(&self, from: &Uri, name: &str) -> Option<Definition<'_>> {
		let here = self
			.document(from)
			.and_then(|doc| Definition::of(doc, name));
		if here.is_some() {
			return here;
		}

		self.ordered()
			.into_iter()
			.filter(|doc| doc.uri.as_str() != from.as_str())
			.find_map(|doc| Definition::of(doc, name))
	}

	/// Every use of `name` as a `kind`, in every open document. Use sites only —
	/// the declarations are [`Definition`]'s business.
	pub fn references(&self, name: &str, kind: NsisSymbolKind) -> Vec<Location> {
		self.ordered()
			.into_iter()
			.flat_map(|doc| {
				symbols::find_references(&doc.text, name, kind)
					.into_iter()
					.map(|range| Location {
						uri: doc.uri.clone(),
						range,
					})
			})
			.collect()
	}

	/// Everything a rename of `name` has to rewrite, keyed by the document it
	/// belongs to. A document with nothing to rewrite is absent rather than
	/// present and empty.
	#[allow(clippy::mutable_key_type)]
	pub fn rename_edits(
		&self,
		name: &str,
		kind: NsisSymbolKind,
		new_name: &str,
	) -> HashMap<Uri, Vec<TextEdit>> {
		let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();

		for doc in self.documents() {
			let mut edits: Vec<TextEdit> = symbols::find_references(&doc.text, name, kind)
				.into_iter()
				.map(|range| TextEdit {
					range,
					new_text: new_name.to_string(),
				})
				.collect();

			// Those are the use sites; the declarations themselves have to be
			// rewritten too, and there may be more than one of them.
			edits.extend(doc.index.definitions_of(name, kind).map(|sym| TextEdit {
				range: sym.selection_range,
				new_text: new_name.to_string(),
			}));

			if !edits.is_empty() {
				changes.insert(doc.uri.clone(), edits);
			}
		}

		changes
	}
}

/// A definition, and the document it was found in.
///
/// The document travels with the symbol because a caller that searched the
/// whole workspace no longer knows which one answered, and both the `Uri` it
/// reports and the kind it renames by come from the same place.
pub struct Definition<'a> {
	/// The document holding the declaration.
	pub uri: &'a Uri,
	/// The declaration itself.
	pub symbol: &'a SymbolDef,
}

impl<'a> Definition<'a> {
	fn of(doc: &'a Document, name: &str) -> Option<Self> {
		doc.index.definition(name).map(|symbol| Definition {
			uri: &doc.uri,
			symbol,
		})
	}

	/// Where the declaration sits, as the client wants to hear it.
	pub fn location(&self) -> Location {
		Location {
			uri: self.uri.clone(),
			range: self.symbol.selection_range,
		}
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

	/// The text of a string is what the installer says, not script: a word in it
	/// that happens to spell a command is prose.
	#[test]
	fn identifier_at_in_string_prose_is_none() {
		let doc = document("DetailPrint \"What's your Name\"");
		assert!(doc.identifier_at(Position::new(0, 26)).is_none());
	}

	/// The holes in a string are script, though — everything that expands is
	/// still an identifier there.
	#[test]
	fn identifier_at_in_string_reads_an_interpolation() {
		let doc = document("DetailPrint \"in $INSTDIR\"");
		assert_eq!(text_at(&doc, Position::new(0, 20)), Some("$INSTDIR".into()));

		let doc = document("DetailPrint \"of ${APP_NAME}\"");
		assert_eq!(text_at(&doc, Position::new(0, 20)), Some("APP_NAME".into()));

		let doc = document("DetailPrint \"say $(MY_TEXT)\"");
		assert_eq!(text_at(&doc, Position::new(0, 20)), Some("MY_TEXT".into()));
	}

	/// A brace that no `$` opened is punctuation in the middle of prose.
	#[test]
	fn identifier_at_in_string_needs_the_dollar_before_the_brace() {
		let doc = document("DetailPrint \"a {Name} b\"");
		assert!(doc.identifier_at(Position::new(0, 17)).is_none());
	}

	/// The gate is the string, not the quote: the same word after it is code
	/// again.
	#[test]
	fn identifier_at_after_a_string_is_read_again() {
		let doc = document("!insertmacro \"foo\" Name");
		assert_eq!(text_at(&doc, Position::new(0, 21)), Some("Name".into()));
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

	// ── definition, references, rename_edits ──

	const A_URI: &str = "file:///a.nsi";
	const B_URI: &str = "file:///b.nsi";

	/// A header declaring a name, and a script that only uses it.
	fn two_documents() -> Workspace {
		let mut workspace = Workspace::new();
		workspace.open(
			uri(A_URI),
			"!define APP_NAME \"Test\"\nDetailPrint ${APP_NAME}".to_string(),
		);
		workspace.open(
			uri(B_URI),
			"DetailPrint ${APP_NAME}\nDetailPrint ${APP_NAME}".to_string(),
		);
		workspace
	}

	/// The point of resolving over the workspace: a name used in one document
	/// and declared in another still has a definition.
	#[test]
	fn a_definition_is_found_in_another_document() {
		let workspace = two_documents();
		let def = workspace.definition(&uri(B_URI), "APP_NAME").unwrap();

		assert_eq!(def.uri, &uri(A_URI));
		assert_eq!(def.symbol.name, "APP_NAME");
		assert_eq!(def.location().range.start.line, 0);
	}

	/// A document that declares a name of its own means that one, whatever the
	/// rest of the workspace says.
	#[test]
	fn the_document_asked_from_wins() {
		let mut workspace = two_documents();
		workspace.open(uri(B_URI), "!define APP_NAME \"Other\"".to_string());

		let def = workspace.definition(&uri(B_URI), "APP_NAME").unwrap();
		assert_eq!(def.uri, &uri(B_URI));
	}

	#[test]
	fn a_name_nothing_declares_has_no_definition() {
		let workspace = two_documents();
		assert!(workspace.definition(&uri(A_URI), "NOPE").is_none());
	}

	/// A request from a document that is not open still resolves — the name is
	/// looked for everywhere, and only the preference for one document is lost.
	#[test]
	fn definition_from_an_unopened_document_still_searches_the_rest() {
		let workspace = two_documents();
		let def = workspace
			.definition(&uri("file:///gone.nsi"), "APP_NAME")
			.unwrap();

		assert_eq!(def.uri, &uri(A_URI));
	}

	#[test]
	fn references_span_every_open_document() {
		let workspace = two_documents();
		let locations = workspace.references("APP_NAME", NsisSymbolKind::Define);

		assert_eq!(locations.iter().filter(|l| l.uri == uri(A_URI)).count(), 1);
		assert_eq!(locations.iter().filter(|l| l.uri == uri(B_URI)).count(), 2);
	}

	/// A rename rewrites the uses and the declaration, and says nothing about a
	/// document that has neither.
	#[test]
	fn rename_edits_cover_uses_and_declarations() {
		let mut workspace = two_documents();
		workspace.open(uri("file:///c.nsi"), "DetailPrint \"nothing\"".to_string());

		#[allow(clippy::mutable_key_type)]
		let changes = workspace.rename_edits("APP_NAME", NsisSymbolKind::Define, "PRODUCT");

		assert_eq!(changes.len(), 2);
		assert_eq!(changes[&uri(A_URI)].len(), 2); // one use plus the `!define`
		assert_eq!(changes[&uri(B_URI)].len(), 2);
		assert!(changes.values().flatten().all(|e| e.new_text == "PRODUCT"));
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
