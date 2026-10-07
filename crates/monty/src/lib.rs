#![doc = include_str!("../README.md")]
// these files first because they include macros for the rest of the crate to use
mod boundary_uuid;
mod heap;
mod heap_traits;

mod args;
mod asyncio;
mod builtins;
mod bytecode;
mod codecs;
mod dump_format;
mod exception_private;
mod expressions;
mod fstring;
mod function;
mod hash;
mod heap_data;
mod identity;
mod intern;
mod modules;
mod name_map;
mod namespace;
mod object_bridge;
mod os_dispatch;
mod parse;
mod percent_format;
mod predicate;
mod prepare;
mod repl;
mod resource_checks;
mod run;
mod run_progress;
mod sorting;
mod source_map;
mod source_nesting;
mod str_format;
mod string_builder;
mod stringize;

/// Render an expression AST without executing it. Useful to hosts that seal
/// captures before compiling an expression. The AST follows the pinned Ruff version.
#[must_use]
pub fn expression_source(expression: &ruff_python_ast::Expr) -> String {
    stringize::stringize_annotation(&mut expression.clone())
}
/// Render a statement AST without evaluating it.
pub fn statement_source(statement: &ruff_python_ast::Stmt) -> String {
    ruff_python_codegen::Generator::new(
        &ruff_python_codegen::Indentation::default(),
        ruff_source_file::LineEnding::Lf,
    )
    .stmt(statement)
}
mod types;
mod value;
mod virtual_path;

#[cfg(feature = "ref-count-return")]
pub use crate::run::RefCountOutput;
pub use crate::{
    dump_format::{
        DUMP_VERSION, Dump, DumpDecodeError, DumpEncodeError, DumpError, MIN_SUPPORTED_DUMP_VERSION, Session,
        SessionRef, dump,
    },
    repl::{
        CheckedSource, MontyRepl, ReplContinuationMode, ReplFunctionCall, ReplNameLookup, ReplOsCall, ReplProgress,
        ReplResolveFutures, ReplStartError, detect_repl_continuation_mode,
    },
    run::MontyRun,
    run_progress::{FunctionCall, NameLookup, OsCall, ResolveFutures, RunProgress},
    source_nesting::source_within_nesting_bound,
};
