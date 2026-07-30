//! One module per LSP feature, each answering the request the server advertised
//! a capability for.
//!
//! A handler is an adapter: it takes the LSP params, asks [`Workspace`] or
//! [`nsis_data`] whatever the feature needs to know, and renders the answer in
//! the shape the protocol wants. Nothing here holds state — dispatch in
//! [`crate::main`] hands each one the workspace and the settings it needs.
//!
//! [`Workspace`]: crate::workspace::Workspace
//! [`nsis_data`]: crate::nsis_data

pub mod code_action;
pub mod completion;
pub mod formatting;
pub mod hover;
pub mod navigation;
pub mod rename;
pub mod signature;
pub mod symbols;
