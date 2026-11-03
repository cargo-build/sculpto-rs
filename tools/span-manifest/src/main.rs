//! Exact function spans and token counts for the Sculpto source map.
//!
//! `syn` parses the file, so a reported span is a real declaration span, not a
//! guessed regex boundary. Token counts come from the `proc-macro2` fallback
//! lexer over the exact span text; a group counts as one token plus its
//! contents. Declaration-only trait methods are marked, never silently omitted.
//!
//! Usage:
//!   span-manifest <path>...            print the manifest to stdout
//!   span-manifest --check <path>...    compare against doc/map/functions.json
//!
//! Paths may be directories; `.rs` files are collected recursively.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use proc_macro2::{Span, TokenStream, TokenTree};
use syn::spanned::Spanned;

const TOKENIZER: &str = "proc-macro2 1.0 fallback lexer; group = 1 token + contents";
const MANIFEST: &str = "doc/map/functions.json";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.first().is_some_and(|arg| arg == "--check");
    if check {
        args.remove(0);
    }
    if args.is_empty() {
        eprintln!("usage: span-manifest [--check] <path>...");
        return ExitCode::from(2);
    }
    let mut files = Vec::new();
    for arg in &args {
        match collect(Path::new(arg), &mut files) {
            Ok(()) => {}
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    }
    files.sort();
    files.dedup();
    let manifest = match build(&files) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let rendered = format!("{manifest}\n");
    if check {
        return match std::fs::read_to_string(MANIFEST) {
            Ok(existing) if existing == rendered => {
                println!("ok: {MANIFEST} is fresh");
                ExitCode::SUCCESS
            }
            Ok(_) => {
                eprintln!("STALE: {MANIFEST} — run: just map");
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("error: {MANIFEST}: {error}");
                ExitCode::FAILURE
            }
        };
    }
    print!("{rendered}");
    ExitCode::SUCCESS
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_dir() {
        let entries = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", path.display()))?;
            collect(&entry.path(), files)?;
        }
        return Ok(());
    }
    if path.extension().is_some_and(|ext| ext == "rs") {
        files.push(path.to_path_buf());
    }
    Ok(())
}

fn build(files: &[PathBuf]) -> Result<serde_json::Value, String> {
    let mut functions = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let parsed = syn::parse_file(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let display = path.display().to_string();
        visit_items(&display, &text, &parsed.items, &mut functions);
    }
    functions.sort_by(|left, right| {
        (
            left["file"].as_str(),
            left["start_line"].as_u64(),
            left["name"].as_str(),
        )
            .cmp(&(
                right["file"].as_str(),
                right["start_line"].as_u64(),
                right["name"].as_str(),
            ))
    });
    let total_tokens: u64 = functions
        .iter()
        .filter_map(|item| item["tokens"].as_u64())
        .sum();
    Ok(serde_json::json!({
        "tokenizer": TOKENIZER,
        "functions": functions,
        "totals": {
            "functions": functions.len(),
            "tokens": total_tokens,
        },
    }))
}

fn visit_items(file: &str, text: &str, items: &[syn::Item], out: &mut Vec<serde_json::Value>) {
    for item in items {
        match item {
            syn::Item::Fn(item_fn) => push(
                file,
                text,
                None,
                "fn",
                &item_fn.sig.ident,
                span_of(item_fn),
                false,
                out,
            ),
            syn::Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    visit_items(file, text, items, out);
                }
            }
            syn::Item::Impl(implementation) => {
                let owner = type_name(&implementation.self_ty);
                for impl_item in &implementation.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        push(
                            file,
                            text,
                            owner.clone(),
                            "method",
                            &method.sig.ident,
                            span_of(method),
                            false,
                            out,
                        );
                    }
                }
            }
            syn::Item::Trait(trait_item) => {
                let owner = trait_item.ident.to_string();
                for trait_item in &trait_item.items {
                    if let syn::TraitItem::Fn(method) = trait_item {
                        push(
                            file,
                            text,
                            Some(owner.clone()),
                            "trait-method",
                            &method.sig.ident,
                            span_of(method),
                            method.default.is_none(),
                            out,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push(
    file: &str,
    text: &str,
    owner: Option<String>,
    kind: &str,
    ident: &syn::Ident,
    span: Span,
    declaration_only: bool,
    out: &mut Vec<serde_json::Value>,
) {
    let start_line = span.start().line;
    let end_line = span.end().line;
    let lines = end_line.saturating_sub(start_line) + 1;
    let tokens = token_count(text, &span).unwrap_or(0);
    out.push(serde_json::json!({
        "file": file,
        "owner": owner,
        "name": ident.to_string(),
        "kind": kind,
        "start_line": start_line,
        "end_line": end_line,
        "lines": lines,
        "tokens": tokens,
        "declaration_only": declaration_only,
    }));
}

fn span_of<T: Spanned>(item: &T) -> Span {
    item.span()
}

/// Tokenize the exact span text with the proc-macro2 lexer.
fn token_count(text: &str, span: &Span) -> Option<u64> {
    let lines: Vec<&str> = text.lines().collect();
    let start = span.start();
    let end = span.end();
    if start.line == 0 || end.line == 0 || end.line > lines.len() || start.line > end.line {
        return None;
    }
    let mut slice = String::new();
    let first = start.line - 1;
    let last = end.line - 1;
    for (index, content) in lines.iter().enumerate().take(end.line).skip(first) {
        let (from, to) = if index == first && index == last {
            (start.column, end.column)
        } else if index == first {
            (start.column, content.len())
        } else if index == last {
            (0, end.column)
        } else {
            (0, content.len())
        };
        if from <= to && to <= content.len() {
            slice.push_str(&content[from..to]);
            slice.push('\n');
        }
    }
    let stream = TokenStream::from_str(&slice).ok()?;
    Some(count(stream))
}

fn count(stream: TokenStream) -> u64 {
    stream
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Group(group) => 1 + count(group.stream()),
            _ => 1,
        })
        .sum()
}

fn type_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}
