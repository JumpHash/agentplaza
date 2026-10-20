//! Compiles the scripts of HTML artifacts to plain JavaScript on the hub, with oxc.
//!
//! An artifact's TypeScript, TSX, JSX and JavaScript files are parsed and checked, their types are stripped, their
//! JSX becomes `React.createElement` calls, and they are emitted as an ES module. An import of a vendored library
//! ([`apz_view::libs`]) becomes a read of the global that library defines in the page, so a page loads only what
//! the hub vendored. Relative imports stay as they are, and anything else is refused.

use std::path::Path;

use apz_view::libs;
use oxc_allocator::Allocator;
use oxc_ast::ast::{ImportDeclaration, ImportDeclarationSpecifier, ImportOrExportKind, Statement};
use oxc_codegen::Codegen;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::Parser;
use oxc_semantic::{SemanticBuilder, SemanticBuilderReturn};
use oxc_span::SourceType;
use oxc_transformer::{JsxRuntime, TransformOptions, Transformer};

/// Compiles one script of an HTML artifact to plain JavaScript (an ES module). TypeScript types are stripped and
/// JSX becomes `React.createElement` (classic runtime). Imports of vendored libraries become reads of their
/// globals: `import React, {useState} from "react"` becomes `const React = window.React;` and
/// `const {useState} = window.React;`, `import * as d3 from "d3"` becomes `const d3 = window.d3;`,
/// `import {LineChart as LC} from "recharts"` becomes `const {LineChart: LC} = window.Recharts;`, and
/// `import "tailwindcss"` becomes nothing. Type-only imports are erased, relative imports are kept, and anything
/// else (a package that is not vendored, a network URL, a re-export from a package) is refused. The errors are
/// messages of the form `line:column: message`, one per problem.
pub fn compile(path: &str, source: &str) -> Result<String, Vec<String>> {
    let source_type = SourceType::from_path(Path::new(path))
        .map_err(|_| {
            vec![format!(
                "{path} is not a TypeScript, TSX, JSX or JavaScript file"
            )]
        })?
        .with_module(true);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(parsed
            .diagnostics
            .iter()
            .map(|d| message(source, d))
            .collect());
    }
    let mut program = parsed.program;
    let mut bindings: Vec<String> = vec![];
    let mut messages: Vec<String> = vec![];
    program
        .body
        .retain(|stmt| keep(stmt, source, &mut bindings, &mut messages));

    let SemanticBuilderReturn {
        semantic,
        diagnostics,
    } = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&program);
    messages.extend(diagnostics.iter().map(|d| message(source, d)));
    let scoping = semantic.into_scoping();

    let mut options = TransformOptions::default();
    options.jsx.jsx_plugin = true;
    options.jsx.runtime = JsxRuntime::Classic;
    let transformed = Transformer::new(&allocator, Path::new(path), &options)
        .build_with_scoping(scoping, &mut program);
    messages.extend(transformed.diagnostics.iter().map(|d| message(source, d)));
    if !messages.is_empty() {
        return Err(messages);
    }

    let code = Codegen::new().build(&program).code;
    let mut out = String::new();
    for line in bindings {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&code);
    Ok(out)
}

/// Whether `stmt` stays in the program. A vendored import is dropped and its `const` lines are pushed to
/// `bindings`. A re-export from anything but a file of the artifact is pushed to `messages` and the statement is
/// kept, so the caller refuses the file. A statement that cannot be compiled is handled the same way.
fn keep(
    stmt: &Statement,
    source: &str,
    bindings: &mut Vec<String>,
    messages: &mut Vec<String>,
) -> bool {
    let from = match stmt {
        Statement::ImportDeclaration(decl) => return import(decl, source, bindings, messages),
        Statement::ExportAllDeclaration(decl) => {
            Some((decl.source.value.to_string(), decl.span.start))
        }
        Statement::ExportFromDeclaration(decl) => {
            Some((decl.source.value.to_string(), decl.span.start))
        }
        _ => None,
    };
    if let Some((spec, start)) = from
        && !is_relative(&spec)
    {
        messages.push(format!(
            "{}: re-exporting from \"{spec}\" is refused; import it instead",
            location(source, start)
        ));
    }
    true
}

/// Whether an import declaration stays in the program (only a relative one does). A vendored import is replaced
/// by its `const` lines in `bindings`; a refused one is pushed to `messages`.
fn import(
    decl: &ImportDeclaration,
    source: &str,
    bindings: &mut Vec<String>,
    messages: &mut Vec<String>,
) -> bool {
    let spec = decl.source.value.to_string();
    if is_relative(&spec) {
        return true;
    }
    if decl.import_kind == ImportOrExportKind::Type {
        return false;
    }
    let at = location(source, decl.span.start);
    if apz_view::artifact::is_network(&spec) {
        messages.push(format!(
            "{at}: importing \"{spec}\" over the network is refused; use a vendored library"
        ));
        return false;
    }
    let Some(lib) = libs::lib(&spec) else {
        messages.push(format!(
            "{at}: importing \"{spec}\" is refused: it is not a vendored library"
        ));
        return false;
    };
    let specifiers: &[ImportDeclarationSpecifier] = match &decl.specifiers {
        Some(list) => list,
        None => &[],
    };
    if !lib.global.is_empty() {
        bindings.extend(binding_lines(specifiers, lib.global));
    }
    false
}

/// The `const` lines that bind the names of an import from a library whose page global is `global`. A default or
/// namespace import binds the whole global; named imports destructure it, and `default as x` binds the whole
/// global too. Type-only specifiers bind nothing.
fn binding_lines(specifiers: &[ImportDeclarationSpecifier], global: &str) -> Vec<String> {
    let mut lines = vec![];
    let mut named: Vec<String> = vec![];
    for specifier in specifiers {
        match specifier {
            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                lines.push(format!("const {} = window.{global};", s.local.name));
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                lines.push(format!("const {} = window.{global};", s.local.name));
            }
            ImportDeclarationSpecifier::ImportSpecifier(s) => {
                if s.import_kind == ImportOrExportKind::Type {
                    continue;
                }
                let imported = s.imported.name().to_string();
                let local = s.local.name.to_string();
                if imported == "default" {
                    lines.push(format!("const {local} = window.{global};"));
                } else if imported == local {
                    named.push(imported);
                } else {
                    named.push(format!("{imported}: {local}"));
                }
            }
        }
    }
    if !named.is_empty() {
        lines.push(format!("const {{{}}} = window.{global};", named.join(", ")));
    }
    lines
}

/// Whether an import specifier names a file of the artifact (`./x`, `../x` or `/x`) rather than a package or a
/// network host. A protocol-relative `//host` is a host, not a file.
fn is_relative(spec: &str) -> bool {
    spec.starts_with("./")
        || spec.starts_with("../")
        || (spec.starts_with('/') && !spec.starts_with("//"))
}

/// A diagnostic as `line:column: message`, positioned at its first label (line 1, column 1 without one).
fn message(source: &str, d: &OxcDiagnostic) -> String {
    let offset = d.labels.first().map_or(0, |label| label.offset());
    format!("{}: {}", location(source, offset), d.message)
}

/// The line and column (both from 1) of byte `offset` in `source`, counting columns in characters.
fn location(source: &str, offset: u32) -> String {
    let mut end = (offset as usize).min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let before = &source[..end];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("{line}:{column}")
}

#[cfg(test)]
mod tests {
    use super::compile;

    #[test]
    fn a_network_import_or_re_export_is_refused() {
        assert!(compile("App.tsx", "export { a } from \"https://x.example/a.js\";").is_err());
        assert!(compile("App.tsx", "export { a } from \"left-pad\";").is_err());
        assert!(compile("App.tsx", "export * from \"left-pad\";").is_err());
        assert!(compile("App.tsx", "import x from \"//x.example/a.js\";").is_err());
    }
}
