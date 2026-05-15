use std::collections::HashMap;
use std::sync::LazyLock;

pub struct DocEntry {
	pub name: String,
	pub description: String,
	pub parameters: Option<String>,
	pub example: Option<String>,
}

const DOCS_RAW: &str = include_str!("./llms-full.txt");

pub static DOCS: LazyLock<HashMap<String, DocEntry>> = LazyLock::new(|| {
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

pub fn lookup_doc(word: &str) -> Option<&'static DocEntry> {
	let key = word.to_lowercase();
	DOCS.get(&key).or_else(|| {
		if !key.starts_with('!') {
			DOCS.get(&format!("!{}", key))
		} else {
			None
		}
	})
}

pub const BUILTIN_VARIABLES: &[(&str, &str)] = &[
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

pub const DEPRECATED_COMMANDS: &[&str] = &[
	"CompareDLLVersions",
	"CompareFileTimes",
	"DirShow",
	"DisabledBitmap",
	"EnabledBitmap",
	"GetFullDLLPath",
	"GetParent",
	"GetWinampInstPath",
	"LangStringUP",
	"PackEXEHeader",
	"SectionDivider",
	"SetPluginUnload",
	"SubSection",
	"SubSectionEnd",
	"UninstallExeName",
];

pub const CONSTANTS: &[(&str, &str)] = &[
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

	#[test]
	fn docs_parses_entries() {
		assert!(!DOCS.is_empty(), "DOCS should contain parsed entries");
	}

	#[test]
	fn lookup_doc_exact_command() {
		let entry = lookup_doc("Name");
		assert!(entry.is_some(), "should find 'Name' command");
	}

	#[test]
	fn lookup_doc_case_insensitive() {
		let entry = lookup_doc("name");
		assert!(entry.is_some(), "lookup should be case-insensitive");
	}

	#[test]
	fn lookup_doc_bang_prefix_fallback() {
		let entry = lookup_doc("include");
		assert!(
			entry.is_some(),
			"should find '!include' when given 'include'"
		);
		assert!(entry.unwrap().name.starts_with('!'));
	}

	#[test]
	fn lookup_doc_nonexistent() {
		assert!(lookup_doc("__nonexistent_command__").is_none());
	}

	#[test]
	fn builtin_variables_not_empty() {
		assert!(!BUILTIN_VARIABLES.is_empty());
	}

	#[test]
	fn builtin_variables_start_with_dollar() {
		for (var, _) in BUILTIN_VARIABLES {
			assert!(var.starts_with('$'), "{var} should start with $");
		}
	}

	#[test]
	fn constants_not_empty() {
		assert!(!CONSTANTS.is_empty());
	}

	#[test]
	fn deprecated_commands_not_empty() {
		assert!(!DEPRECATED_COMMANDS.is_empty());
	}
}
