//! Tree-sitter based syntax highlighting and lightweight code analysis for PRmator.
//!
//! The crate exposes three entry points:
//!
//! * [`detect`] maps a file path to a [`Lang`],
//! * [`highlight`] produces per-line highlight spans for rendering,
//! * [`symbols`] extracts definitions (functions, types, impls, ...) for outlines and
//!   "which function changed" annotations.
//!
//! Everything is safe to call from multiple threads concurrently. Grammars and queries are
//! compiled lazily on first use and cached for the lifetime of the process. All functions
//! tolerate partial or syntactically broken input (e.g. fragments of diff hunks) and never
//! panic on bad input.
//!
//! ```
//! use prmator_syntax::{detect, highlight, symbols, Highlight, Lang, SymbolKind};
//!
//! let lang = detect("src/main.rs").unwrap();
//! assert_eq!(lang, Lang::Rust);
//!
//! let source = "fn main() {}\n";
//! let lines = highlight(lang, source);
//! assert_eq!(lines.len(), 1);
//! assert_eq!(lines[0][0], (0..2, Highlight::Keyword));
//!
//! let syms = symbols(lang, source);
//! assert_eq!(syms[0].name, "main");
//! assert_eq!(syms[0].kind, SymbolKind::Function);
//! ```

mod highlight;
mod lang;
mod symbols;

pub use highlight::{highlight, Highlight, LineHighlights};
pub use lang::{detect, Lang};
pub use symbols::{symbols, Symbol, SymbolKind};

/// Inputs larger than this (in bytes) are not analysed: [`highlight`] returns empty line
/// entries and [`symbols`] returns no symbols.
pub const MAX_SOURCE_LEN: usize = 2 * 1024 * 1024;
