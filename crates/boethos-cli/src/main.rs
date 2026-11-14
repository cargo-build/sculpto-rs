//! `boethos` — agent-first asset catalogue CLI.
//!
//! Every command is non-interactive and prints a stable text summary or one
//! JSON document. The database is local state; exports are the reviewable
//! snapshot. Arguments are validated strictly: a typo must fail, never fall
//! back to the default database or the whole catalogue.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use boethos::Store;
use boethos_catalogue::{Asset, Assets, Category, IndexOptions, Sex, index_root};

fn main() -> ExitCode {
    match parse_args(env::args().skip(1).collect()) {
        Ok(Invocation::Help) => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(Invocation::Version) => {
            println!("boethos {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Invocation::Run(cli)) => match run(&cli) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(2)
            }
        },
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

enum Invocation {
    Help,
    Version,
    Run(Cli),
}

struct Cli {
    db: Option<String>,
    command: String,
    values: BTreeMap<String, Vec<String>>,
    switches: BTreeSet<String>,
    positionals: Vec<String>,
}

impl Cli {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    fn values(&self, name: &str) -> &[String] {
        self.values.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    fn has(&self, name: &str) -> bool {
        self.switches.contains(name)
    }
}

struct Spec {
    values: &'static [&'static str],
    switches: &'static [&'static str],
    repeat: &'static [&'static str],
    max_positionals: usize,
}

fn spec(command: &str) -> Option<Spec> {
    Some(match command {
        "index" => Spec {
            values: &["--root", "--source"],
            switches: &["--no-archives", "--prune", "--json"],
            repeat: &["--root"],
            max_positionals: 0,
        },
        "list" => Spec {
            values: &[
                "--category",
                "--sex",
                "--family",
                "--source",
                "--tag",
                "--name",
                "--limit",
                "--offset",
            ],
            switches: &["--json"],
            repeat: &["--tag"],
            max_positionals: 0,
        },
        "get" => Spec {
            values: &[],
            switches: &["--json"],
            repeat: &[],
            max_positionals: 1,
        },
        "delete" => Spec {
            values: &[],
            switches: &[],
            repeat: &[],
            max_positionals: 1,
        },
        "stats" => Spec {
            values: &[],
            switches: &["--json"],
            repeat: &[],
            max_positionals: 0,
        },
        "export" => Spec {
            values: &[],
            switches: &[],
            repeat: &[],
            max_positionals: 1,
        },
        "help" => Spec {
            values: &[],
            switches: &[],
            repeat: &[],
            max_positionals: 0,
        },
        _ => return None,
    })
}

const VALUE_FLAGS: [&str; 10] = [
    "--db",
    "--root",
    "--source",
    "--category",
    "--sex",
    "--family",
    "--tag",
    "--name",
    "--limit",
    "--offset",
];

fn parse_args(mut arguments: Vec<String>) -> Result<Invocation, String> {
    expand_equals(&mut arguments);
    let db = take_db(&mut arguments)?;
    if arguments.is_empty() {
        return Ok(Invocation::Help);
    }
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
        || arguments[0] == "help"
    {
        return Ok(Invocation::Help);
    }
    if arguments[0] == "--version" || arguments[0] == "version" {
        return Ok(Invocation::Version);
    }
    let command = arguments.remove(0);
    let spec =
        spec(&command).ok_or_else(|| format!("unknown command {command:?}; run `boethos help`"))?;
    let mut cli = Cli {
        db,
        command,
        values: BTreeMap::new(),
        switches: BTreeSet::new(),
        positionals: Vec::new(),
    };
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].clone();
        if argument == "--help" || argument == "-h" {
            return Ok(Invocation::Help);
        }
        if argument.starts_with("--") {
            if spec.values.contains(&argument.as_str()) {
                let Some(value) = arguments
                    .get(index + 1)
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                else {
                    return Err(format!("{argument} requires a value"));
                };
                if !spec.repeat.contains(&argument.as_str()) && cli.values.contains_key(&argument) {
                    return Err(format!("duplicate {argument}"));
                }
                cli.values.entry(argument).or_default().push(value.clone());
                index += 2;
                continue;
            }
            if spec.switches.contains(&argument.as_str()) {
                if !cli.switches.insert(argument.clone()) {
                    return Err(format!("duplicate {argument}"));
                }
                index += 1;
                continue;
            }
            return Err(format!(
                "unknown option {argument:?} for `{}`; run `boethos help`",
                cli.command
            ));
        }
        if argument.starts_with('-') && argument.len() > 1 {
            return Err(format!(
                "unknown option {argument:?} for `{}`; run `boethos help`",
                cli.command
            ));
        }
        cli.positionals.push(argument);
        index += 1;
    }
    if cli.positionals.len() > spec.max_positionals {
        return Err(format!(
            "unexpected argument {:?} for `{}`",
            cli.positionals[spec.max_positionals], cli.command
        ));
    }
    Ok(Invocation::Run(cli))
}

fn expand_equals(arguments: &mut Vec<String>) {
    let mut expanded = Vec::with_capacity(arguments.len());
    for argument in arguments.drain(..) {
        if argument.starts_with("--")
            && let Some((name, value)) = argument.split_once('=')
            && VALUE_FLAGS.contains(&name)
        {
            expanded.push(name.to_string());
            expanded.push(value.to_string());
            continue;
        }
        expanded.push(argument);
    }
    *arguments = expanded;
}

fn take_db(arguments: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut db = None;
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] != "--db" {
            index += 1;
            continue;
        }
        if db.is_some() {
            return Err("duplicate --db".to_string());
        }
        let value = arguments
            .get(index + 1)
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .cloned()
            .ok_or_else(|| "--db requires a value".to_string())?;
        db = Some(value);
        arguments.drain(index..=index + 1);
    }
    Ok(db)
}

fn usage() -> &'static str {
    "usage: boethos [--db PATH] <command> [options]

commands:
  index --root DIR [--root DIR ...] [--source LABEL] [--no-archives] [--prune] [--json]
        index files and archive members; --prune removes rows for files
        that disappeared from the root
  list [--category C] [--sex S] [--family F] [--source S] [--tag T]
       [--name TEXT] [--offset N] [--limit N] [--json]
        list indexed assets; --name is a case-insensitive substring
  get ID [--json]
        print one asset; accepts an unambiguous 8+ character id prefix
  delete ID
        remove one asset and, for an archive, its indexed members
  stats [--json]
        print counts by category and the database path
  export PATH
        write one JSON object per record, in identity order
  help, --help, --version

database:
  --db PATH, else $BOETHOS_DB, else $XDG_STATE_HOME/boethos/assets.redb,
  else $HOME/.local/state/boethos/assets.redb

categories:
  body outfit hair face animation preset face_preset project
  texture mesh image archive other

sex:
  female male unknown

exit codes:
  0 success, 2 refusal (usage, missing database, failed index)

text list columns: id, category, sex, name, path (tab-separated, escaped)
json list: array of asset records; index: one report object"
}

fn run(cli: &Cli) -> Result<(), String> {
    match cli.command.as_str() {
        "index" => command_index(cli),
        "list" => command_list(cli),
        "get" => command_get(cli),
        "delete" => command_delete(cli),
        "stats" => command_stats(cli),
        "export" => command_export(cli),
        other => Err(format!("unknown command {other:?}; run `boethos help`")),
    }
}

fn command_index(cli: &Cli) -> Result<(), String> {
    let roots = cli.values("--root");
    if roots.is_empty() {
        return Err("index requires at least one --root directory".to_string());
    }
    for root in roots {
        if !Path::new(root).is_dir() {
            return Err(format!("{root}: not a directory"));
        }
    }
    let options = IndexOptions {
        archive_entries: !cli.has("--no-archives"),
        prune: cli.has("--prune"),
    };
    let store = open_write(cli)?;
    let mut report = boethos_catalogue::IndexReport::default();
    for root in roots {
        let root_path = Path::new(root);
        let source = cli
            .value("--source")
            .map(str::to_string)
            .unwrap_or_else(|| {
                root_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(root)
                    .to_string()
            });
        let root_report = index_root(&store, root_path, &source, &options);
        report.files += root_report.files;
        report.archives += root_report.archives;
        report.members += root_report.members;
        report.inserted += root_report.inserted;
        report.updated += root_report.updated;
        report.unchanged += root_report.unchanged;
        report.pruned += root_report.pruned;
        report.symlinks += root_report.symlinks;
        report.ignored_dirs += root_report.ignored_dirs;
        report.special += root_report.special;
        report.non_utf8 += root_report.non_utf8;
        report.unsupported_archives += root_report.unsupported_archives;
        report.errors.extend(root_report.errors);
    }
    if cli.has("--json") {
        let json = serde_json::json!({
            "roots": roots,
            "files": report.files,
            "archives": report.archives,
            "members": report.members,
            "inserted": report.inserted,
            "updated": report.updated,
            "unchanged": report.unchanged,
            "pruned": report.pruned,
            "symlinks": report.symlinks,
            "ignored_dirs": report.ignored_dirs,
            "special": report.special,
            "non_utf8": report.non_utf8,
            "unsupported_archives": report.unsupported_archives,
            "errors": report.errors,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).map_err(|error| error.to_string())?
        );
    } else {
        println!(
            "indexed {} files, {} archives, {} members: inserted {}, updated {}, unchanged {}, pruned {}, errors {}",
            report.files,
            report.archives,
            report.members,
            report.inserted,
            report.updated,
            report.unchanged,
            report.pruned,
            report.errors.len()
        );
        if report.symlinks
            + report.ignored_dirs
            + report.special
            + report.non_utf8
            + report.unsupported_archives
            > 0
        {
            println!(
                "skipped: {} symlinks, {} ignored directories, {} special files, {} non-UTF-8 paths, {} unsupported archives",
                report.symlinks,
                report.ignored_dirs,
                report.special,
                report.non_utf8,
                report.unsupported_archives
            );
        }
    }
    for error in &report.errors {
        eprintln!("warning: {error}");
    }
    if report.inserted + report.updated == 0 && !report.errors.is_empty() {
        return Err(format!(
            "index wrote nothing: {} errors (see warnings)",
            report.errors.len()
        ));
    }
    Ok(())
}

fn command_list(cli: &Cli) -> Result<(), String> {
    let category = optional_parse(cli, "--category", Category::parse)?;
    let sex = optional_parse(cli, "--sex", Sex::parse)?;
    let family = cli.value("--family").map(str::to_lowercase);
    let source = cli.value("--source");
    let tags: Vec<String> = cli
        .values("--tag")
        .iter()
        .map(|tag| tag.to_lowercase())
        .collect();
    let name = cli.value("--name").map(str::to_lowercase);
    let limit = parse_count(cli, "--limit")?;
    let offset = parse_count(cli, "--offset")?.unwrap_or(0);
    let store = open_read(cli)?;
    let rows = store
        .iter_kind::<Assets>()
        .map_err(|error| error.to_string())?;
    let mut assets: Vec<Asset> = rows
        .into_iter()
        .map(|(_, asset)| asset)
        .filter(|asset| category.is_none_or(|category| asset.category == category))
        .filter(|asset| match sex {
            None => true,
            Some(Sex::Unknown) => asset.sex.is_none(),
            Some(sex) => asset.sex == Some(sex),
        })
        .filter(|asset| {
            family
                .as_deref()
                .is_none_or(|family| asset.family.as_deref() == Some(family))
        })
        .filter(|asset| source.is_none_or(|source| asset.source == source))
        .filter(|asset| tags.iter().all(|tag| asset.tags.contains(tag)))
        .filter(|asset| {
            name.as_deref().is_none_or(|name| {
                asset.name.to_lowercase().contains(name) || asset.path.to_lowercase().contains(name)
            })
        })
        .collect();
    assets.sort_by(|left, right| {
        (left.category, &left.name, &left.id).cmp(&(right.category, &right.name, &right.id))
    });
    let matched = assets.len();
    let assets: Vec<Asset> = assets
        .into_iter()
        .skip(offset)
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    if cli.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&assets).map_err(|error| error.to_string())?
        );
    } else {
        for asset in &assets {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                asset.id,
                asset.category,
                asset.sex.map(Sex::as_str).unwrap_or("-"),
                escape_field(&asset.name),
                escape_field(&asset.path)
            );
        }
        if offset > 0 || limit.is_some() {
            println!("{} of {matched} assets", assets.len());
        } else {
            println!("{matched} assets");
        }
    }
    Ok(())
}

fn command_get(cli: &Cli) -> Result<(), String> {
    let requested = cli
        .positionals
        .first()
        .ok_or_else(|| "get requires an asset id".to_string())?;
    let store = open_read(cli)?;
    let asset = find_asset(&store, requested)?;
    if cli.has("--json") {
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
        println!("status:   {}", asset.status.as_str());
        if let Some(archive) = &asset.archive {
            println!("archive:  {archive}");
        }
        if let Some(entry) = &asset.entry {
            println!("entry:    {entry}");
        }
    }
    Ok(())
}

fn command_delete(cli: &Cli) -> Result<(), String> {
    let requested = cli
        .positionals
        .first()
        .ok_or_else(|| "delete requires an asset id".to_string())?;
    let store = open_write(cli)?;
    let asset = find_asset(&store, requested)?;
    let mut removed = 0usize;
    if store
        .delete::<Assets>(&asset.id)
        .map_err(|error| error.to_string())?
    {
        removed += 1;
    }
    if asset.category == Category::Archive {
        let members: Vec<String> = store
            .iter_kind::<Assets>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|(_, member)| member.archive.as_deref() == Some(asset.path.as_str()))
            .map(|(id, _)| id)
            .collect();
        for id in members {
            if store
                .delete::<Assets>(&id)
                .map_err(|error| error.to_string())?
            {
                removed += 1;
            }
        }
    }
    println!("deleted {removed} record(s) for {}", asset.id);
    Ok(())
}

fn command_stats(cli: &Cli) -> Result<(), String> {
    let path = database_path(cli.db.as_deref());
    let store = open_read(cli)?;
    let rows = store
        .iter_kind::<Assets>()
        .map_err(|error| error.to_string())?;
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (_, asset) in &rows {
        *counts.entry(asset.category.as_str()).or_default() += 1;
    }
    let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    if cli.has("--json") {
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

fn command_export(cli: &Cli) -> Result<(), String> {
    let out = cli
        .positionals
        .first()
        .ok_or_else(|| "export requires an output path".to_string())?;
    let store = open_read(cli)?;
    let count = store
        .export_jsonl(out)
        .map_err(|error| format!("{out}: {error}"))?;
    println!("exported {count} records to {out}");
    Ok(())
}

fn open_write(cli: &Cli) -> Result<Store, String> {
    let path = database_path(cli.db.as_deref());
    Store::open(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn open_read(cli: &Cli) -> Result<Store, String> {
    let path = database_path(cli.db.as_deref());
    if !path.is_file() {
        return Err(format!(
            "{}: database not indexed; run `boethos index --root DIR`",
            path.display()
        ));
    }
    Store::open_read_only(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn database_path(db: Option<&str>) -> PathBuf {
    if let Some(path) = db.filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var("BOETHOS_DB").ok().filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let state = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_string())).join(".local/state")
        });
    state.join("boethos/assets.redb")
}

fn find_asset(store: &Store, requested: &str) -> Result<Asset, String> {
    let id = requested.to_lowercase();
    if let Some(asset) = store
        .get::<Assets>(&id)
        .map_err(|error| error.to_string())?
    {
        return Ok(asset);
    }
    if id.len() >= 8 && id.len() < 64 && id.chars().all(|character| character.is_ascii_hexdigit()) {
        let matches: Vec<Asset> = store
            .iter_kind::<Assets>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|(candidate, _)| candidate.starts_with(&id))
            .map(|(_, asset)| asset)
            .collect();
        match matches.len() {
            1 => return Ok(matches.into_iter().next().expect("one match")),
            0 => {}
            count => {
                return Err(format!(
                    "asset id {requested:?} is ambiguous: {count} records"
                ));
            }
        }
    }
    Err(format!("asset {requested} not found"))
}

fn parse_count(cli: &Cli, name: &str) -> Result<Option<usize>, String> {
    cli.value(name)
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid {name} {value:?}"))
        })
        .transpose()
}

fn optional_parse<T>(
    cli: &Cli,
    name: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, String> {
    match cli.value(name) {
        None => Ok(None),
        Some(value) => parse(value)
            .map(Some)
            .ok_or_else(|| format!("invalid {name} {value:?}")),
    }
}

fn escape_field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{{{:x}}}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}
