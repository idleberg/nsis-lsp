# nsis-lsp

![Crates.io License](https://img.shields.io/crates/l/nsis-lsp?style=for-the-badge)
[![Crates.io Version](https://img.shields.io/crates/v/nsis-lsp?style=for-the-badge)](https://crates.io/crates/nsis-lsp)
[![npm Version](https://img.shields.io/npm/v/@nsis/lsp?style=for-the-badge)](https://www.npmjs.org/package/@nsis/lsp)
[![CI](https://img.shields.io/github/actions/workflow/status/idleberg/nsis-lsp/ci.yml?style=for-the-badge)](https://github.com/idleberg/nsis-lsp/actions)

> An opinionated language server for NSIS.

## Description

While still in an **experimental stage**, this language server already provides some useful features:

- code actions
- code formatting
- compiler diagnostics
- completions
- document symbols
- find references
- go-to-definition
- on-hover information
- rename symbol
- signature help
- syntax highlighting

## Installation

### Cargo

```sh
cargo install nsis-lsp
```

### Scoop

```sh
scoop bucket add nsis https://github.com/NSIS-Dev/scoop-nsis
scoop install nsis/lsp
```

### Homebrew

```sh
brew install idleberg/asahi/nsis-lsp
```

### Source

```sh
git clone https://github.com/idleberg/nsis-lsp.git
cd nsis-lsp
cargo build --release
```

The binary is at `target/release/nsis-lsp`.

## Configuration

The language server accepts settings via LSP `initializationOptions`:

| Setting                       | Type             | Default | Description                                                                             |
| ----------------------------- | ---------------- | ------- | --------------------------------------------------------------------------------------- |
| `diagnostics.preprocess_mode` | `string \| null` | `"ppo"` | Preprocessor mode for `makensis`: `"ppo"`, `"safe_ppo"`, or `null` for full compilation |
| `diagnostics.enabled_on_save` | `boolean`        | `true`  | Run compiler diagnostics on save                                                        |
| `formatter.print_width`       | `number`         | `0`     | Maximum line width before breaking with `\` continuations (`0` disables wrapping)       |
| `makensis.path`               | `string`         | `""`    | Custom path to the `makensis` binary (uses `PATH` when empty)                           |

## License

This work is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or [The MIT License](LICENSE-MIT).
