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
