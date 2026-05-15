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
}
