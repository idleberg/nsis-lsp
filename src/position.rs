//! Converting between LSP positions and byte offsets.
//!
//! LSP counts columns in UTF-16 code units; Rust indexes text by byte. Every
//! conversion between the two goes through here, so no caller has to remember
//! which of the two a number is in.

/// The text of line `line`, counting from 0, or `None` past the last line.
pub fn line_at(text: &str, line: u32) -> Option<&str> {
	text.lines().nth(line as usize)
}

/// Whether `b` can appear inside an NSIS identifier. Sigils (`$`, `!`) and `{`
/// are deliberately excluded — callers decide whether to take them.
pub fn is_ident_char(b: u8) -> bool {
	b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

/// The byte offset of UTF-16 column `utf16_col`, clamped to the end of `line`.
pub fn utf16_to_byte_offset(line: &str, utf16_col: u32) -> usize {
	let mut utf16_count = 0u32;
	for (byte_idx, ch) in line.char_indices() {
		if utf16_count >= utf16_col {
			return byte_idx;
		}
		utf16_count += ch.len_utf16() as u32;
	}
	line.len()
}

/// The UTF-16 column of `byte_offset`, which must be a char boundary in `line`.
pub fn byte_to_utf16_offset(line: &str, byte_offset: usize) -> u32 {
	let mut count = 0u32;
	for ch in line[..byte_offset].chars() {
		count += ch.len_utf16() as u32;
	}
	count
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn line_at_picks_the_line() {
		let text = "one\ntwo\nthree";
		assert_eq!(line_at(text, 1), Some("two"));
	}

	#[test]
	fn line_at_past_the_end() {
		assert_eq!(line_at("one", 5), None);
	}

	#[test]
	fn ident_char_alphanumeric() {
		assert!(is_ident_char(b'a'));
		assert!(is_ident_char(b'Z'));
		assert!(is_ident_char(b'5'));
	}

	#[test]
	fn ident_char_underscore_dot() {
		assert!(is_ident_char(b'_'));
		assert!(is_ident_char(b'.'));
	}

	#[test]
	fn ident_char_rejects_special() {
		assert!(!is_ident_char(b' '));
		assert!(!is_ident_char(b'!'));
		assert!(!is_ident_char(b'$'));
	}

	#[test]
	fn utf16_to_byte_ascii() {
		assert_eq!(utf16_to_byte_offset("hello", 3), 3);
	}

	#[test]
	fn utf16_to_byte_past_end_is_clamped() {
		assert_eq!(utf16_to_byte_offset("hello", 99), 5);
	}

	#[test]
	fn byte_to_utf16_ascii() {
		assert_eq!(byte_to_utf16_offset("hello", 3), 3);
	}

	#[test]
	fn utf16_roundtrip_multibyte() {
		let line = "aé€b";
		let byte_off = utf16_to_byte_offset(line, 3);
		let utf16_off = byte_to_utf16_offset(line, byte_off);
		assert_eq!(utf16_off, 3);
	}
}
