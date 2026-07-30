# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

Tasks are defined in `mise.toml`; CI runs `mise run checks` followed by `mise run build`.

```sh
mise run checks        # format:check + lint + test (what CI runs)
mise run test          # cargo test
mise run lint          # cargo clippy -- --deny warnings
mise run format        # cargo fmt (a PostToolUse hook already runs this after every .rs edit)
mise run build         # cargo build --release
mise run install       # cargo install --path .
```

Single test: `cargo test <name substring>` — e.g. `cargo test settings_null_is_unavailable`. Tests live in `#[cfg(test)] mod tests` blocks inside each module, so `cargo test symbols::` scopes to one module.

Rust edition 2024. `rustfmt.toml` sets `hard_tabs = true`; the repo indents with tabs throughout.

## Architecture

A synchronous, single-threaded LSP server over stdio built on `lsp-server`/`lsp-types`. There is no async runtime and no request cancellation: [main.rs](src/main.rs) blocks on `connection.receiver` and handles one message to completion before the next.

Three pieces of state exist, all owned by `main`:

- `LspState` — settings, built from `initializationOptions` at startup and rebuilt wholesale on `workspace/didChangeConfiguration`. `parse_settings` accepts the options bare or under an `nsis` section, and distinguishes `Unavailable` (client sent `null`) from `Unparseable` (logged, settings kept).
- `Workspace` ([workspace.rs](src/workspace.rs)) — the open documents. Each `Document` re-derives its `DocumentIndex` and deprecation diagnostics on every open/change; document sync is FULL, so a change replaces the text.
- `Client` ([client.rs](src/client.rs)) — the trait for everything the server says outward. `Stdio` writes to the real connection; `Recorder` captures messages in memory so `handle_request`/`handle_notification` can be driven directly in tests and the resulting responses, diagnostics and log lines asserted on.

Request and notification dispatch are two flat `match`/`if let` chains in `main.rs`; each `handle_*` function is a pure-ish function of `(&Workspace, params, &LspState)`. Adding a feature means advertising it in `capabilities`, adding an arm, and writing the handler beside its siblings.

### Knowledge about NSIS

[nsis_data.rs](src/nsis_data.rs) is the single source of truth for what NSIS itself defines. Four private tables — commands parsed at first use from the bundled [llms-full.txt](src/llms-full.txt) (~20k lines of NSIS docs, `include_str!`ed into the binary), plus `const` tables of built-in variables, flag constants and deprecated commands — sit behind one entry point, `lookup(word) -> Option<Known>`. Callers branch on the returned `Known` variant rather than knowing which table answered or in what order they are searched. Sigils (`$`, `!`, `${…}`) and case are normalised inside `lookup`. New NSIS facts belong in a table here, not in a handler.

### Text handling

There is no parser. Every feature works line-by-line over the raw text:

- [context.rs](src/context.rs) decides whether a position is `Code`, `Comment` or `String` (`context_at`), and `CodeScan` blanks comment bytes while preserving offsets so a scan over "code only" still reports columns that address the raw line. Strings are line-local, matching makensis.
- [position.rs](src/position.rs) converts between byte offsets and the UTF-16 columns LSP speaks. Handlers must convert at the boundary — LSP positions are never byte offsets.
- [symbols.rs](src/symbols.rs) builds the two-deep `DocumentIndex` (containers hold the labels inside them, the shape `documentSymbol` wants) and provides `find_references`. Lookups live on `DocumentIndex` so one matching rule serves every call site.

### Diagnostics, in two layers

`deprecation::scan` runs on open/change and needs nothing but the text. Compiler diagnostics run on save only, shell out to `makensis` ([compiler.rs](src/compiler.rs)) with `-PPO`/`-SAFEPPO` per `diagnostics.preprocess_mode`, and are parsed by [diagnostics.rs](src/diagnostics.rs); they need a file on disk, so they are merged on top at publish time rather than stored on the `Document`. When no `makensis` is found the server logs a warning once and stays silent thereafter.

[deprecation.rs](src/deprecation.rs) owns the diagnostic, the hover and the quickfix for deprecated commands as one feature over one table. The canonical command name travels in `Diagnostic::data` and the diagnostic is tagged with `code: "deprecated-command"` — `deprecation::fix` answers only for its own diagnostics and never parses the human-readable message.

### Formatting

The formatter is the external `ardent` crate; this repo only maps settings onto `FormatterOptions` and returns one whole-document `TextEdit`. Indentation deliberately comes from the per-request `FormattingOptions` (`tabSize`, `insertSpaces`) rather than from settings. A format failure both publishes a diagnostic at the parsed error position and shows a message, then answers with no edits.

## Conventions

- Comments explain *why*, and are written as prose about the behaviour ("Clients that hold their settings server-side send `null`…"). Test names read as sentences: `rename_edits_every_open_document`. Follow both.
- Commits are Conventional Commits with lowercase, intent-first subjects (`fix: include sigils in completions`, `refactor: put a seam on the client connection`).
- `src/cli.rs` uses `//` and not `///` on purpose — doc comments would leak into `--help`.
- The `packages/` directory holds npm wrapper packages (`@nsis/lsp` plus per-platform binaries) filled in by the release workflow; versions there are placeholders bumped by CI, not by hand.

## Checking NSIS syntax

`makensis -CMDHELP <keyword>` prints the authoritative parameter syntax for a command — use it before adding or correcting an entry in `nsis_data.rs`.

## Testing Requirements

- Every new feature and bugfix must include a corresponding test.
- When modifying formatter behavior, verify that existing tests still pass — and update them if the expected output intentionally changed. Editing existing tests always requires user confirmation.
- Run `mise run test` (or the full `mise run checks`) to confirm.

## Code Style

- Rust edition 2024
- Indentation: tabs (see `.editorconfig`)
- `cargo fmt` handles Rust formatting automatically (enforced by hooks)
- `#![warn(missing_docs)]` is enabled — public items need doc comments
