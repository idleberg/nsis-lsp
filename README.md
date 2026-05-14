# nsis-lsp

![Crates.io License](https://img.shields.io/crates/l/ardent?style=for-the-badge)
[![Crates.io Version](https://img.shields.io/crates/v/nsis-lsp?style=for-the-badge)](https://crates.io/crates/nsis-lsp)
[![CI](https://img.shields.io/github/actions/workflow/status/idleberg/nsis-lsp/ci.yml?style=for-the-badge)](https://github.com/idleberg/nsis-lsp/actions)

> An opinionated language server for NSIS.

## Description

While still in experimental stage, this language server provides the following features:

- syntax highlighting
- code formatting
- go-to-definition
- on-hover information

## Installation

### crates.io

```sh
cargo install nsis-lsp
```

### Source

```sh
git clone https://github.com/idleberg/nsis-lsp.git
cd nsis-lsp
cargo build --release
```

The binary is at `target/release/nsis-lsp`.

## License

This work is licensed under the [Apache License, Version 2.0](LICENSE).
