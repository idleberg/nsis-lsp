use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum PreprocessMode {
	Ppo,
	SafePpo,
	None,
}

impl PreprocessMode {
	pub fn from_option(s: Option<&str>) -> Self {
		match s {
			Some("ppo") => Self::Ppo,
			Some("safe_ppo") => Self::SafePpo,
			_ => Self::None,
		}
	}
}

pub struct CompilerOutput {
	pub stdout: String,
	pub stderr: String,
}

pub fn find_makensis(custom_path: &str) -> Option<String> {
	if !custom_path.is_empty() && Path::new(custom_path).exists() {
		return Some(custom_path.to_string());
	}
	which("makensis")
}

pub fn run_makensis(
	makensis_path: &str,
	file_path: &str,
	mode: &PreprocessMode,
) -> Result<CompilerOutput, String> {
	let mut cmd = Command::new(makensis_path);
	cmd.arg("-V2");

	match mode {
		PreprocessMode::Ppo => {
			cmd.arg("-PPO");
		}
		PreprocessMode::SafePpo => {
			cmd.arg("-SAFEPPO");
		}
		PreprocessMode::None => {}
	}

	cmd.arg(file_path);

	let output = cmd
		.output()
		.map_err(|e| format!("failed to run makensis: {e}"))?;

	Ok(CompilerOutput {
		stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
		stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
	})
}

/// The path `makensis` should be handed for a document's `Uri`.
///
/// The compiler reads the file from disk, so a document that lives anywhere but
/// the local filesystem has nothing to compile.
pub fn uri_to_file_path(uri_str: &str) -> Option<String> {
	let stripped = uri_str.strip_prefix("file://")?;
	Some(percent_decode(stripped))
}

fn percent_decode(s: &str) -> String {
	let mut result = Vec::new();
	let bytes = s.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'%'
			&& i + 2 < bytes.len()
			&& let Ok(byte) =
				u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
		{
			result.push(byte);
			i += 3;
			continue;
		}
		result.push(bytes[i]);
		i += 1;
	}
	String::from_utf8_lossy(&result).into_owned()
}

fn which(binary: &str) -> Option<String> {
	let path_var = std::env::var("PATH").ok()?;
	let sep = if cfg!(windows) { ';' } else { ':' };

	for dir in path_var.split(sep) {
		let candidate = Path::new(dir).join(binary);
		if candidate.is_file() {
			return Some(candidate.to_string_lossy().into_owned());
		}
		if cfg!(windows) {
			let with_exe = candidate.with_extension("exe");
			if with_exe.is_file() {
				return Some(with_exe.to_string_lossy().into_owned());
			}
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn from_option_ppo() {
		assert_eq!(
			PreprocessMode::from_option(Some("ppo")),
			PreprocessMode::Ppo
		);
	}

	#[test]
	fn from_option_safe_ppo() {
		assert_eq!(
			PreprocessMode::from_option(Some("safe_ppo")),
			PreprocessMode::SafePpo
		);
	}

	#[test]
	fn from_option_none_variant() {
		assert_eq!(PreprocessMode::from_option(None), PreprocessMode::None);
	}

	#[test]
	fn from_option_unknown_string() {
		assert_eq!(
			PreprocessMode::from_option(Some("bogus")),
			PreprocessMode::None
		);
	}

	#[test]
	fn find_makensis_empty_custom_path() {
		let result = find_makensis("");
		// Falls through to which(); result depends on environment
		// but must not panic
		let _ = result;
	}

	#[test]
	fn find_makensis_nonexistent_custom_path_falls_through_to_which() {
		let result = find_makensis("/no/such/binary/makensis");
		// Custom path doesn't exist, so it falls through to which("makensis").
		// Result depends on whether makensis is installed on the system.
		assert!(result.is_none() || result.as_deref().is_some_and(|p| p.contains("makensis")));
	}

	#[test]
	fn which_nonexistent_binary() {
		assert_eq!(which("__no_such_binary_xyz__"), None);
	}

	// ── percent_decode / uri_to_file_path ──

	#[test]
	fn percent_decode_no_encoding() {
		assert_eq!(percent_decode("/foo/bar.nsi"), "/foo/bar.nsi");
	}

	#[test]
	fn percent_decode_spaces() {
		assert_eq!(
			percent_decode("/my%20path/file%20name"),
			"/my path/file name"
		);
	}

	#[test]
	fn percent_decode_mixed() {
		assert_eq!(percent_decode("a%2Fb%25c"), "a/b%c");
	}

	#[test]
	fn uri_to_file_path_valid() {
		assert_eq!(
			uri_to_file_path("file:///home/user/test.nsi"),
			Some("/home/user/test.nsi".into())
		);
	}

	#[test]
	fn uri_to_file_path_not_file_uri() {
		assert_eq!(uri_to_file_path("https://example.com"), None);
	}
}
