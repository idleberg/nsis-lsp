//! What the client tells the server about how to behave, and what the server
//! makes of it.
//!
//! The wire shape ([`InitOptions`] and the sections under it) and the shape the
//! handlers read ([`LspState`]) are deliberately two types: the first mirrors
//! the JSON exactly, defaults and all, and the second is already resolved —
//! `makensis` located on disk, the preprocess mode parsed. Settings arrive twice
//! by two different routes, `initializationOptions` at startup and
//! `workspace/didChangeConfiguration` later, and both end at
//! [`LspState::from_options`].

use ardent::{CommentStyle, EndOfLine};
use lsp_types::MessageType;
use serde::Deserialize;

use crate::client::Client;
use crate::compiler::{self, PreprocessMode};

#[derive(Debug, Default, Deserialize)]
pub struct InitOptions {
	#[serde(default)]
	pub diagnostics: DiagnosticsOptions,
	#[serde(default)]
	pub formatter: FormatterInitOptions,
	#[serde(default)]
	pub makensis: MakensisOptions,
}

#[derive(Debug, Deserialize)]
pub struct FormatterInitOptions {
	#[serde(default)]
	pub end_of_line: Option<String>,
	#[serde(default)]
	pub print_width: usize,
	#[serde(default = "default_true")]
	pub trim_empty_lines: bool,
	#[serde(default)]
	pub single_quote: bool,
	#[serde(default)]
	pub comment_style: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DiagnosticsOptions {
	#[serde(default = "default_preprocess_mode")]
	pub preprocess_mode: Option<String>,
	#[serde(default = "default_true")]
	pub enabled_on_save: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct MakensisOptions {
	#[serde(default)]
	pub path: String,
}

fn default_preprocess_mode() -> Option<String> {
	Some("ppo".into())
}

fn default_true() -> bool {
	true
}

impl Default for DiagnosticsOptions {
	fn default() -> Self {
		Self {
			preprocess_mode: default_preprocess_mode(),
			enabled_on_save: true,
		}
	}
}

// Mirrors the `serde` defaults above, so an absent `formatter` key and an empty
// one produce the same settings.
impl Default for FormatterInitOptions {
	fn default() -> Self {
		Self {
			end_of_line: None,
			print_width: 0,
			trim_empty_lines: true,
			single_quote: false,
			comment_style: None,
		}
	}
}

pub fn parse_end_of_line(value: Option<&str>) -> Option<EndOfLine> {
	match value {
		Some("crlf") => Some(EndOfLine::Crlf),
		Some("lf") => Some(EndOfLine::Lf),
		_ => None,
	}
}

// Anything else, an absent setting included, leaves every comment with the marker
// the author wrote it with.
pub fn parse_comment_style(value: Option<&str>) -> Option<CommentStyle> {
	match value {
		Some("hash") => Some(CommentStyle::Hash),
		Some("semi") => Some(CommentStyle::Semi),
		_ => None,
	}
}

pub struct LspState {
	pub makensis_path: Option<String>,
	pub preprocess_mode: PreprocessMode,
	pub diagnostics_on_save: bool,
	pub end_of_line: Option<EndOfLine>,
	pub print_width: usize,
	pub trim_empty_lines: bool,
	pub single_quote: bool,
	pub comment_style: Option<CommentStyle>,
}

impl LspState {
	pub fn from_options(options: InitOptions) -> Self {
		Self {
			makensis_path: compiler::find_makensis(&options.makensis.path),
			preprocess_mode: PreprocessMode::from_option(
				options.diagnostics.preprocess_mode.as_deref(),
			),
			diagnostics_on_save: options.diagnostics.enabled_on_save,
			end_of_line: parse_end_of_line(options.formatter.end_of_line.as_deref()),
			print_width: options.formatter.print_width,
			trim_empty_lines: options.formatter.trim_empty_lines,
			single_quote: options.formatter.single_quote,
			comment_style: parse_comment_style(options.formatter.comment_style.as_deref()),
		}
	}
}

pub enum SettingsUpdate {
	Replace(InitOptions),
	// Clients that hold their settings server-side send `null`, which we cannot
	// resolve without a `workspace/configuration` request.
	Unavailable,
	Unparseable,
}

// Settings take the same shape as `initializationOptions`, optionally wrapped in
// an `nsis` section. They replace the previous set wholesale, so a client has to
// send every option it cares about, not just the one that changed.
pub fn parse_settings(settings: serde_json::Value) -> SettingsUpdate {
	let settings = match settings.get("nsis") {
		Some(section) => section.clone(),
		None => settings,
	};

	if settings.is_null() {
		return SettingsUpdate::Unavailable;
	}

	match serde_json::from_value(settings) {
		Ok(options) => SettingsUpdate::Replace(options),
		Err(_) => SettingsUpdate::Unparseable,
	}
}

pub fn handle_configuration_change(
	client: &impl Client,
	settings: serde_json::Value,
	state: &mut LspState,
) {
	let options = match parse_settings(settings) {
		SettingsUpdate::Replace(options) => options,
		SettingsUpdate::Unavailable => return,
		SettingsUpdate::Unparseable => {
			client.log(
				MessageType::WARNING,
				"Ignoring workspace/didChangeConfiguration: settings could not be parsed",
			);
			return;
		}
	};

	let previous_makensis = state.makensis_path.clone();
	*state = LspState::from_options(options);

	client.log(MessageType::INFO, "Configuration reloaded");

	if state.makensis_path != previous_makensis {
		log_makensis_path(client, state.makensis_path.as_deref());
	}
}

/// Tell the client which compiler the server found, or that it found none.
pub fn log_makensis_path(client: &impl Client, path: Option<&str>) {
	match path {
		Some(path) => client.log(MessageType::INFO, &format!("Using makensis: {path}")),
		None => client.log(
			MessageType::WARNING,
			"makensis not found — diagnostics unavailable",
		),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn replaced(settings: serde_json::Value) -> InitOptions {
		match parse_settings(settings) {
			SettingsUpdate::Replace(options) => options,
			_ => panic!("expected settings to be replaced"),
		}
	}

	// ── parse_settings ──

	#[test]
	fn settings_bare_object() {
		let options = replaced(json!({
			"formatter": { "print_width": 100, "single_quote": true },
		}));

		assert_eq!(options.formatter.print_width, 100);
		assert!(options.formatter.single_quote);
	}

	#[test]
	fn settings_nsis_section() {
		let options = replaced(json!({
			"nsis": { "formatter": { "print_width": 80 } },
		}));

		assert_eq!(options.formatter.print_width, 80);
	}

	#[test]
	fn settings_omitted_options_fall_back_to_defaults() {
		let options = replaced(json!({ "formatter": { "single_quote": true } }));

		assert_eq!(options.formatter.print_width, 0);
		assert!(options.formatter.trim_empty_lines);
		assert_eq!(options.diagnostics.preprocess_mode.as_deref(), Some("ppo"));
	}

	#[test]
	fn settings_empty_object_is_all_defaults() {
		let options = replaced(json!({}));

		assert!(options.formatter.trim_empty_lines);
		assert!(options.diagnostics.enabled_on_save);
		assert_eq!(options.makensis.path, "");
	}

	#[test]
	fn settings_null_is_unavailable() {
		assert!(matches!(
			parse_settings(json!(null)),
			SettingsUpdate::Unavailable
		));
	}

	#[test]
	fn settings_null_section_is_unavailable() {
		assert!(matches!(
			parse_settings(json!({ "nsis": null })),
			SettingsUpdate::Unavailable
		));
	}

	#[test]
	fn settings_wrong_type_is_unparseable() {
		assert!(matches!(
			parse_settings(json!({ "formatter": { "print_width": "wide" } })),
			SettingsUpdate::Unparseable
		));
	}

	#[test]
	fn settings_unknown_keys_are_ignored() {
		let options = replaced(json!({
			"formatter": { "printWidth": 100 },
			"nonsense": true,
		}));

		assert_eq!(options.formatter.print_width, 0);
	}

	// ── LspState::from_options ──

	#[test]
	fn state_parses_end_of_line() {
		assert!(matches!(parse_end_of_line(Some("lf")), Some(EndOfLine::Lf)));
		assert!(matches!(
			parse_end_of_line(Some("crlf")),
			Some(EndOfLine::Crlf)
		));
		assert!(parse_end_of_line(Some("auto")).is_none());
		assert!(parse_end_of_line(None).is_none());
	}

	#[test]
	fn state_parses_comment_style() {
		assert!(matches!(
			parse_comment_style(Some("hash")),
			Some(CommentStyle::Hash)
		));
		assert!(matches!(
			parse_comment_style(Some("semi")),
			Some(CommentStyle::Semi)
		));
		assert!(parse_comment_style(Some("preserve")).is_none());
		assert!(parse_comment_style(None).is_none());
	}

	#[test]
	fn state_leaves_comment_markers_alone_by_default() {
		let state = LspState::from_options(replaced(json!({})));

		assert!(state.comment_style.is_none());
	}

	#[test]
	fn state_carries_formatter_options() {
		let state = LspState::from_options(replaced(json!({
			"formatter": {
				"end_of_line": "crlf",
				"print_width": 90,
				"trim_empty_lines": false,
				"single_quote": true,
			},
		})));

		assert!(matches!(state.end_of_line, Some(EndOfLine::Crlf)));
		assert_eq!(state.print_width, 90);
		assert!(!state.trim_empty_lines);
		assert!(state.single_quote);
	}
}
