//! What NSIS itself defines: the commands in the reference manual, the built-in
//! variables, the flag constants, and the commands that are still accepted but
//! no longer documented.
//!
//! The four tables are private. Callers ask one question — [`lookup`] — and get
//! back a [`Known`] saying which table answered, so no caller has to know there
//! is more than one, or which order they are searched in.

use std::collections::HashMap;
use std::sync::LazyLock;

pub struct DocEntry {
	pub name: String,
	pub description: String,
	pub parameters: Option<String>,
	pub example: Option<String>,
}

const DOCS_RAW: &str = include_str!("./llms-full.txt");

static DOCS: LazyLock<HashMap<String, DocEntry>> = LazyLock::new(|| {
	let mut map = HashMap::new();

	let start = match DOCS_RAW.find("\n# ") {
		Some(pos) => pos + 1,
		None => return map,
	};

	for section in DOCS_RAW[start..].split("\n# ") {
		let mut lines = section.lines();
		let name = match lines.next() {
			Some(n) => n.trim().to_string(),
			None => continue,
		};
		if name.is_empty() {
			continue;
		}

		let mut description = String::new();
		let mut parameters = None;
		let mut example = None;
		let mut subsection = "";
		let mut in_code = false;
		let mut code_buf = String::new();

		for line in lines {
			if line.starts_with("```") {
				if in_code {
					in_code = false;
					match subsection {
						"parameters" if parameters.is_none() => {
							parameters = Some(code_buf.trim().to_string());
						}
						"example" if example.is_none() => {
							example = Some(code_buf.trim().to_string());
						}
						_ => {}
					}
					code_buf.clear();
				} else {
					in_code = true;
				}
				continue;
			}
			if in_code {
				if !code_buf.is_empty() {
					code_buf.push('\n');
				}
				code_buf.push_str(line);
				continue;
			}
			if line.starts_with("> ") && description.is_empty() {
				description = line[2..].to_string();
			} else if line.starts_with("## Parameters") {
				subsection = "parameters";
			} else if line.starts_with("## Example") {
				subsection = "example";
			} else if line.starts_with("## ") {
				subsection = "";
			}
		}

		map.insert(
			name.to_lowercase(),
			DocEntry {
				name,
				description,
				parameters,
				example,
			},
		);
	}

	map
});

/// What NSIS knows a word to be.
///
/// The variants are ordered the way [`lookup`] searches: a documented command
/// wins over a variable of the same name, and a deprecated command is only
/// reported when nothing documented matches.
pub enum Known {
	/// A command or preprocessor keyword from the reference manual.
	Command(&'static DocEntry),
	/// A built-in variable, with its `$`.
	Variable {
		name: &'static str,
		description: &'static str,
	},
	/// A flag, registry-root or dialog constant.
	Constant {
		name: &'static str,
		description: &'static str,
	},
	/// A command NSIS no longer documents.
	Deprecated(&'static Deprecated),
}

/// A command NSIS no longer documents, and what can be said in its place.
pub struct Deprecated {
	pub name: &'static str,
	pub replacement: Replacement,
}

/// What NSIS offers instead of a deprecated command.
///
/// The three variants are three different situations, not three shades of one:
/// only a [`Swap`](Replacement::Swap) can be applied as a quickfix, because
/// only a `Swap` names a command that means the same thing.
pub enum Replacement {
	/// A modern command that does the same job — safe to substitute verbatim.
	Swap(&'static str),
	/// No drop-in exists, but there is something worth telling the user. A full
	/// sentence, rendered after the deprecation notice.
	Advice(&'static str),
	/// The command is gone and nothing takes its place.
	None,
}

impl Known {
	/// The canonical spelling, whatever the caller wrote.
	pub fn name(&self) -> &'static str {
		match self {
			Known::Command(entry) => &entry.name,
			Known::Variable { name, .. } | Known::Constant { name, .. } => name,
			Known::Deprecated(dep) => dep.name,
		}
	}
}

/// What NSIS defines `word` to be, or `None` if it defines nothing by that
/// name.
///
/// Matching is case-insensitive throughout. A word without a `!` still finds
/// the preprocessor keyword — the client asks about `include` while the user is
/// still typing `!include` — and a variable is found with or without its `$`.
pub fn lookup(word: &str) -> Option<Known> {
	if let Some(entry) = lookup_doc(word) {
		return Some(Known::Command(entry));
	}

	let bare = word.trim_start_matches('$');
	for (name, description) in BUILTIN_VARIABLES {
		if name.trim_start_matches('$').eq_ignore_ascii_case(bare)
			|| name.eq_ignore_ascii_case(word)
		{
			return Some(Known::Variable { name, description });
		}
	}

	for (name, description) in CONSTANTS {
		if name.eq_ignore_ascii_case(word) {
			return Some(Known::Constant { name, description });
		}
	}

	DEPRECATED_COMMANDS
		.iter()
		.find(|dep| dep.name.eq_ignore_ascii_case(word))
		.map(Known::Deprecated)
}

fn lookup_doc(word: &str) -> Option<&'static DocEntry> {
	let key = word.to_lowercase();
	DOCS.get(&key).or_else(|| {
		if !key.starts_with('!') {
			DOCS.get(&format!("!{}", key))
		} else {
			None
		}
	})
}

/// Every documented command, in no particular order.
pub fn commands() -> impl Iterator<Item = &'static DocEntry> {
	DOCS.values()
}

/// Every built-in variable, as `($NAME, description)`.
pub fn variables() -> impl Iterator<Item = (&'static str, &'static str)> {
	BUILTIN_VARIABLES.iter().copied()
}

/// Every constant, as `(NAME, description)`.
pub fn constants() -> impl Iterator<Item = (&'static str, &'static str)> {
	CONSTANTS.iter().copied()
}

/// Every deprecated command, in no particular order.
///
/// Unlike the other three tables, nothing in the server walks this one at
/// runtime — a deprecated command is never offered as a completion. It is here
/// so the tests that guard the table can be written as loops over it, and can
/// therefore never fall behind an entry added later.
#[cfg(test)]
pub fn deprecated() -> impl Iterator<Item = &'static Deprecated> {
	DEPRECATED_COMMANDS.iter()
}

const BUILTIN_VARIABLES: &[(&str, &str)] = &[
	("$INSTDIR", "The installation directory"),
	(
		"$OUTDIR",
		"The current output directory (set with SetOutPath)",
	),
	(
		"$EXEDIR",
		"The directory containing the installer executable",
	),
	("$EXEFILE", "The filename of the installer executable"),
	("$EXEPATH", "The full path of the installer executable"),
	("$LANGUAGE", "The currently selected language"),
	("$TEMP", "The system temporary directory"),
	(
		"$PLUGINSDIR",
		"The plugins directory (initialized by InitPluginsDir)",
	),
	("$WINDIR", "The Windows directory"),
	("$SYSDIR", "The Windows system directory"),
	("$PROGRAMFILES", "The Program Files directory"),
	("$PROGRAMFILES32", "The 32-bit Program Files directory"),
	("$PROGRAMFILES64", "The 64-bit Program Files directory"),
	("$COMMONFILES", "The Common Files directory"),
	("$DESKTOP", "The desktop directory"),
	("$STARTMENU", "The Start Menu directory"),
	("$SMPROGRAMS", "The Start Menu Programs directory"),
	("$SMSTARTUP", "The Start Menu Startup directory"),
	("$QUICKLAUNCH", "The Quick Launch directory"),
	("$DOCUMENTS", "The Documents directory"),
	("$SENDTO", "The Send To directory"),
	("$RECENT", "The Recent directory"),
	("$FAVORITES", "The Favorites directory"),
	("$MUSIC", "The Music directory"),
	("$PICTURES", "The Pictures directory"),
	("$VIDEOS", "The Videos directory"),
	("$NETHOOD", "The Network Neighborhood directory"),
	("$FONTS", "The Fonts directory"),
	("$TEMPLATES", "The Templates directory"),
	("$APPDATA", "The Application Data directory"),
	("$LOCALAPPDATA", "The Local Application Data directory"),
	("$PRINTHOOD", "The Print Neighborhood directory"),
	("$INTERNET_CACHE", "The Internet Cache directory"),
	("$COOKIES", "The Cookies directory"),
	("$HISTORY", "The History directory"),
	("$PROFILE", "The user profile directory"),
	("$ADMINTOOLS", "The Administrative Tools directory"),
	("$RESOURCES", "The Resources directory"),
	("$RESOURCES_LOCALIZED", "The Localized Resources directory"),
	("$CDBURN_AREA", "The CD Burning directory"),
	("$HWNDPARENT", "The HWND of the parent (installer) window"),
	("$CMDLINE", "The command line of the installer"),
	("$NSISDIR", "The NSIS directory"),
	("$0", "User variable $0"),
	("$1", "User variable $1"),
	("$2", "User variable $2"),
	("$3", "User variable $3"),
	("$4", "User variable $4"),
	("$5", "User variable $5"),
	("$6", "User variable $6"),
	("$7", "User variable $7"),
	("$8", "User variable $8"),
	("$9", "User variable $9"),
	("$R0", "User variable $R0"),
	("$R1", "User variable $R1"),
	("$R2", "User variable $R2"),
	("$R3", "User variable $R3"),
	("$R4", "User variable $R4"),
	("$R5", "User variable $R5"),
	("$R6", "User variable $R6"),
	("$R7", "User variable $R7"),
	("$R8", "User variable $R8"),
	("$R9", "User variable $R9"),
];

/// Commands NSIS no longer documents.
///
/// The `Swap` targets and the `Advice` wording come from `makensis -CMDHELP` on
/// NSIS 3.12: a `Swap` is one the compiler still accepts and that means the same
/// thing, `Advice` is one it accepts but that no longer does anything useful,
/// and `None` is one it rejects outright.
const DEPRECATED_COMMANDS: &[Deprecated] = &[
	Deprecated {
		name: "CompareDLLVersions",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "CompareFileTimes",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "DirShow",
		replacement: Replacement::Advice("It does not currently work."),
	},
	Deprecated {
		name: "DisabledBitmap",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "EnabledBitmap",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "GetFullDLLPath",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "GetParent",
		replacement: Replacement::Advice("Use the ${GetParent} macro from FileFunc.nsh instead."),
	},
	Deprecated {
		name: "GetWinampInstPath",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "LangStringUP",
		replacement: Replacement::Swap("LangString"),
	},
	Deprecated {
		name: "PackEXEHeader",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "SectionDivider",
		replacement: Replacement::None,
	},
	Deprecated {
		name: "SetPluginUnload",
		replacement: Replacement::Advice("Plug-ins should handle unloading on their own."),
	},
	Deprecated {
		name: "SubSection",
		replacement: Replacement::Swap("SectionGroup"),
	},
	Deprecated {
		name: "SubSectionEnd",
		replacement: Replacement::Swap("SectionGroupEnd"),
	},
	Deprecated {
		name: "UninstallExeName",
		replacement: Replacement::Advice("Use WriteUninstaller from a section instead."),
	},
];

const CONSTANTS: &[(&str, &str)] = &[
	("MB_OK", "OK button only"),
	("MB_OKCANCEL", "OK and Cancel buttons"),
	("MB_ABORTRETRYIGNORE", "Abort, Retry, and Ignore buttons"),
	("MB_RETRYCANCEL", "Retry and Cancel buttons"),
	("MB_YESNO", "Yes and No buttons"),
	("MB_YESNOCANCEL", "Yes, No, and Cancel buttons"),
	("MB_ICONEXCLAMATION", "Exclamation mark icon"),
	("MB_ICONINFORMATION", "Information icon"),
	("MB_ICONQUESTION", "Question mark icon"),
	("MB_ICONSTOP", "Stop sign icon"),
	("MB_USERICON", "User-defined icon"),
	("MB_TOPMOST", "Make the message box topmost"),
	("MB_SETFOREGROUND", "Set the message box as foreground"),
	("MB_RIGHT", "Right-aligned text"),
	("MB_RTLREADING", "Right-to-left reading"),
	("MB_DEFBUTTON1", "First button is default"),
	("MB_DEFBUTTON2", "Second button is default"),
	("MB_DEFBUTTON3", "Third button is default"),
	("MB_DEFBUTTON4", "Fourth button is default"),
	("IDABORT", "Abort button was clicked"),
	("IDCANCEL", "Cancel button was clicked"),
	("IDIGNORE", "Ignore button was clicked"),
	("IDNO", "No button was clicked"),
	("IDOK", "OK button was clicked"),
	("IDRETRY", "Retry button was clicked"),
	("IDYES", "Yes button was clicked"),
	("HKCR", "HKEY_CLASSES_ROOT"),
	("HKCU", "HKEY_CURRENT_USER"),
	("HKLM", "HKEY_LOCAL_MACHINE"),
	("HKU", "HKEY_USERS"),
	("HKCC", "HKEY_CURRENT_CONFIG"),
	("HKDD", "HKEY_DYN_DATA"),
	("HKPD", "HKEY_PERFORMANCE_DATA"),
	("HKCR32", "HKEY_CLASSES_ROOT (32-bit view)"),
	("HKCR64", "HKEY_CLASSES_ROOT (64-bit view)"),
	("HKCU32", "HKEY_CURRENT_USER (32-bit view)"),
	("HKCU64", "HKEY_CURRENT_USER (64-bit view)"),
	("HKLM32", "HKEY_LOCAL_MACHINE (32-bit view)"),
	("HKLM64", "HKEY_LOCAL_MACHINE (64-bit view)"),
	(
		"SHCTX",
		"SHELL_CONTEXT (HKLM or HKCU based on SetShellVarContext)",
	),
	("SW_HIDE", "Hide window"),
	("SW_SHOWNORMAL", "Show window normally"),
	("SW_SHOWMINIMIZED", "Show window minimized"),
	("SW_SHOWMAXIMIZED", "Show window maximized"),
	("SW_SHOWDEFAULT", "Show window in default state"),
	("ARCHIVE", "Archive file attribute"),
	("HIDDEN", "Hidden file attribute"),
	("NORMAL", "Normal file attribute"),
	("OFFLINE", "Offline file attribute"),
	("READONLY", "Read-only file attribute"),
	("SYSTEM", "System file attribute"),
	("TEMPORARY", "Temporary file attribute"),
	("FILE_ATTRIBUTE_ARCHIVE", "Archive file attribute"),
	("FILE_ATTRIBUTE_HIDDEN", "Hidden file attribute"),
	("FILE_ATTRIBUTE_NORMAL", "Normal file attribute"),
	("FILE_ATTRIBUTE_OFFLINE", "Offline file attribute"),
	("FILE_ATTRIBUTE_READONLY", "Read-only file attribute"),
	("FILE_ATTRIBUTE_SYSTEM", "System file attribute"),
	("FILE_ATTRIBUTE_TEMPORARY", "Temporary file attribute"),
	("IDD_LICENSE", "License page dialog ID"),
	("IDD_DIR", "Directory page dialog ID"),
	("IDD_SELCOM", "Component selection dialog ID"),
	("IDD_INST", "Install page dialog ID"),
	("IDD_INSTFILES", "Install files page dialog ID"),
	("IDD_UNINST", "Uninstall page dialog ID"),
	("IDD_VERIFY", "Verify page dialog ID"),
];

#[cfg(test)]
mod tests {
	use super::*;

	/// The variant that answered, for assertions that only care about which
	/// table a word came from.
	fn kind_of(word: &str) -> Option<&'static str> {
		Some(match lookup(word)? {
			Known::Command(_) => "command",
			Known::Variable { .. } => "variable",
			Known::Constant { .. } => "constant",
			Known::Deprecated(_) => "deprecated",
		})
	}

	#[test]
	fn docs_parses_entries() {
		assert!(commands().next().is_some(), "no doc entries were parsed");
	}

	#[test]
	fn a_command_is_found_by_name() {
		assert_eq!(kind_of("Name"), Some("command"));
		assert_eq!(lookup("Name").unwrap().name(), "Name");
	}

	#[test]
	fn lookup_is_case_insensitive() {
		assert_eq!(lookup("name").unwrap().name(), "Name");
		assert_eq!(lookup("instdir").unwrap().name(), "$INSTDIR");
		assert_eq!(lookup("mb_ok").unwrap().name(), "MB_OK");
		assert_eq!(lookup("dirshow").unwrap().name(), "DirShow");
	}

	/// The client asks about `include` while the user is still typing
	/// `!include`.
	#[test]
	fn a_preprocessor_keyword_is_found_without_its_bang() {
		assert_eq!(lookup("include").unwrap().name(), "!include");
	}

	#[test]
	fn a_variable_is_found_with_or_without_its_dollar() {
		assert_eq!(lookup("$INSTDIR").unwrap().name(), "$INSTDIR");
		assert_eq!(lookup("INSTDIR").unwrap().name(), "$INSTDIR");
	}

	#[test]
	fn a_deprecated_command_reports_its_canonical_spelling() {
		assert_eq!(kind_of("subsection"), Some("deprecated"));
		assert_eq!(lookup("subsection").unwrap().name(), "SubSection");
	}

	#[test]
	fn an_unknown_word_is_unknown() {
		assert!(lookup("__nonexistent_command__").is_none());
	}

	/// `lookup` answers with the first table that matches, so a deprecated
	/// command that also appeared in the reference manual would come back as a
	/// `Command` — and the deprecation warning would silently stop firing.
	#[test]
	fn no_deprecated_command_is_also_documented() {
		for dep in deprecated() {
			assert_eq!(
				kind_of(dep.name),
				Some("deprecated"),
				"{} is shadowed by another table",
				dep.name
			);
		}
	}

	/// Every command a `Swap` points at has to be one the reference manual
	/// documents, or the quickfix would trade a deprecated command for a
	/// nonexistent one.
	#[test]
	fn every_swap_target_is_a_documented_command() {
		for dep in deprecated() {
			let Replacement::Swap(target) = dep.replacement else {
				continue;
			};
			assert_eq!(
				kind_of(target),
				Some("command"),
				"{} points at {target}, which is not a documented command",
				dep.name
			);
		}
	}

	#[test]
	fn builtin_variables_start_with_dollar() {
		for (var, _) in variables() {
			assert!(var.starts_with('$'), "{var} should start with $");
		}
	}

	#[test]
	fn the_tables_are_populated() {
		assert!(variables().next().is_some());
		assert!(constants().next().is_some());
	}
}
