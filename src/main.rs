use std::cmp::Ordering;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;

use serde_json::{Map, Value};

const USAGE: &str = r#"Usage:
  search [OPTIONS] [<title-keyword> ...]
  search import [OPTIONS]
  search remove [OPTIONS]

Search accepted paper titles under the PaperJson data directory.

Positional arguments:
  <title-keyword>                    Optional title include keywords. Every keyword must match.
                                     Positional keywords are equivalent to --keyword.

Options:
  -k, --keyword <keyword>            Title include keyword. Repeatable, supports comma-separated values.
  -x, --exclude <keyword>            Title exclude keyword. Repeatable, supports comma-separated values.
      --exclude-keyword <keyword>    Alias of --exclude.
  -l, --level <level>                Conference level include filter. Repeatable, supports comma-separated values.
      --exclude-level <level>        Conference level exclude filter. Repeatable, supports comma-separated values.
  -n, --conference <name>            Conference name include filter. Repeatable, supports comma-separated values.
      --exclude-conference <name>    Conference name exclude filter. Repeatable, supports comma-separated values.
  -y, --year <year>                  Conference year include filter. Exact match, repeatable, supports comma-separated values.
      --exclude-year <year>          Conference year exclude filter. Exact match, repeatable, supports comma-separated values.
  -s, --sort <field>:<order>         Sort rule, repeatable. Fields: level, conference, year, title.
                                     Orders: asc, desc.
  -c, --columns <list>               Comma-separated columns to display.
                                     Available: level, conference, year, title.
                                     Output order is always: level, conference, year, title.
      --data-dir <path>              Override PaperJson data directory path.
      --paper-dir <path>             Alias of --data-dir.
      --path <path>                  Search one specific conference JSON file.
  -h, --help                         Show this help message.

Import options:
      --path <path>                  Import into one specific conference JSON file.
      --input <path>                 JSON file containing one paper, a list, or a conference document.
      --json <json>                  Inline JSON containing one paper, a list, or a conference document.
      --force                       Import even if the target JSON already has the same title.
  -l, --level <level>                Optional level override when --path cannot infer it.
  -n, --conference <name>            Optional conference override when --path cannot infer it.
      --no-backup                    Do not create a .bak before overwriting an existing JSON file.

Remove options:
      --path <path>                  Remove from one specific conference JSON file.
      --data-dir <path>              PaperJson root used with --level and --conference.
  -l, --level <level>                Conference level for the record.
  -n, --conference <name>            Conference name for the record.
  -y, --year <year>                  Optional year constraint for the record.
      --title <title>                Exact paper title to remove.
      --no-backup                    Do not create a .bak before overwriting an existing JSON file.

Examples:
  search diffusion model
  search --keyword diffusion --keyword model
  search --level A --conference AAAI --year 2024
  search --level A,B --conference AAAI,ICML --year 2024,2025 diffusion
  search --exclude-level B --exclude-year 2024
  search --conference NeurIPS --exclude survey --exclude-year 2023 --sort year:desc --sort title:asc
  search --level A --columns conference,year,title
  search --path PaperJson/A/ICML.json diffusion
  search import --path PaperJson/A/ICML.json --input paper.json --force
  search remove --path PaperJson/A/ICML.json --title "Paper Title"
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Field {
    Level,
    Conference,
    Year,
    Title,
}

impl Field {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "level" => Ok(Self::Level),
            "conference" | "conf" | "name" => Ok(Self::Conference),
            "year" => Ok(Self::Year),
            "title" | "paper" => Ok(Self::Title),
            other => Err(format!("unsupported field: {other}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Asc,
    Desc,
}

impl Direction {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "asc" => Ok(Self::Asc),
            "desc" => Ok(Self::Desc),
            other => Err(format!("unsupported order: {other}")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SortSpec {
    field: Field,
    direction: Direction,
}

#[derive(Debug, Eq, PartialEq)]
struct Config {
    title_include_keywords: Vec<String>,
    title_exclude_keywords: Vec<String>,
    level_include_filters: Vec<String>,
    level_exclude_filters: Vec<String>,
    conference_include_filters: Vec<String>,
    conference_exclude_filters: Vec<String>,
    year_include_filters: Vec<String>,
    year_exclude_filters: Vec<String>,
    sort_specs: Vec<SortSpec>,
    display_fields: Vec<Field>,
    paper_dir: Option<PathBuf>,
    paper_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Record {
    level: String,
    conference: String,
    year: String,
    title: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TargetConfig {
    data_dir: Option<PathBuf>,
    path: Option<PathBuf>,
    level: Option<String>,
    conference: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct ImportConfig {
    target: TargetConfig,
    source: ImportSource,
    force: bool,
    backup: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum ImportSource {
    File(PathBuf),
    Inline(String),
}

#[derive(Clone, Debug, PartialEq)]
struct PaperImportRecord {
    year: String,
    title: String,
    entry: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
struct RemoveConfig {
    target: TargetConfig,
    year: Option<String>,
    title: String,
    backup: bool,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(AppError::Help) => {
            print!("{USAGE}");
        }
        Err(AppError::Message(message)) => {
            eprintln!("Error: {message}\n");
            eprintln!("{USAGE}");
            process::exit(1);
        }
        Err(AppError::Io(error)) => {
            eprintln!("Error: {error}");
            process::exit(1);
        }
    }
}

fn run() -> Result<(), AppError> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("add") {
        return Err(AppError::Message(
            "search add has been removed; use search import --path <file.json> --input <paper.json>"
                .to_string(),
        ));
    }
    if args.first().map(String::as_str) == Some("import") {
        return run_import(parse_import_args(args.into_iter().skip(1))?);
    }
    if args.first().map(String::as_str) == Some("remove") {
        return run_remove(parse_remove_args(args.into_iter().skip(1))?);
    }

    let config = parse_args(args)?;
    let mut records = if let Some(paper_path) = config.paper_path.as_deref() {
        load_filtered_records_from_path(paper_path, &config)?
    } else {
        let paper_dir = resolve_paper_dir(config.paper_dir.as_deref())?;
        load_filtered_records(&paper_dir, &config)?
    };

    records.retain(|record| record_matches(record, &config));

    if !config.sort_specs.is_empty() {
        records.sort_by(|left, right| compare_records(left, right, &config.sort_specs));
    }

    for record in records {
        println!("{}", format_record(&record, &config.display_fields));
    }

    Ok(())
}

#[derive(Debug)]
enum AppError {
    Help,
    Message(String),
    Io(io::Error),
}

impl From<io::Error> for AppError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

fn parse_args<I>(args: I) -> Result<Config, AppError>
where
    I: IntoIterator<Item = String>,
{
    let mut title_include_keywords = Vec::new();
    let mut title_exclude_keywords = Vec::new();
    let mut level_include_filters = Vec::new();
    let mut level_exclude_filters = Vec::new();
    let mut conference_include_filters = Vec::new();
    let mut conference_exclude_filters = Vec::new();
    let mut year_include_filters = Vec::new();
    let mut year_exclude_filters = Vec::new();
    let mut sort_specs = Vec::new();
    let mut display_fields = canonical_fields();
    let mut paper_dir = None;
    let mut paper_path = None;

    let args: Vec<String> = args.into_iter().collect();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "-h" | "--help" => return Err(AppError::Help),
            "-k" | "--keyword" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --keyword".to_string()))?;
                append_normalized_values(&mut title_include_keywords, value)?;
            }
            "-x" | "--exclude" | "--exclude-keyword" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --exclude".to_string()))?;
                append_normalized_values(&mut title_exclude_keywords, value)?;
            }
            "-l" | "--level" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --level".to_string()))?;
                append_normalized_values(&mut level_include_filters, value)?;
            }
            "--exclude-level" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    AppError::Message("missing value for --exclude-level".to_string())
                })?;
                append_normalized_values(&mut level_exclude_filters, value)?;
            }
            "-n" | "--conference" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    AppError::Message("missing value for --conference".to_string())
                })?;
                append_normalized_values(&mut conference_include_filters, value)?;
            }
            "--exclude-conference" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    AppError::Message("missing value for --exclude-conference".to_string())
                })?;
                append_normalized_values(&mut conference_exclude_filters, value)?;
            }
            "-y" | "--year" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --year".to_string()))?;
                append_normalized_values(&mut year_include_filters, value)?;
            }
            "--exclude-year" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    AppError::Message("missing value for --exclude-year".to_string())
                })?;
                append_normalized_values(&mut year_exclude_filters, value)?;
            }
            "-s" | "--sort" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --sort".to_string()))?;
                sort_specs.push(parse_sort_spec(value)?);
            }
            "-c" | "--columns" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --columns".to_string()))?;
                display_fields = parse_columns(value)?;
            }
            "--data-dir" | "--paper-dir" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --data-dir".to_string()))?;
                paper_dir = Some(PathBuf::from(value));
            }
            "--path" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| AppError::Message("missing value for --path".to_string()))?;
                paper_path = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--keyword=") => {
                let value = arg.trim_start_matches("--keyword=");
                append_normalized_values(&mut title_include_keywords, value)?;
            }
            _ if arg.starts_with("--exclude=") || arg.starts_with("--exclude-keyword=") => {
                let value = arg
                    .split_once('=')
                    .map(|(_, value)| value)
                    .unwrap_or_default();
                append_normalized_values(&mut title_exclude_keywords, value)?;
            }
            _ if arg.starts_with("--level=") => {
                let value = arg.trim_start_matches("--level=");
                append_normalized_values(&mut level_include_filters, value)?;
            }
            _ if arg.starts_with("--exclude-level=") => {
                let value = arg.trim_start_matches("--exclude-level=");
                append_normalized_values(&mut level_exclude_filters, value)?;
            }
            _ if arg.starts_with("--conference=") => {
                let value = arg.trim_start_matches("--conference=");
                append_normalized_values(&mut conference_include_filters, value)?;
            }
            _ if arg.starts_with("--exclude-conference=") => {
                let value = arg.trim_start_matches("--exclude-conference=");
                append_normalized_values(&mut conference_exclude_filters, value)?;
            }
            _ if arg.starts_with("--year=") => {
                let value = arg.trim_start_matches("--year=");
                append_normalized_values(&mut year_include_filters, value)?;
            }
            _ if arg.starts_with("--exclude-year=") => {
                let value = arg.trim_start_matches("--exclude-year=");
                append_normalized_values(&mut year_exclude_filters, value)?;
            }
            _ if arg.starts_with("--sort=") => {
                let value = arg.trim_start_matches("--sort=");
                sort_specs.push(parse_sort_spec(value)?);
            }
            _ if arg.starts_with("--columns=") => {
                let value = arg.trim_start_matches("--columns=");
                display_fields = parse_columns(value)?;
            }
            _ if arg.starts_with("--paper-dir=") => {
                let value = arg.trim_start_matches("--paper-dir=");
                paper_dir = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--data-dir=") => {
                let value = arg.trim_start_matches("--data-dir=");
                paper_dir = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--path=") => {
                let value = arg.trim_start_matches("--path=");
                paper_path = Some(PathBuf::from(value));
            }
            _ if arg.starts_with('-') => {
                return Err(AppError::Message(format!("unsupported option: {arg}")));
            }
            _ => title_include_keywords.push(normalize_value(arg)?),
        }

        index += 1;
    }

    if !has_any_filter(&[
        &title_include_keywords,
        &title_exclude_keywords,
        &level_include_filters,
        &level_exclude_filters,
        &conference_include_filters,
        &conference_exclude_filters,
        &year_include_filters,
        &year_exclude_filters,
    ]) {
        return Err(AppError::Message(
            "at least one filter is required: keyword/level/conference/year include or exclude"
                .to_string(),
        ));
    }

    if paper_dir.is_some() && paper_path.is_some() {
        return Err(AppError::Message(
            "--path cannot be combined with --data-dir/--paper-dir".to_string(),
        ));
    }

    Ok(Config {
        title_include_keywords,
        title_exclude_keywords,
        level_include_filters,
        level_exclude_filters,
        conference_include_filters,
        conference_exclude_filters,
        year_include_filters,
        year_exclude_filters,
        sort_specs,
        display_fields,
        paper_dir,
        paper_path,
    })
}

fn parse_import_args<I>(args: I) -> Result<ImportConfig, AppError>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let mut target = TargetConfig::default();
    let mut source = None;
    let mut force = false;
    let mut backup = true;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "-h" | "--help" => return Err(AppError::Help),
            "--data-dir" | "--paper-dir" => {
                index += 1;
                target.data_dir = Some(PathBuf::from(next_arg(&args, index, "--data-dir")?));
            }
            "--path" => {
                index += 1;
                target.path = Some(PathBuf::from(next_arg(&args, index, "--path")?));
            }
            "--input" | "--file" => {
                index += 1;
                set_import_source(
                    &mut source,
                    ImportSource::File(PathBuf::from(next_arg(&args, index, "--input")?)),
                )?;
            }
            "--json" => {
                index += 1;
                set_import_source(
                    &mut source,
                    ImportSource::Inline(next_arg(&args, index, "--json")?.to_string()),
                )?;
            }
            "--force" => force = true,
            "-l" | "--level" => {
                index += 1;
                target.level = Some(required_raw_value(
                    next_arg(&args, index, "--level")?,
                    "level",
                )?);
            }
            "-n" | "--conference" => {
                index += 1;
                target.conference = Some(required_raw_value(
                    next_arg(&args, index, "--conference")?,
                    "conference",
                )?);
            }
            "--no-backup" => backup = false,
            _ if arg.starts_with("--data-dir=") => {
                target.data_dir = Some(PathBuf::from(arg.trim_start_matches("--data-dir=")));
            }
            _ if arg.starts_with("--paper-dir=") => {
                target.data_dir = Some(PathBuf::from(arg.trim_start_matches("--paper-dir=")));
            }
            _ if arg.starts_with("--path=") => {
                target.path = Some(PathBuf::from(arg.trim_start_matches("--path=")));
            }
            _ if arg.starts_with("--input=") => {
                set_import_source(
                    &mut source,
                    ImportSource::File(PathBuf::from(arg.trim_start_matches("--input="))),
                )?;
            }
            _ if arg.starts_with("--file=") => {
                set_import_source(
                    &mut source,
                    ImportSource::File(PathBuf::from(arg.trim_start_matches("--file="))),
                )?;
            }
            _ if arg.starts_with("--json=") => {
                set_import_source(
                    &mut source,
                    ImportSource::Inline(arg.trim_start_matches("--json=").to_string()),
                )?;
            }
            _ if arg.starts_with("--level=") => {
                target.level = Some(required_raw_value(
                    arg.trim_start_matches("--level="),
                    "level",
                )?);
            }
            _ if arg.starts_with("--conference=") => {
                target.conference = Some(required_raw_value(
                    arg.trim_start_matches("--conference="),
                    "conference",
                )?);
            }
            _ if arg.starts_with('-') => {
                return Err(AppError::Message(format!(
                    "unsupported import option: {arg}"
                )));
            }
            _ => {
                return Err(AppError::Message(format!(
                    "unexpected import argument: {arg}"
                )));
            }
        }

        index += 1;
    }

    Ok(ImportConfig {
        target,
        source: source.ok_or_else(|| {
            AppError::Message("missing --input <path> or --json <json>".to_string())
        })?,
        force,
        backup,
    })
}

fn set_import_source(
    target: &mut Option<ImportSource>,
    value: ImportSource,
) -> Result<(), AppError> {
    if target.is_some() {
        return Err(AppError::Message(
            "--input/--file and --json are mutually exclusive".to_string(),
        ));
    }
    *target = Some(value);
    Ok(())
}

fn parse_remove_args<I>(args: I) -> Result<RemoveConfig, AppError>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let mut target = TargetConfig::default();
    let mut year = None;
    let mut title = None;
    let mut backup = true;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "-h" | "--help" => return Err(AppError::Help),
            "--data-dir" | "--paper-dir" => {
                index += 1;
                target.data_dir = Some(PathBuf::from(next_arg(&args, index, "--data-dir")?));
            }
            "--path" => {
                index += 1;
                target.path = Some(PathBuf::from(next_arg(&args, index, "--path")?));
            }
            "-l" | "--level" => {
                index += 1;
                target.level = Some(required_raw_value(
                    next_arg(&args, index, "--level")?,
                    "level",
                )?);
            }
            "-n" | "--conference" => {
                index += 1;
                target.conference = Some(required_raw_value(
                    next_arg(&args, index, "--conference")?,
                    "conference",
                )?);
            }
            "-y" | "--year" => {
                index += 1;
                year = Some(required_year(next_arg(&args, index, "--year")?)?);
            }
            "--title" => {
                index += 1;
                title = Some(required_raw_value(
                    next_arg(&args, index, "--title")?,
                    "title",
                )?);
            }
            "--no-backup" => backup = false,
            _ if arg.starts_with("--data-dir=") => {
                target.data_dir = Some(PathBuf::from(arg.trim_start_matches("--data-dir=")));
            }
            _ if arg.starts_with("--paper-dir=") => {
                target.data_dir = Some(PathBuf::from(arg.trim_start_matches("--paper-dir=")));
            }
            _ if arg.starts_with("--path=") => {
                target.path = Some(PathBuf::from(arg.trim_start_matches("--path=")));
            }
            _ if arg.starts_with("--level=") => {
                target.level = Some(required_raw_value(
                    arg.trim_start_matches("--level="),
                    "level",
                )?);
            }
            _ if arg.starts_with("--conference=") => {
                target.conference = Some(required_raw_value(
                    arg.trim_start_matches("--conference="),
                    "conference",
                )?);
            }
            _ if arg.starts_with("--year=") => {
                year = Some(required_year(arg.trim_start_matches("--year="))?);
            }
            _ if arg.starts_with("--title=") => {
                title = Some(required_raw_value(
                    arg.trim_start_matches("--title="),
                    "title",
                )?);
            }
            _ if arg.starts_with('-') => {
                return Err(AppError::Message(format!(
                    "unsupported remove option: {arg}"
                )));
            }
            _ => {
                return Err(AppError::Message(format!(
                    "unexpected remove argument: {arg}"
                )));
            }
        }

        index += 1;
    }

    Ok(RemoveConfig {
        target,
        year,
        title: title.ok_or_else(|| AppError::Message("missing --title".to_string()))?,
        backup,
    })
}

fn next_arg<'a>(args: &'a [String], index: usize, option: &str) -> Result<&'a str, AppError> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| AppError::Message(format!("missing value for {option}")))
}

fn required_raw_value(value: &str, name: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(AppError::Message(format!("{name} cannot be empty")))
    } else {
        Ok(trimmed.to_string())
    }
}

fn required_year(value: &str) -> Result<String, AppError> {
    let year = required_raw_value(value, "year")?;
    if is_year_key(&year) {
        Ok(year)
    } else {
        Err(AppError::Message(format!("invalid year: {year}")))
    }
}

fn has_any_filter(filter_groups: &[&[String]]) -> bool {
    filter_groups.iter().any(|group| !group.is_empty())
}

fn append_normalized_values(target: &mut Vec<String>, raw: &str) -> Result<(), AppError> {
    let mut appended = false;
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = normalize_value(trimmed)?;
        if !target.contains(&normalized) {
            target.push(normalized);
        }
        appended = true;
    }

    if appended {
        Ok(())
    } else {
        Err(AppError::Message(
            "filter value cannot be empty".to_string(),
        ))
    }
}

fn normalize_value(value: &str) -> Result<String, AppError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        Err(AppError::Message(
            "filter value cannot be empty".to_string(),
        ))
    } else {
        Ok(normalized)
    }
}

fn parse_sort_spec(value: &str) -> Result<SortSpec, AppError> {
    let (field_raw, order_raw) = value
        .split_once(':')
        .ok_or_else(|| AppError::Message(format!("invalid sort spec: {value}")))?;

    let field = Field::parse(field_raw).map_err(AppError::Message)?;
    let direction = Direction::parse(order_raw).map_err(AppError::Message)?;

    Ok(SortSpec { field, direction })
}

fn parse_columns(value: &str) -> Result<Vec<Field>, AppError> {
    let mut selected = Vec::new();
    for item in value.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let field = Field::parse(trimmed).map_err(AppError::Message)?;
        if !selected.contains(&field) {
            selected.push(field);
        }
    }

    if selected.is_empty() {
        return Err(AppError::Message(
            "at least one column must be selected".to_string(),
        ));
    }

    Ok(canonical_fields()
        .into_iter()
        .filter(|field| selected.contains(field))
        .collect())
}

fn canonical_fields() -> Vec<Field> {
    vec![Field::Level, Field::Conference, Field::Year, Field::Title]
}

fn resolve_paper_dir(override_path: Option<&Path>) -> Result<PathBuf, AppError> {
    if let Some(path) = override_path {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        return Err(AppError::Message(format!(
            "data directory does not exist: {}",
            path.display()
        )));
    }

    let exe_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let current_dir = env::current_dir().ok();

    let mut candidates = Vec::new();

    if let Some(dir) = &exe_dir {
        candidates.push(dir.join("PaperJson"));
    }
    if let Some(dir) = &current_dir {
        candidates.push(dir.join("PaperJson"));
    }
    if let Some(dir) = &exe_dir {
        candidates.push(dir.join("..").join("PaperJson"));
    }
    if let Some(dir) = &current_dir {
        candidates.push(dir.join("..").join("PaperJson"));
    }

    for candidate in candidates {
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }

    Err(AppError::Message(
        "unable to locate PaperJson directory; use --data-dir to specify it".to_string(),
    ))
}

fn run_import(config: ImportConfig) -> Result<(), AppError> {
    let records = read_import_records(&config.source)?;
    let (json_path, level, conference) = resolve_mutation_target(&config.target)?;
    let mut document = load_or_create_conference_document(&json_path, &level, &conference)?;

    normalize_conference_document(&mut document, &level, &conference)?;
    let stats = apply_import_records(&mut document, &records, config.force)?;

    if stats.changed() {
        if config.backup {
            backup_json_file(&json_path)?;
        }
        write_json_file(&json_path, &document)?;
    }

    println!(
        "Imported {} inserted, {} replaced into {}",
        stats.inserted,
        stats.replaced,
        json_path.display()
    );
    Ok(())
}

fn run_remove(config: RemoveConfig) -> Result<(), AppError> {
    let (json_path, level, conference) = resolve_mutation_target(&config.target)?;
    if !json_path.exists() {
        return Err(AppError::Message(format!(
            "conference JSON does not exist: {}",
            json_path.display()
        )));
    }

    let mut document = load_or_create_conference_document(&json_path, &level, &conference)?;
    normalize_conference_document(&mut document, &level, &conference)?;
    remove_entry_from_document(&mut document, &config)?;

    if config.backup {
        backup_json_file(&json_path)?;
    }

    write_json_file(&json_path, &document)?;
    println!("Removed {}", config.title);
    Ok(())
}

fn resolve_mutation_target(config: &TargetConfig) -> Result<(PathBuf, String, String), AppError> {
    if let Some(path) = &config.path {
        let level = match &config.level {
            Some(level) => level.clone(),
            None => path
                .parent()
                .and_then(Path::file_name)
                .and_then(|value| value.to_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    AppError::Message("missing --level and cannot infer it from --path".to_string())
                })?,
        };
        let conference = match &config.conference {
            Some(conference) => conference.clone(),
            None => file_stem(path)?,
        };
        return Ok((path.clone(), level, conference));
    }

    let level = config
        .level
        .clone()
        .ok_or_else(|| AppError::Message("missing --level".to_string()))?;
    let conference = config
        .conference
        .clone()
        .ok_or_else(|| AppError::Message("missing --conference".to_string()))?;
    let data_dir = match &config.data_dir {
        Some(path) => path.clone(),
        None => resolve_paper_dir(None)?,
    };

    Ok((
        data_dir.join(&level).join(format!("{conference}.json")),
        level,
        conference,
    ))
}

fn load_or_create_conference_document(
    json_path: &Path,
    level: &str,
    conference: &str,
) -> Result<Value, AppError> {
    if !json_path.exists() {
        return Ok(base_conference_document(level, conference));
    }

    let content = fs::read_to_string(json_path)?;
    let value: Value = serde_json::from_str(strip_utf8_bom(&content)).map_err(|error| {
        AppError::Message(format!("invalid JSON in {}: {error}", json_path.display()))
    })?;
    Ok(value)
}

fn base_conference_document(level: &str, conference: &str) -> Value {
    let mut root = Map::new();
    root.insert("schema_version".to_string(), Value::Number(1.into()));
    root.insert("level".to_string(), Value::String(level.to_string()));
    root.insert(
        "conference".to_string(),
        Value::String(conference.to_string()),
    );
    root.insert("papers".to_string(), Value::Object(Map::new()));
    Value::Object(root)
}

fn normalize_conference_document(
    document: &mut Value,
    level: &str,
    conference: &str,
) -> Result<(), AppError> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| AppError::Message("conference JSON root must be an object".to_string()))?;

    root.insert("schema_version".to_string(), Value::Number(1.into()));
    root.insert("level".to_string(), Value::String(level.to_string()));
    root.insert(
        "conference".to_string(),
        Value::String(conference.to_string()),
    );
    root.entry("papers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));

    let top_level_years: Vec<String> = root
        .keys()
        .filter(|key| is_year_key(key))
        .cloned()
        .collect();
    let mut migrated_years = Vec::new();
    for year in top_level_years {
        if let Some(year_value) = root.remove(&year) {
            migrated_years.push((year, year_value));
        }
    }

    let papers = root
        .get_mut("papers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| AppError::Message("papers must be an object".to_string()))?;

    for (year, year_value) in migrated_years {
        merge_year_value(papers, year, year_value)?;
    }

    for (year, titles) in papers.iter_mut() {
        if !is_year_key(year) {
            return Err(AppError::Message(format!("invalid year key: {year}")));
        }
        let titles = titles
            .as_object_mut()
            .ok_or_else(|| AppError::Message(format!("papers[{year:?}] must be an object")))?;
        for entry in titles.values_mut() {
            normalize_entry(entry)?;
        }
    }

    Ok(())
}

fn merge_year_value(
    papers: &mut Map<String, Value>,
    year: String,
    year_value: Value,
) -> Result<(), AppError> {
    let incoming_titles = year_value
        .as_object()
        .ok_or_else(|| AppError::Message(format!("top-level year {year} must be an object")))?;
    let target_year = papers
        .entry(year.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    let target_titles = target_year
        .as_object_mut()
        .ok_or_else(|| AppError::Message(format!("papers[{year:?}] must be an object")))?;

    for (title, entry) in incoming_titles {
        target_titles
            .entry(title.clone())
            .or_insert_with(|| entry.clone());
    }

    Ok(())
}

fn normalize_entry(entry: &mut Value) -> Result<(), AppError> {
    if !entry.is_object() {
        let old_value = std::mem::replace(entry, Value::Object(Map::new()));
        let object = entry
            .as_object_mut()
            .ok_or_else(|| AppError::Message("failed to normalize non-object entry".to_string()))?;
        object.insert("value".to_string(), old_value);
    }

    let object = entry
        .as_object_mut()
        .ok_or_else(|| AppError::Message("paper entry must be an object".to_string()))?;

    if let Some(auth) = object.remove("auth") {
        object.entry("author".to_string()).or_insert(auth);
    }
    object.remove("source");
    object.remove("sources");

    object
        .entry("author".to_string())
        .or_insert_with(|| Value::String(String::new()));
    object
        .entry("bib".to_string())
        .or_insert_with(|| Value::String(String::new()));
    object
        .entry("url".to_string())
        .or_insert_with(|| Value::String(String::new()));

    Ok(())
}

#[derive(Default)]
struct ImportStats {
    inserted: usize,
    replaced: usize,
}

impl ImportStats {
    fn changed(&self) -> bool {
        self.inserted + self.replaced > 0
    }
}

fn read_import_records(source: &ImportSource) -> Result<Vec<PaperImportRecord>, AppError> {
    let content = match source {
        ImportSource::File(path) => fs::read_to_string(path).map_err(|error| {
            AppError::Message(format!("failed to read {}: {error}", path.display()))
        })?,
        ImportSource::Inline(value) => value.clone(),
    };
    let value: Value = serde_json::from_str(strip_utf8_bom(&content))
        .map_err(|error| AppError::Message(format!("invalid import JSON: {error}")))?;
    let records = import_records_from_value(value)?;
    if records.is_empty() {
        return Err(AppError::Message(
            "import JSON does not contain any paper records".to_string(),
        ));
    }
    validate_unique_import_titles(&records)?;
    Ok(records)
}

fn import_records_from_value(value: Value) -> Result<Vec<PaperImportRecord>, AppError> {
    match value {
        Value::Array(items) => parse_import_record_array(items, "root array"),
        Value::Object(mut object) => {
            if let Some(papers) = object.remove("papers") {
                return match papers {
                    Value::Array(items) => parse_import_record_array(items, "papers"),
                    Value::Object(papers) => parse_conference_papers_import(papers),
                    _ => Err(AppError::Message(
                        "papers must be an array or an object".to_string(),
                    )),
                };
            }

            Ok(vec![parse_import_record_object(object, "root object")?])
        }
        _ => Err(AppError::Message(
            "import JSON root must be an object or an array".to_string(),
        )),
    }
}

fn parse_import_record_array(
    items: Vec<Value>,
    context: &str,
) -> Result<Vec<PaperImportRecord>, AppError> {
    let mut records = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let Value::Object(object) = item else {
            return Err(AppError::Message(format!(
                "{context}[{index}] must be an object"
            )));
        };
        records.push(parse_import_record_object(
            object,
            &format!("{context}[{index}]"),
        )?);
    }
    Ok(records)
}

fn parse_import_record_object(
    mut object: Map<String, Value>,
    context: &str,
) -> Result<PaperImportRecord, AppError> {
    let year = parse_import_year(
        object
            .remove("year")
            .ok_or_else(|| AppError::Message(format!("{context} is missing year")))?,
        context,
    )?;
    let title = parse_import_title(
        object
            .remove("title")
            .ok_or_else(|| AppError::Message(format!("{context} is missing title")))?,
        context,
    )?;
    validate_import_entry_fields(&object, context)?;

    Ok(PaperImportRecord {
        year,
        title,
        entry: object,
    })
}

fn parse_conference_papers_import(
    papers: Map<String, Value>,
) -> Result<Vec<PaperImportRecord>, AppError> {
    let mut records = Vec::new();

    for (year, titles) in papers {
        let year = required_year(&year)?;
        let Value::Object(titles) = titles else {
            return Err(AppError::Message(format!(
                "papers[{year:?}] must be an object"
            )));
        };
        for (title, entry) in titles {
            let title = required_raw_value(&title, "title")?;
            let Value::Object(entry) = entry else {
                return Err(AppError::Message(format!(
                    "entry for {title:?} must be an object"
                )));
            };
            validate_import_entry_fields(&entry, &format!("entry for {title:?}"))?;
            records.push(PaperImportRecord {
                year: year.clone(),
                title,
                entry,
            });
        }
    }

    Ok(records)
}

fn parse_import_year(value: Value, context: &str) -> Result<String, AppError> {
    match value {
        Value::String(year) => required_year(&year),
        Value::Number(number) => required_year(&number.to_string()),
        _ => Err(AppError::Message(format!(
            "{context} year must be a string or integer"
        ))),
    }
}

fn parse_import_title(value: Value, context: &str) -> Result<String, AppError> {
    match value {
        Value::String(title) => required_raw_value(&title, "title"),
        _ => Err(AppError::Message(format!(
            "{context} title must be a string"
        ))),
    }
}

fn validate_import_entry_fields(entry: &Map<String, Value>, context: &str) -> Result<(), AppError> {
    for (key, value) in entry {
        if key.trim().is_empty() {
            return Err(AppError::Message(format!(
                "{context} contains an empty field key"
            )));
        }
        if matches!(key.as_str(), "auth" | "source" | "sources") {
            return Err(AppError::Message(format!(
                "{context} contains disallowed field {key:?}"
            )));
        }
        if matches!(key.as_str(), "year" | "title") {
            return Err(AppError::Message(format!(
                "{context} cannot store identity field {key:?} inside paper metadata"
            )));
        }
        if matches!(key.as_str(), "author" | "bib" | "url") && !value.is_string() {
            return Err(AppError::Message(format!(
                "{context} field {key:?} must be a string"
            )));
        }
    }
    Ok(())
}

fn validate_unique_import_titles(records: &[PaperImportRecord]) -> Result<(), AppError> {
    let mut seen = HashSet::new();
    for record in records {
        let key = normalize_title_key(&record.title);
        if !seen.insert(key) {
            return Err(AppError::Message(format!(
                "duplicate title in import JSON: {}",
                record.title
            )));
        }
    }
    Ok(())
}

fn apply_import_records(
    document: &mut Value,
    records: &[PaperImportRecord],
    force: bool,
) -> Result<ImportStats, AppError> {
    let papers = document
        .get_mut("papers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| AppError::Message("papers must be an object".to_string()))?;

    if !force {
        for record in records {
            if title_exists(papers, &record.title)? {
                return Err(AppError::Message(format!(
                    "duplicate title already exists in target JSON: {}",
                    record.title
                )));
            }
        }
    }

    let mut stats = ImportStats::default();
    for record in records {
        if let Some(existing_title) = find_title_in_year(papers, &record.year, &record.title)? {
            remove_title_at(papers, &record.year, &existing_title)?;
            stats.replaced += 1;
        } else {
            stats.inserted += 1;
        }
        insert_record_at(papers, &record.year, &record.title, &record.entry)?;
    }

    Ok(stats)
}

fn find_title_in_year(
    papers: &Map<String, Value>,
    year: &str,
    title: &str,
) -> Result<Option<String>, AppError> {
    let Some(year_value) = papers.get(year) else {
        return Ok(None);
    };
    let titles = year_value
        .as_object()
        .ok_or_else(|| AppError::Message(format!("papers[{year:?}] must be an object")))?;
    let needle = normalize_title_key(title);
    Ok(titles
        .keys()
        .find(|existing_title| normalize_title_key(existing_title) == needle)
        .cloned())
}

fn title_exists(papers: &Map<String, Value>, title: &str) -> Result<bool, AppError> {
    Ok(!title_locations(papers, title)?.is_empty())
}

fn find_title_location(
    papers: &Map<String, Value>,
    title: &str,
    preferred_year: Option<&str>,
) -> Result<Option<(String, String)>, AppError> {
    let locations = title_locations(papers, title)?;
    if locations.len() <= 1 {
        return Ok(locations.into_iter().next());
    }

    if let Some(preferred_year) = preferred_year {
        let mut matching_year = locations
            .iter()
            .filter(|(year, _)| year == preferred_year)
            .cloned();
        if let Some(location) = matching_year.next() {
            if matching_year.next().is_none() {
                return Ok(Some(location));
            }
        }
    }

    Err(AppError::Message(format!(
        "target JSON contains multiple entries for title {title:?}; use --year for remove or clean the duplicates before import"
    )))
}

fn title_locations(
    papers: &Map<String, Value>,
    title: &str,
) -> Result<Vec<(String, String)>, AppError> {
    let mut locations = Vec::new();
    let needle = normalize_title_key(title);

    for (year, titles) in papers {
        let titles = titles
            .as_object()
            .ok_or_else(|| AppError::Message(format!("papers[{year:?}] must be an object")))?;
        for title in titles.keys() {
            if normalize_title_key(title) == needle {
                locations.push((year.clone(), title.clone()));
            }
        }
    }

    Ok(locations)
}

fn normalize_title_key(title: &str) -> String {
    title.trim().to_ascii_lowercase()
}

fn insert_record_at(
    papers: &mut Map<String, Value>,
    year: &str,
    title: &str,
    entry: &Map<String, Value>,
) -> Result<(), AppError> {
    let year_value = papers
        .entry(year.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let titles = year_value
        .as_object_mut()
        .ok_or_else(|| AppError::Message(format!("papers[{year:?}] must be an object")))?;
    titles.insert(title.to_string(), Value::Object(defaulted_entry(entry)));
    Ok(())
}

fn remove_entry_from_document(document: &mut Value, config: &RemoveConfig) -> Result<(), AppError> {
    let papers = document
        .get_mut("papers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| AppError::Message("papers must be an object".to_string()))?;

    let (year, title) = if let Some(year) = &config.year {
        (year.clone(), config.title.clone())
    } else {
        find_title_location(papers, &config.title, None)?
            .ok_or_else(|| AppError::Message(format!("title not found: {}", config.title)))?
    };

    remove_title_at(papers, &year, &title)?;

    Ok(())
}

fn remove_title_at(
    papers: &mut Map<String, Value>,
    year: &str,
    title: &str,
) -> Result<(), AppError> {
    let Some(year_value) = papers.get_mut(year) else {
        return Err(AppError::Message(format!("year {year} does not exist")));
    };
    let titles = year_value
        .as_object_mut()
        .ok_or_else(|| AppError::Message(format!("papers[{year:?}] must be an object")))?;

    if titles.remove(title).is_none() {
        return Err(AppError::Message(format!(
            "title not found in {year}: {title}"
        )));
    }
    if titles.is_empty() {
        papers.remove(year);
    }
    Ok(())
}

fn defaulted_entry(entry: &Map<String, Value>) -> Map<String, Value> {
    let mut defaulted = Map::new();
    defaulted.insert("author".to_string(), Value::String(String::new()));
    defaulted.insert("bib".to_string(), Value::String(String::new()));
    defaulted.insert("url".to_string(), Value::String(String::new()));
    for (key, value) in entry {
        defaulted.insert(key.clone(), value.clone());
    }
    defaulted
}

fn backup_json_file(json_path: &Path) -> Result<(), AppError> {
    if !json_path.exists() {
        return Ok(());
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| AppError::Message(format!("system clock error: {error}")))?
        .as_nanos();
    let file_name = json_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Message(format!("invalid file name: {}", json_path.display())))?;
    let mut backup_path = json_path.with_file_name(format!("{file_name}.{timestamp}.bak"));
    let mut counter = 1;
    while backup_path.exists() {
        backup_path = json_path.with_file_name(format!("{file_name}.{timestamp}.{counter}.bak"));
        counter += 1;
    }
    fs::copy(json_path, backup_path)?;
    Ok(())
}

fn write_json_file(json_path: &Path, document: &Value) -> Result<(), AppError> {
    if let Some(parent) = json_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let content = serde_json::to_string_pretty(document).map_err(|error| {
        AppError::Message(format!(
            "failed to serialize {}: {error}",
            json_path.display()
        ))
    })?;
    fs::write(json_path, format!("{content}\n"))?;
    Ok(())
}

#[cfg(test)]
fn load_records(root: &Path) -> Result<Vec<Record>, AppError> {
    Ok(deduplicate_records(load_json_records(root, None)?))
}

fn load_filtered_records(root: &Path, config: &Config) -> Result<Vec<Record>, AppError> {
    Ok(deduplicate_records(load_json_records(root, Some(config))?))
}

fn load_filtered_records_from_path(path: &Path, config: &Config) -> Result<Vec<Record>, AppError> {
    let level = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            AppError::Message(
                "cannot infer level from --path; use a PaperJson/<level>/<conference>.json path"
                    .to_string(),
            )
        })?;
    let conference = file_stem(path)?;

    if !matches_scalar_filter(
        &level,
        &config.level_include_filters,
        &config.level_exclude_filters,
    ) || !matches_scalar_filter(
        &conference,
        &config.conference_include_filters,
        &config.conference_exclude_filters,
    ) {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(strip_utf8_bom(&content)).map_err(|error| {
        AppError::Message(format!("invalid JSON in {}: {error}", path.display()))
    })?;
    let mut records = Vec::new();
    append_records_from_conference_json(&mut records, &level, &conference, &value, Some(config))?;
    Ok(deduplicate_records(records))
}

fn load_json_records(root: &Path, config: Option<&Config>) -> Result<Vec<Record>, AppError> {
    let mut records = Vec::new();

    for level_dir in sorted_dirs(root)? {
        let level = file_name(&level_dir)?;
        if let Some(config) = config {
            if !matches_scalar_filter(
                &level,
                &config.level_include_filters,
                &config.level_exclude_filters,
            ) {
                continue;
            }
        }

        for file_path in sorted_json_files(&level_dir)? {
            let conference = file_stem(&file_path)?;
            if let Some(config) = config {
                if !matches_scalar_filter(
                    &conference,
                    &config.conference_include_filters,
                    &config.conference_exclude_filters,
                ) {
                    continue;
                }
            }

            let content = fs::read_to_string(&file_path)?;
            let value: Value = serde_json::from_str(strip_utf8_bom(&content)).map_err(|error| {
                AppError::Message(format!("invalid JSON in {}: {error}", file_path.display()))
            })?;
            append_records_from_conference_json(&mut records, &level, &conference, &value, config)?;
        }
    }

    Ok(records)
}

fn strip_utf8_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

fn append_records_from_conference_json(
    records: &mut Vec<Record>,
    level: &str,
    conference: &str,
    value: &Value,
    config: Option<&Config>,
) -> Result<(), AppError> {
    let papers = value
        .get("papers")
        .filter(|papers| papers.is_object())
        .unwrap_or(value);

    let years = papers
        .as_object()
        .ok_or_else(|| AppError::Message("conference JSON must be an object".to_string()))?;

    for (year, titles) in years {
        if !is_year_key(year) {
            continue;
        }
        if let Some(config) = config {
            if !matches_scalar_filter(
                year,
                &config.year_include_filters,
                &config.year_exclude_filters,
            ) {
                continue;
            }
        }

        let Some(title_entries) = titles.as_object() else {
            continue;
        };

        for title in title_entries.keys() {
            let trimmed = title.trim();
            if trimmed.is_empty() {
                continue;
            }
            records.push(Record {
                level: level.to_string(),
                conference: conference.to_string(),
                year: year.to_string(),
                title: trimmed.to_string(),
            });
        }
    }

    Ok(())
}

fn is_year_key(value: &str) -> bool {
    value.len() == 4 && value.chars().all(|character| character.is_ascii_digit())
}

fn deduplicate_records(records: Vec<Record>) -> Vec<Record> {
    let mut seen = HashSet::new();
    let mut deduplicated = Vec::new();

    for record in records {
        let key = (
            record.level.clone(),
            record.conference.clone(),
            record.year.clone(),
            record.title.clone(),
        );
        if seen.insert(key) {
            deduplicated.push(record);
        }
    }

    deduplicated
}

fn sorted_dirs(path: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut items = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            items.push(entry.path());
        }
    }
    items.sort();
    Ok(items)
}

fn sorted_json_files(path: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut items = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let item_path = entry.path();
        if item_path.extension().and_then(|value| value.to_str()) == Some("json") {
            items.push(item_path);
        }
    }
    items.sort();
    Ok(items)
}

fn file_name(path: &Path) -> Result<String, AppError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::Message(format!("invalid directory name: {}", path.display())))
}

fn file_stem(path: &Path) -> Result<String, AppError> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::Message(format!("invalid file name: {}", path.display())))
}

fn record_matches(record: &Record, config: &Config) -> bool {
    matches_scalar_filter(
        &record.level,
        &config.level_include_filters,
        &config.level_exclude_filters,
    ) && matches_scalar_filter(
        &record.conference,
        &config.conference_include_filters,
        &config.conference_exclude_filters,
    ) && matches_scalar_filter(
        &record.year,
        &config.year_include_filters,
        &config.year_exclude_filters,
    ) && matches_title_keywords(
        &record.title,
        &config.title_include_keywords,
        &config.title_exclude_keywords,
    )
}

fn matches_scalar_filter(
    value: &str,
    include_filters: &[String],
    exclude_filters: &[String],
) -> bool {
    let normalized = value.to_ascii_lowercase();
    (include_filters.is_empty() || include_filters.iter().any(|filter| filter == &normalized))
        && !exclude_filters.iter().any(|filter| filter == &normalized)
}

fn matches_title_keywords(
    title: &str,
    include_keywords: &[String],
    exclude_keywords: &[String],
) -> bool {
    let words: Vec<String> = title
        .split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect();

    include_keywords
        .iter()
        .all(|keyword| words.iter().any(|word| word.contains(keyword)))
        && !exclude_keywords
            .iter()
            .any(|keyword| words.iter().any(|word| word.contains(keyword)))
}

fn compare_records(left: &Record, right: &Record, sort_specs: &[SortSpec]) -> Ordering {
    for spec in sort_specs {
        let ordering = match spec.field {
            Field::Level => compare_case_insensitive(&left.level, &right.level),
            Field::Conference => compare_case_insensitive(&left.conference, &right.conference),
            Field::Year => compare_year(&left.year, &right.year),
            Field::Title => compare_case_insensitive(&left.title, &right.title),
        };

        let ordering = match spec.direction {
            Direction::Asc => ordering,
            Direction::Desc => ordering.reverse(),
        };

        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    Ordering::Equal
}

fn compare_case_insensitive(left: &str, right: &str) -> Ordering {
    left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase())
}

fn compare_year(left: &str, right: &str) -> Ordering {
    match (left.parse::<u32>(), right.parse::<u32>()) {
        (Ok(l), Ok(r)) => l.cmp(&r),
        _ => left.cmp(right),
    }
}

fn format_record(record: &Record, fields: &[Field]) -> String {
    let mut parts = Vec::with_capacity(fields.len());
    for field in fields {
        let value = match field {
            Field::Level => &record.level,
            Field::Conference => &record.conference,
            Field::Year => &record.year,
            Field::Title => &record.title,
        };
        parts.push(value.as_str());
    }
    parts.join("\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_columns_keeps_canonical_order() {
        let fields = parse_columns("title,year,conference").unwrap();
        assert_eq!(fields, vec![Field::Conference, Field::Year, Field::Title]);
    }

    #[test]
    fn parse_args_accepts_equals_forms_and_deduplicates_values() {
        let config = parse_args(
            [
                "--keyword=Graph, graph,Diffusion",
                "--exclude=Survey",
                "--level=A,a",
                "--exclude-level=B",
                "--conference=ICML,NeurIPS",
                "--exclude-conference=AAAI",
                "--year=2024,2025",
                "--exclude-year=2023",
                "--sort=year:desc",
                "--columns=title,conference,title",
                "--data-dir=custom-data",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert_eq!(
            config.title_include_keywords,
            vec!["graph".to_string(), "diffusion".to_string()]
        );
        assert_eq!(config.title_exclude_keywords, vec!["survey".to_string()]);
        assert_eq!(config.level_include_filters, vec!["a".to_string()]);
        assert_eq!(config.level_exclude_filters, vec!["b".to_string()]);
        assert_eq!(
            config.conference_include_filters,
            vec!["icml".to_string(), "neurips".to_string()]
        );
        assert_eq!(config.conference_exclude_filters, vec!["aaai".to_string()]);
        assert_eq!(
            config.year_include_filters,
            vec!["2024".to_string(), "2025".to_string()]
        );
        assert_eq!(config.year_exclude_filters, vec!["2023".to_string()]);
        assert_eq!(
            config.sort_specs,
            vec![SortSpec {
                field: Field::Year,
                direction: Direction::Desc,
            }]
        );
        assert_eq!(config.display_fields, vec![Field::Conference, Field::Title]);
        assert_eq!(config.paper_dir, Some(PathBuf::from("custom-data")));
    }

    #[test]
    fn parse_args_rejects_empty_filters_invalid_sort_and_empty_columns() {
        assert_message_contains(
            parse_args(["--keyword=, ,"].into_iter().map(str::to_string)).unwrap_err(),
            "filter value cannot be empty",
        );
        assert_message_contains(
            parse_args(["--sort=year", "diffusion"].into_iter().map(str::to_string)).unwrap_err(),
            "invalid sort spec",
        );
        assert_message_contains(
            parse_args(["--columns=,", "diffusion"].into_iter().map(str::to_string)).unwrap_err(),
            "at least one column",
        );
    }

    #[test]
    fn parse_args_accepts_combined_structured_filters_without_keywords() {
        let config = parse_args([
            "--level".to_string(),
            "A,B".to_string(),
            "--conference".to_string(),
            "AAAI".to_string(),
            "--year".to_string(),
            "2024,2025".to_string(),
        ])
        .unwrap();

        assert_eq!(
            config.level_include_filters,
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(config.conference_include_filters, vec!["aaai".to_string()]);
        assert_eq!(
            config.year_include_filters,
            vec!["2024".to_string(), "2025".to_string()]
        );
        assert!(config.title_include_keywords.is_empty());
    }

    #[test]
    fn parse_args_accepts_exclude_filters_only() {
        let config = parse_args([
            "--exclude-level".to_string(),
            "B".to_string(),
            "--exclude-conference".to_string(),
            "COLING,EACL".to_string(),
            "--exclude-year".to_string(),
            "2020,2021".to_string(),
            "--exclude-keyword".to_string(),
            "survey,tutorial".to_string(),
        ])
        .unwrap();

        assert_eq!(config.level_exclude_filters, vec!["b".to_string()]);
        assert_eq!(
            config.conference_exclude_filters,
            vec!["coling".to_string(), "eacl".to_string()]
        );
        assert_eq!(
            config.year_exclude_filters,
            vec!["2020".to_string(), "2021".to_string()]
        );
        assert_eq!(
            config.title_exclude_keywords,
            vec!["survey".to_string(), "tutorial".to_string()]
        );
    }

    #[test]
    fn parse_import_args_accepts_path_input_and_force() {
        let config = parse_import_args(
            [
                "--path=PaperJson/A/ICML.json",
                "--input=paper.json",
                "--force",
                "--no-backup",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert_eq!(
            config.target.path,
            Some(PathBuf::from("PaperJson/A/ICML.json"))
        );
        assert_eq!(
            config.source,
            ImportSource::File(PathBuf::from("paper.json"))
        );
        assert!(config.force);
        assert!(!config.backup);
    }

    #[test]
    fn import_and_remove_arg_parsers_reject_invalid_required_values() {
        assert_message_contains(
            parse_import_args(
                ["--path=PaperJson/A/ICML.json"]
                    .into_iter()
                    .map(str::to_string),
            )
            .unwrap_err(),
            "missing --input",
        );
        assert_message_contains(
            parse_import_args(
                ["--path=PaperJson/A/ICML.json", "--json={}", "--merge"]
                    .into_iter()
                    .map(str::to_string),
            )
            .unwrap_err(),
            "unsupported import option",
        );
        assert_message_contains(
            parse_remove_args(["--year=2026"].into_iter().map(str::to_string)).unwrap_err(),
            "missing --title",
        );
    }

    #[test]
    fn record_matches_supports_combined_filters() {
        let record = Record {
            level: "A".to_string(),
            conference: "ICML".to_string(),
            year: "2024".to_string(),
            title: "Graph-aware Diffusion Models for Retrieval".to_string(),
        };
        let config = Config {
            title_include_keywords: vec!["graph".to_string(), "diff".to_string()],
            title_exclude_keywords: vec!["survey".to_string()],
            level_include_filters: vec!["a".to_string()],
            level_exclude_filters: vec![],
            conference_include_filters: vec!["icml".to_string(), "neurips".to_string()],
            conference_exclude_filters: vec![],
            year_include_filters: vec!["2024".to_string()],
            year_exclude_filters: vec![],
            sort_specs: Vec::new(),
            display_fields: canonical_fields(),
            paper_dir: None,
            paper_path: None,
        };

        assert!(record_matches(&record, &config));

        let mut wrong_year = config;
        wrong_year.year_include_filters = vec!["2023".to_string()];
        assert!(!record_matches(&record, &wrong_year));
    }

    #[test]
    fn record_matches_respects_scalar_exclude_filters() {
        let record = Record {
            level: "A".to_string(),
            conference: "ICML".to_string(),
            year: "2024".to_string(),
            title: "Graph-aware Diffusion Models for Retrieval".to_string(),
        };
        let config = Config {
            title_include_keywords: vec![],
            title_exclude_keywords: vec![],
            level_include_filters: vec![],
            level_exclude_filters: vec!["b".to_string()],
            conference_include_filters: vec![],
            conference_exclude_filters: vec!["neurips".to_string()],
            year_include_filters: vec![],
            year_exclude_filters: vec!["2023".to_string()],
            sort_specs: Vec::new(),
            display_fields: canonical_fields(),
            paper_dir: None,
            paper_path: None,
        };

        assert!(record_matches(&record, &config));

        let mut excluded_conf = config;
        excluded_conf.conference_exclude_filters = vec!["icml".to_string()];
        assert!(!record_matches(&record, &excluded_conf));
    }

    #[test]
    fn title_keyword_matching_uses_space_split_and_substring_logic() {
        assert!(matches_title_keywords(
            "Graph-aware Diffusion Models for Retrieval",
            &["graph".to_string(), "diff".to_string()],
            &[]
        ));
        assert!(!matches_title_keywords(
            "Graph-aware Diffusion Models for Retrieval",
            &["graph".to_string(), "survey".to_string()],
            &[]
        ));
        assert!(!matches_title_keywords(
            "Graph-aware Diffusion Models for Retrieval",
            &["graph".to_string()],
            &["retriev".to_string()]
        ));
    }

    #[test]
    fn compare_records_respects_priority_order() {
        let mut records = [
            Record {
                level: "A".to_string(),
                conference: "ICML".to_string(),
                year: "2023".to_string(),
                title: "B".to_string(),
            },
            Record {
                level: "A".to_string(),
                conference: "ICML".to_string(),
                year: "2024".to_string(),
                title: "A".to_string(),
            },
            Record {
                level: "B".to_string(),
                conference: "ACL".to_string(),
                year: "2024".to_string(),
                title: "C".to_string(),
            },
        ];

        let specs = vec![
            SortSpec {
                field: Field::Year,
                direction: Direction::Desc,
            },
            SortSpec {
                field: Field::Title,
                direction: Direction::Asc,
            },
        ];

        records.sort_by(|left, right| compare_records(left, right, &specs));

        assert_eq!(records[0].year, "2024");
        assert_eq!(records[0].title, "A");
        assert_eq!(records[1].year, "2024");
        assert_eq!(records[1].title, "C");
        assert_eq!(records[2].year, "2023");
    }

    #[test]
    fn normalize_conference_document_migrates_legacy_years_and_entries() {
        let mut document = serde_json::json!({
            "2024": {
                "Migrated Paper": {
                    "auth": "Legacy Author",
                    "source": "drop me",
                    "url": "https://example.com/old"
                }
            },
            "papers": {
                "2024": {
                    "Existing Paper": "raw entry"
                },
                "2025": {
                    "Modern Paper": {
                        "sources": ["drop me"]
                    }
                }
            }
        });

        normalize_conference_document(&mut document, "A", "ICML").unwrap();

        assert_eq!(document["schema_version"], serde_json::json!(1));
        assert_eq!(document["level"], "A");
        assert_eq!(document["conference"], "ICML");
        assert!(document.get("2024").is_none());

        let migrated = &document["papers"]["2024"]["Migrated Paper"];
        assert_eq!(migrated["author"], "Legacy Author");
        assert_eq!(migrated["bib"], "");
        assert_eq!(migrated["url"], "https://example.com/old");
        assert!(migrated.get("auth").is_none());
        assert!(migrated.get("source").is_none());

        let existing = &document["papers"]["2024"]["Existing Paper"];
        assert_eq!(existing["value"], "raw entry");
        assert_eq!(existing["author"], "");
        assert_eq!(existing["bib"], "");
        assert_eq!(existing["url"], "");

        let modern = &document["papers"]["2025"]["Modern Paper"];
        assert!(modern.get("sources").is_none());
        assert_eq!(modern["author"], "");
    }

    #[test]
    fn force_import_replaces_same_year_entry() {
        let mut document = serde_json::json!({
            "papers": {
                "2026": {
                    "Patchable Paper": {
                        "author": "Original Author",
                        "bib": "Original Bib",
                        "url": "https://example.com/original",
                        "tags": ["old"]
                    }
                }
            }
        });
        normalize_conference_document(&mut document, "A", "ICML").unwrap();

        let record = PaperImportRecord {
            year: "2026".to_string(),
            title: "Patchable Paper".to_string(),
            entry: Map::from_iter([
                (
                    "author".to_string(),
                    Value::String("Updated Author".to_string()),
                ),
                ("score".to_string(), serde_json::json!(0.9)),
            ]),
        };

        let stats = apply_import_records(&mut document, &[record], true).unwrap();

        let entry = &document["papers"]["2026"]["Patchable Paper"];
        assert_eq!(stats.replaced, 1);
        assert_eq!(entry["author"], "Updated Author");
        assert_eq!(entry["bib"], "");
        assert_eq!(entry["url"], "");
        assert!(entry.get("tags").is_none());
        assert_eq!(entry["score"], serde_json::json!(0.9));
    }

    #[test]
    fn import_does_not_block_on_unrelated_existing_duplicates() {
        let mut document = serde_json::json!({
            "papers": {
                "2024": {
                    "Front Matter": {}
                },
                "2025": {
                    "Front Matter": {}
                }
            }
        });
        normalize_conference_document(&mut document, "A", "VLDB").unwrap();

        let record = PaperImportRecord {
            year: "2026".to_string(),
            title: "New Unique Paper".to_string(),
            entry: Map::new(),
        };

        let stats = apply_import_records(&mut document, &[record], false).unwrap();

        assert_eq!(stats.inserted, 1);
        assert!(document["papers"]["2026"].get("New Unique Paper").is_some());
    }

    #[test]
    fn remove_entry_prunes_empty_year() {
        let mut document = serde_json::json!({
            "papers": {
                "2026": {
                    "Only Paper": {}
                },
                "2027": {
                    "Future Paper": {}
                }
            }
        });
        let config = RemoveConfig {
            target: TargetConfig::default(),
            year: Some("2026".to_string()),
            title: "Only Paper".to_string(),
            backup: false,
        };

        remove_entry_from_document(&mut document, &config).unwrap();

        assert!(document["papers"].get("2026").is_none());
        assert!(document["papers"].get("2027").is_some());
    }

    #[test]
    fn load_records_reads_level_conference_year_from_json_path() {
        let root = temp_test_dir();
        let paper_json_dir = root.join("PaperJson");
        fs::create_dir_all(paper_json_dir.join("A")).unwrap();
        fs::create_dir_all(paper_json_dir.join("B")).unwrap();
        fs::write(
            paper_json_dir.join("A").join("ICML.json"),
            r#"{
  "schema_version": 1,
  "level": "A",
  "conference": "ICML",
  "papers": {
    "2024": {
      "First Paper": {},
      "Second Paper": {}
    }
  }
}"#,
        )
        .unwrap();
        fs::write(
            paper_json_dir.join("B").join("EMNLP.json"),
            r#"{
  "schema_version": 1,
  "level": "B",
  "conference": "EMNLP",
  "papers": {
    "2025": {
      "Third Paper": {}
    }
  }
}"#,
        )
        .unwrap();

        let records = load_records(&paper_json_dir).unwrap();

        assert_eq!(records.len(), 3);
        assert_eq!(records[0].level, "A");
        assert_eq!(records[0].conference, "ICML");
        assert_eq!(records[0].year, "2024");
        assert_eq!(records[0].title, "First Paper");
        assert_eq!(records[2].level, "B");
        assert_eq!(records[2].conference, "EMNLP");
        assert_eq!(records[2].year, "2025");
        assert_eq!(records[2].title, "Third Paper");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_records_reads_per_conference_json() {
        let root = temp_test_dir();
        let paper_json_dir = root.join("PaperJson");
        fs::create_dir_all(paper_json_dir.join("A")).unwrap();
        fs::write(
            paper_json_dir.join("A").join("ICML.json"),
            r#"{
    "2024": {
      "JSON Paper": {
      "author": "A. Author",
      "bib": "",
      "url": "https://example.com"
    }
  },
  "2025": {
    "Another JSON Paper": {
      "author": "",
      "bib": "",
      "url": ""
    }
  }
}"#,
        )
        .unwrap();

        let records = load_records(&paper_json_dir).unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].level, "A");
        assert_eq!(records[0].conference, "ICML");
        assert_eq!(records[0].year, "2024");
        assert_eq!(records[0].title, "JSON Paper");
        assert_eq!(records[1].year, "2025");
        assert_eq!(records[1].title, "Another JSON Paper");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_filtered_records_skips_excluded_directories_before_reading_json() {
        let root = temp_test_dir();
        let paper_json_dir = root.join("PaperJson");
        fs::create_dir_all(paper_json_dir.join("A")).unwrap();
        fs::create_dir_all(paper_json_dir.join("B")).unwrap();
        fs::write(
            paper_json_dir.join("A").join("ICML.json"),
            r#"{
  "papers": {
    "2026": {
      "Included Paper": {}
    }
  }
}"#,
        )
        .unwrap();
        fs::write(paper_json_dir.join("A").join("SKIPPED.json"), "not json").unwrap();
        fs::write(paper_json_dir.join("B").join("BAD.json"), "not json").unwrap();

        let config = parse_args(
            ["--level=A", "--conference=ICML", "--year=2026"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        let records = load_filtered_records(&paper_json_dir, &config).unwrap();

        assert_eq!(
            records,
            vec![Record {
                level: "A".to_string(),
                conference: "ICML".to_string(),
                year: "2026".to_string(),
                title: "Included Paper".to_string(),
            }]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_records_trims_titles_and_ignores_non_year_keys() {
        let root = temp_test_dir();
        let paper_json_dir = root.join("PaperJson");
        fs::create_dir_all(paper_json_dir.join("A")).unwrap();
        fs::write(
            paper_json_dir.join("A").join("TRIM.json"),
            r#"{
  "papers": {
    "2026": {
      "  Trimmed Paper  ": {},
      "   ": {},
      "": {}
    },
    "latest": {
      "Ignored Paper": {}
    }
  }
}"#,
        )
        .unwrap();

        let records = load_records(&paper_json_dir).unwrap();

        assert_eq!(
            records,
            vec![Record {
                level: "A".to_string(),
                conference: "TRIM".to_string(),
                year: "2026".to_string(),
                title: "Trimmed Paper".to_string(),
            }]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deduplicate_records_keeps_first_duplicate_record() {
        let duplicate = Record {
            level: "A".to_string(),
            conference: "ICML".to_string(),
            year: "2026".to_string(),
            title: "Same Paper".to_string(),
        };
        let different_year = Record {
            year: "2025".to_string(),
            ..duplicate.clone()
        };

        let records = deduplicate_records(vec![
            duplicate.clone(),
            duplicate.clone(),
            different_year.clone(),
        ]);

        assert_eq!(records, vec![duplicate, different_year]);
    }

    fn assert_message_contains(error: AppError, expected: &str) {
        match error {
            AppError::Message(message) => assert!(
                message.contains(expected),
                "expected error to contain {expected:?}, got {message:?}"
            ),
            other => panic!("expected AppError::Message containing {expected:?}, got {other:?}"),
        }
    }

    fn temp_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("topaperlist-search-test-{nanos}"));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
