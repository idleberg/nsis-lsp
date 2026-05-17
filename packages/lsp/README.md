# @nsis/lsp

> npm distribution of the [nsis-lsp](https://crates.io/crates/nsis-lsp) language server.

## Installation

```sh
npm install @nsis/lsp
```

Platform-specific binaries are installed automatically via optional dependencies.

## Usage

After installation, the `nsis-lsp` binary is available:

```sh
npx nsis-lsp
```

The server communicates over stdio using the Language Server Protocol.

### Overriding the binary

Set the `NSIS_LSP_BINARY` environment variable to use a custom binary path:

```sh
NSIS_LSP_BINARY=/path/to/nsis-lsp npx nsis-lsp
```

## Supported Platforms

| Package                      | Platform           |
| ---------------------------- | ------------------ |
| `@nsis/lsp-darwin-arm64`     | macOS ARM64        |
| `@nsis/lsp-darwin-x64`       | macOS x64          |
| `@nsis/lsp-linux-arm64`      | Linux ARM64        |
| `@nsis/lsp-linux-x64`        | Linux x64          |
| `@nsis/lsp-linux-arm64-musl` | Linux ARM64 (musl) |
| `@nsis/lsp-linux-x64-musl`   | Linux x64 (musl)   |
| `@nsis/lsp-win32-x64`        | Windows x64        |
| `@nsis/lsp-win32-arm64`      | Windows ARM64      |

## License

This work is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or [The MIT License](LICENSE-MIT).
