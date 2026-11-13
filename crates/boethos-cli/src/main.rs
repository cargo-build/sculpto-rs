//! `boethos` — agent-first asset catalogue CLI.
//!
//! Every command is non-interactive and prints a stable text summary or one
//! JSON document. The database is local state; exports are the reviewable
//! snapshot.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use boethos::Store;
use boethos_catalogue::{Asset, Assets, Category, IndexOptions, Sex, index_root};

fn main() -> ExitCode {
    let mut arguments: Vec<String> = env::args().skip(1).collect();
    if let Some(index) = arguments.iter().position(|argument| argument == "--db")
        && let Some(value) = arguments.get(index + 1).cloned()
    {
        arguments.drain(index..=index + 1);
        arguments.push("--db".to_string());
        arguments.push(value);
    }
    if arguments.is_empty() || arguments[0] == "help" || arguments[0] == "--help" {
        println!("{}", usage());
        return ExitCode::SUCCESS;
    }
    let command = arguments.remove(0);
    let result = match command.as_str() {
        "index" => command_index(&arguments),
        "list" => command_list(&arguments),
        "get" => command_get(&arguments),
        "stats" => command_stats(&arguments),
        "export" => command_export(&arguments),
        other => Err(format!("unknown command {other:?}; run `boethos help`")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn usage() -> &'static str {
    "usage: boethos [--db PATH] <command>

commands:
  index --root DIR [--root DIR ...] [--source LABEL] [--no-archives] [--json]
        index files and archive members into the catalogue
  list [--category C] [--sex S] [--family F] [--limit N] [--json]
        list indexed assets
  get ID [--json]
        print one asset
  stats [--json]
        print counts by category and the database path
  export PATH
        write one JSON object per record, in identity order"
}

fn command_index(arguments: &[String]) -> Result<(), String> {
    let roots = values(arguments, "--root");
    if roots.is_empty() {
        return Err("index requires at least one --root directory".to_string());
    }
    let source_flag = flag(arguments, "--source");
    let options = IndexOptions {
        archive_entries: !has(arguments, "--no-archives"),
    };
    let store = open(arguments)?;
    let mut inserted = 0usize;
    let mut replaced = 0usize;
    let mut files = 0usize;
    let mut archives = 0usize;
    let mut members = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for root in &roots {
        let root_path = Path::new(root);
        let source = source_flag.map(str::to_string).unwrap_or_else(|| {
            root_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(root)
                .to_string()
        });
        let report = index_root(&store, root_path, &source, &options);
        inserted += report.inserted;
        replaced += report.replaced;
        files += report.files;
        archives += report.archives;
        members += report.members;
        errors.extend(report.errors);
    }
    if has(arguments, "--json") {
        let json = serde_json::json!({
            "roots": roots,
            "files": files,
            "archives": archives,
            "members": members,
            "inserted": inserted,
            "replaced": replaced,
            "errors": errors,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
        );
    } else {
        println!(
            "indexed {files} files, {archives} archives, {members} members: inserted {inserted}, replaced {replaced}, errors {}",
            errors.len()
        );
        for error in &errors {
            eprintln!("warning: {error}");
        }
    }
    Ok(())
}

fn command_list(arguments: &[String]) -> Result<(), String> {
    let category = optional_parse(arguments, "--category", Category::parse)?;
    let sex = optional_parse(arguments, "--sex", Sex::parse)?;
    let family = flag(arguments, "--family").map(str::to_lowercase);
    let limit = flag(arguments, "--limit")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid --limit {value:?}"))
        })
        .transpose()?;
    let store = open(arguments)?;
    let rows = store
        .iter_kind::<Assets>()
        .map_err(|error| error.to_string())?;
    let mut assets: Vec<Asset> = rows
        .into_iter()
        .map(|(_, asset)| asset)
        .filter(|asset| category.is_none_or(|category| asset.category == category))
        .filter(|asset| sex.is_none_or(|sex| asset.sex == Some(sex)))
        .filter(|asset| {
            family
                .as_deref()
                .is_none_or(|family| asset.family.as_deref() == Some(family))
        })
        .collect();
    assets.sort_by(|left, right| {
        (left.category, &left.name, &left.id).cmp(&(right.category, &right.name, &right.id))
    });
    if let Some(limit) = limit {
        assets.truncate(limit);
    }
    if has(arguments, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&assets).map_err(|error| error.to_string())?
        );
    } else {
        for asset in &assets {
            println!(
                "{}\t{}\t{}\t{}",
                asset.id,
                asset.category,
                asset.sex.map(Sex::as_str).unwrap_or("-"),
                asset.name
            );
        }
        println!("{} assets", assets.len());
    }
    Ok(())
}

fn command_get(arguments: &[String]) -> Result<(), String> {
    let id = positional(arguments).ok_or_else(|| "get requires an asset id".to_string())?;
    let store = open(arguments)?;
    let asset = store
        .get::<Assets>(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("asset {id} not found"))?;
    if has(arguments, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&asset).map_err(|error| error.to_string())?
        );
    } else {
        println!("id:       {}", asset.id);
        println!("category: {}", asset.category);
        println!("name:     {}", asset.name);
        println!("path:     {}", asset.path);
        println!("source:   {}", asset.source);
        println!("format:   {}", asset.format);
        if let Some(bytes) = asset.bytes {
            println!("bytes:    {bytes}");
        }
        if let Some(sex) = asset.sex {
            println!("sex:      {}", sex.as_str());
        }
        if let Some(family) = &asset.family {
            println!("family:   {family}");
        }
        println!("status:   {:?}", asset.status);
        if let Some(archive) = &asset.archive {
            println!("archive:  {archive}");
        }
        if let Some(entry) = &asset.entry {
            println!("entry:    {entry}");
        }
    }
    Ok(())
}

fn command_stats(arguments: &[String]) -> Result<(), String> {
    let path = database_path(arguments);
    let store = open(arguments)?;
    let rows = store
        .iter_kind::<Assets>()
        .map_err(|error| error.to_string())?;
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (_, asset) in &rows {
        *counts.entry(asset.category.as_str()).or_default() += 1;
    }
    let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    if has(arguments, "--json") {
        let json = serde_json::json!({
            "db": path,
            "bytes": bytes,
            "total": rows.len(),
            "categories": counts,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
        );
    } else {
        println!("db:      {}", path.display());
        println!("bytes:   {bytes}");
        println!("total:   {}", rows.len());
        for (category, count) in &counts {
            println!("{category:<12} {count}");
        }
    }
    Ok(())
}

fn command_export(arguments: &[String]) -> Result<(), String> {
    let out = positional(arguments).ok_or_else(|| "export requires an output path".to_string())?;
    let store = open(arguments)?;
    let count = store.export_jsonl(out).map_err(|error| error.to_string())?;
    println!("exported {count} records to {out}");
    Ok(())
}

fn open(arguments: &[String]) -> Result<Store, String> {
    let path = database_path(arguments);
    Store::open(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn database_path(arguments: &[String]) -> PathBuf {
    if let Some(path) = flag(arguments, "--db") {
        return PathBuf::from(path);
    }
    if let Ok(path) = env::var("BOETHOS_DB") {
        return PathBuf::from(path);
    }
    let state = env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_string())).join(".local/state")
        });
    state.join("boethos/assets.redb")
}

fn flag<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
}

fn has(arguments: &[String], name: &str) -> bool {
    arguments.iter().any(|argument| argument == name)
}

fn values<'a>(arguments: &'a [String], name: &str) -> Vec<&'a str> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == name
            && let Some(value) = arguments.get(index + 1)
        {
            values.push(value.as_str());
            index += 2;
            continue;
        }
        index += 1;
    }
    values
}

const VALUE_FLAGS: [&str; 7] = [
    "--db",
    "--category",
    "--sex",
    "--family",
    "--limit",
    "--root",
    "--source",
];

fn positional(arguments: &[String]) -> Option<&str> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        if VALUE_FLAGS.contains(&argument) {
            index += 2;
            continue;
        }
        if argument.starts_with("--") {
            index += 1;
            continue;
        }
        return Some(argument);
    }
    None
}

fn optional_parse<T>(
    arguments: &[String],
    name: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, String> {
    match flag(arguments, name) {
        None => Ok(None),
        Some(value) => parse(value)
            .map(Some)
            .ok_or_else(|| format!("invalid {name} {value:?}")),
    }
}

#[allow(dead_code)]
fn path_display(path: &Path) -> String {
    path.display().to_string()
}
