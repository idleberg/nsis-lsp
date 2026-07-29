use clap::Parser;

// The server speaks LSP over stdio and takes no runtime configuration from the
// command line — settings arrive via `initializationOptions`. Parsing exists so
// that `--version` and `--help` behave like any other Rust binary.
//
// Note: use `//` and not `///` here. Doc comments become clap's `long_about`,
// which would put these implementation notes into user-facing `--help` output.
#[derive(Debug, Parser)]
#[command(name = env!("CARGO_PKG_NAME"), version, about = env!("CARGO_PKG_DESCRIPTION"))]
pub struct Cli {}

#[cfg(test)]
mod tests {
	use super::*;
	use clap::CommandFactory;

	#[test]
	fn cli_definition_is_valid() {
		Cli::command().debug_assert();
	}
}
