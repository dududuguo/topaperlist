use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_search")
}

fn paper_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("PaperJson")
}

fn run_search(args: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary_path());
    command.arg("--paper-dir").arg(paper_dir());
    command.args(args);
    command.output().expect("failed to run search binary")
}

fn run_search_without_default_data_dir(args: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary_path());
    command.args(args);
    command.output().expect("failed to run search binary")
}

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout is not utf-8");
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "command failed: status={:?}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure_contains(output: &std::process::Output, expected: &str) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded: stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "expected stderr to contain {expected:?}, got {stderr:?}"
    );
}

fn title_matches_keywords(title: &str, include: &[&str], exclude: &[&str]) -> bool {
    let words: Vec<String> = title
        .split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect();

    include.iter().all(|keyword| {
        words
            .iter()
            .any(|word| word.contains(&keyword.to_ascii_lowercase()))
    }) && !exclude.iter().any(|keyword| {
        words
            .iter()
            .any(|word| word.contains(&keyword.to_ascii_lowercase()))
    })
}

#[test]
fn help_command_prints_usage() {
    let output = run_search(&["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("--exclude-year"));
}

#[test]
fn add_command_is_removed() {
    let output = run_search_without_default_data_dir(&["add", "--help"]);
    assert_failure_contains(&output, "search add has been removed");
}

#[test]
fn search_without_filters_fails_with_actionable_error() {
    let output = run_search(&[]);
    assert_failure_contains(&output, "at least one filter is required");
}

#[test]
fn search_can_discover_default_paperjson_from_project_root() {
    let output = run_search_without_default_data_dir(&[
        "--conference",
        "EMNLP",
        "--year",
        "2020",
        "attention",
        "chinese",
        "segmentation",
    ]);

    assert_success(&output);
    assert_eq!(
        stdout_lines(&output),
        vec![
            "B\tEMNLP\t2020\tAttention Is All You Need for Chinese Word Segmentation.".to_string()
        ]
    );
}

#[test]
fn invalid_search_options_return_errors() {
    let bad_sort = run_search(&["--sort", "year", "diffusion"]);
    assert_failure_contains(&bad_sort, "invalid sort spec");

    let bad_columns = run_search(&["--columns", ",", "diffusion"]);
    assert_failure_contains(&bad_columns, "at least one column");
}

#[test]
fn positional_keywords_match_explicit_keyword_option() {
    let positional = run_search(&["graph", "diffusion"]);
    let explicit = run_search(&["--keyword", "graph,diffusion"]);

    assert_success(&positional);
    assert_success(&explicit);
    assert_eq!(positional.stdout, explicit.stdout);
}

#[test]
fn attention_is_all_you_need_query_returns_expected_record() {
    let output = run_search(&[
        "--conference",
        "EMNLP",
        "--year",
        "2020",
        "attention",
        "is",
        "all",
        "you",
        "need",
    ]);

    assert_success(&output);
    assert_eq!(
        stdout_lines(&output),
        vec![
            "B\tEMNLP\t2020\tAttention Is All You Need for Chinese Word Segmentation.".to_string()
        ]
    );
}

#[test]
fn level_conference_year_filters_are_exact_and_case_insensitive() {
    let output = run_search(&["--level", "a", "--conference", "aaai", "--year", "2024"]);
    assert_success(&output);

    let lines = stdout_lines(&output);
    assert!(!lines.is_empty(), "expected results for AAAI 2024");

    for line in lines {
        let parts: Vec<&str> = line.split('\t').collect();
        assert_eq!(parts.len(), 4, "unexpected column count in line: {line}");
        assert_eq!(parts[0], "A");
        assert_eq!(parts[1], "AAAI");
        assert_eq!(parts[2], "2024");
    }
}

#[test]
fn keyword_include_and_exclude_arrays_filter_titles() {
    let output = run_search(&[
        "--keyword",
        "graph,diffusion",
        "--exclude-keyword",
        "survey,tutorial",
    ]);
    assert_success(&output);

    let lines = stdout_lines(&output);
    assert!(!lines.is_empty(), "expected keyword results");

    for line in lines {
        let parts: Vec<&str> = line.split('\t').collect();
        assert_eq!(parts.len(), 4, "unexpected column count in line: {line}");
        assert!(title_matches_keywords(
            parts[3],
            &["graph", "diffusion"],
            &["survey", "tutorial"]
        ));
    }
}

#[test]
fn scalar_exclude_filters_work_together() {
    let output = run_search(&[
        "--exclude-level",
        "B",
        "--exclude-conference",
        "AAAI,ICML",
        "--exclude-year",
        "2024,2025",
        "diffusion",
    ]);
    assert_success(&output);

    let lines = stdout_lines(&output);
    assert!(
        !lines.is_empty(),
        "expected results after scalar exclusions"
    );

    for line in lines {
        let parts: Vec<&str> = line.split('\t').collect();
        assert_eq!(parts.len(), 4, "unexpected column count in line: {line}");
        assert_ne!(parts[0], "B");
        assert_ne!(parts[1], "AAAI");
        assert_ne!(parts[1], "ICML");
        assert_ne!(parts[2], "2024");
        assert_ne!(parts[2], "2025");
        assert!(title_matches_keywords(parts[3], &["diffusion"], &[]));
    }
}

#[test]
fn mixed_filters_sorting_and_columns_are_honored() {
    let output = run_search(&[
        "--conference",
        "ICML,NeurIPS",
        "--exclude-year",
        "2025",
        "--sort",
        "conference:asc",
        "--sort",
        "year:desc",
        "--columns",
        "conference,year,title",
        "diffusion",
    ]);
    assert_success(&output);

    let lines = stdout_lines(&output);
    assert!(!lines.is_empty(), "expected sorted filtered results");

    let mut previous: Option<(String, u32, String)> = None;
    for line in lines {
        let parts: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            parts.len(),
            3,
            "unexpected filtered column count in line: {line}"
        );
        assert!(matches!(parts[0], "ICML" | "NeurIPS"));
        assert_ne!(parts[1], "2025");
        assert!(title_matches_keywords(parts[2], &["diffusion"], &[]));

        let current = (
            parts[0].to_string(),
            parts[1].parse::<u32>().expect("year should be numeric"),
            parts[2].to_string(),
        );

        if let Some(previous) = &previous {
            assert!(
                previous.0 < current.0 || (previous.0 == current.0 && previous.1 >= current.1),
                "results are not sorted by conference asc, year desc: prev={previous:?}, current={current:?}"
            );
        }

        previous = Some(current);
    }
}

#[test]
fn import_command_writes_json_that_search_can_index() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("TESTCONF.json");
    let input_path = root.join("paper.json");
    std::fs::write(
        &input_path,
        r#"{
  "year": 2026,
  "title": "CLI Imported Paper",
  "author": "A. Author",
  "url": "https://example.com",
  "tags": ["json", "api"]
}"#,
    )
    .unwrap();
    let json_path_string = json_path.to_string_lossy().to_string();
    let input_path_string = input_path.to_string_lossy().to_string();
    let data_dir_string = data_dir.to_string_lossy().to_string();

    let import_output = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &input_path_string,
        "--no-backup",
    ]);
    assert_success(&import_output);

    let query_output = run_search_without_default_data_dir(&[
        "--data-dir",
        &data_dir_string,
        "--conference",
        "TESTCONF",
        "--year",
        "2026",
        "imported",
    ]);
    assert_success(&query_output);
    assert_eq!(
        stdout_lines(&query_output),
        vec!["A\tTESTCONF\t2026\tCLI Imported Paper".to_string()]
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn import_command_rejects_reserved_fields() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("BADFIELD.json");
    let input_path = root.join("bad.json");
    std::fs::write(
        &input_path,
        r#"{
  "year": "2026",
  "title": "Bad Field Paper",
  "source": "legacy"
}"#,
    )
    .unwrap();
    let json_path_string = json_path.to_string_lossy().to_string();
    let input_path_string = input_path.to_string_lossy().to_string();

    let output = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &input_path_string,
        "--no-backup",
    ]);

    assert_failure_contains(&output, "disallowed field");
    assert!(!data_dir.exists());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn import_command_force_allows_duplicate_title() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("UPDCONF.json");
    let json_path_string = json_path.to_string_lossy().to_string();
    let first_input = root.join("first.json");
    let second_input = root.join("second.json");
    std::fs::write(
        &first_input,
        r#"{
  "year": "2026",
  "title": "Updatable Paper",
  "author": "Original Author",
  "score": 1
}"#,
    )
    .unwrap();
    std::fs::write(
        &second_input,
        r#"{
  "year": "2027",
  "title": "Updatable Paper",
  "url": "https://example.com/updated",
  "tags": ["updated"]
}"#,
    )
    .unwrap();
    let first_input_string = first_input.to_string_lossy().to_string();
    let second_input_string = second_input.to_string_lossy().to_string();

    let first_import = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &first_input_string,
        "--no-backup",
    ]);
    assert_success(&first_import);

    let second_import = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &second_input_string,
        "--force",
        "--no-backup",
    ]);
    assert_success(&second_import);

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    let first_entry = &value["papers"]["2026"]["Updatable Paper"];
    let second_entry = &value["papers"]["2027"]["Updatable Paper"];
    assert_eq!(first_entry["author"], "Original Author");
    assert_eq!(first_entry["score"], serde_json::json!(1));
    assert_eq!(second_entry["author"], "");
    assert_eq!(second_entry["url"], "https://example.com/updated");
    assert_eq!(second_entry["tags"], serde_json::json!(["updated"]));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn import_command_creates_missing_conference_and_appends_existing_conference() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let data_dir_string = data_dir.to_string_lossy().to_string();
    let json_path = data_dir.join("A").join("NEWCONF.json");
    let json_path_string = json_path.to_string_lossy().to_string();
    let first_input = root.join("created1.json");
    let second_input = root.join("created2.json");
    std::fs::write(
        &first_input,
        r#"{"year":"2026","title":"First Created Paper"}"#,
    )
    .unwrap();
    std::fs::write(
        &second_input,
        r#"{"year":"2026","title":"Second Appended Paper"}"#,
    )
    .unwrap();
    let first_input_string = first_input.to_string_lossy().to_string();
    let second_input_string = second_input.to_string_lossy().to_string();

    let first_import = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &first_input_string,
        "--no-backup",
    ]);
    assert_success(&first_import);

    let second_import = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &second_input_string,
        "--no-backup",
    ]);
    assert_success(&second_import);

    let output = run_search_without_default_data_dir(&[
        "--data-dir",
        &data_dir_string,
        "--conference",
        "NEWCONF",
        "--year",
        "2026",
        "paper",
        "--sort",
        "title:asc",
    ]);
    assert_success(&output);
    assert_eq!(
        stdout_lines(&output),
        vec![
            "A\tNEWCONF\t2026\tFirst Created Paper".to_string(),
            "A\tNEWCONF\t2026\tSecond Appended Paper".to_string(),
        ]
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn import_command_rejects_duplicate_title_by_default() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("DUPCONF.json");
    let json_path_string = json_path.to_string_lossy().to_string();
    let first_input = root.join("dup-first.json");
    let second_input = root.join("dup-second.json");
    std::fs::write(
        &first_input,
        r#"{"year":"2026","title":"Duplicate Paper","author":"Original"}"#,
    )
    .unwrap();
    std::fs::write(
        &second_input,
        r#"{"year":"2027","title":"Duplicate Paper","author":"Replacement"}"#,
    )
    .unwrap();
    let first_input_string = first_input.to_string_lossy().to_string();
    let second_input_string = second_input.to_string_lossy().to_string();

    let first_import = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &first_input_string,
        "--no-backup",
    ]);
    assert_success(&first_import);

    let duplicate_import = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &second_input_string,
        "--no-backup",
    ]);
    assert_failure_contains(&duplicate_import, "duplicate title");

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert_eq!(
        value["papers"]["2026"]["Duplicate Paper"]["author"],
        "Original"
    );
    assert!(value["papers"].get("2027").is_none());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_path_limits_indexing_to_one_json_file() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("ONLY.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(
        &json_path,
        r#"{
  "papers": {
    "2026": {
      "Scoped Search Paper": {}
    }
  }
}"#,
    )
    .unwrap();
    std::fs::create_dir_all(data_dir.join("B")).unwrap();
    std::fs::write(data_dir.join("B").join("BROKEN.json"), "not json").unwrap();
    let json_path_string = json_path.to_string_lossy().to_string();

    let output = run_search_without_default_data_dir(&[
        "--path",
        &json_path_string,
        "--year",
        "2026",
        "scoped",
    ]);
    assert_success(&output);
    assert_eq!(
        stdout_lines(&output),
        vec!["A\tONLY\t2026\tScoped Search Paper".to_string()]
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn remove_command_deletes_title_and_keeps_other_titles() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let data_dir_string = data_dir.to_string_lossy().to_string();
    let json_path = data_dir.join("A").join("REMCONF.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(
        &json_path,
        r#"{
  "schema_version": 1,
  "level": "A",
  "conference": "REMCONF",
  "papers": {
    "2026": {
      "Paper To Remove": {},
      "Paper To Keep": {}
    }
  }
}"#,
    )
    .unwrap();
    let json_path_string = json_path.to_string_lossy().to_string();

    let remove_output = run_search_without_default_data_dir(&[
        "remove",
        "--path",
        &json_path_string,
        "--title",
        "Paper To Remove",
        "--no-backup",
    ]);
    assert_success(&remove_output);

    let removed_query = run_search_without_default_data_dir(&[
        "--data-dir",
        &data_dir_string,
        "--conference",
        "REMCONF",
        "--year",
        "2026",
        "remove",
    ]);
    assert_success(&removed_query);
    assert!(stdout_lines(&removed_query).is_empty());

    let kept_query = run_search_without_default_data_dir(&[
        "--data-dir",
        &data_dir_string,
        "--conference",
        "REMCONF",
        "--year",
        "2026",
        "keep",
    ]);
    assert_success(&kept_query);
    assert_eq!(
        stdout_lines(&kept_query),
        vec!["A\tREMCONF\t2026\tPaper To Keep".to_string()]
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn remove_command_prunes_empty_year_from_json_file() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("PRUNECONF.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(
        &json_path,
        r#"{
  "papers": {
    "2026": {
      "Only Paper In Year": {}
    }
  }
}"#,
    )
    .unwrap();
    let json_path_string = json_path.to_string_lossy().to_string();

    let remove_output = run_search_without_default_data_dir(&[
        "remove",
        "--path",
        &json_path_string,
        "--title",
        "Only Paper In Year",
        "--no-backup",
    ]);
    assert_success(&remove_output);

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert!(value["papers"].get("2026").is_none());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn remove_command_reports_missing_title() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("MISSREM.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(
        &json_path,
        r#"{
  "papers": {
    "2026": {
      "Existing Paper": {}
    }
  }
}"#,
    )
    .unwrap();
    let json_path_string = json_path.to_string_lossy().to_string();

    let remove_output = run_search_without_default_data_dir(&[
        "remove",
        "--path",
        &json_path_string,
        "--title",
        "Missing Paper",
        "--no-backup",
    ]);
    assert_failure_contains(&remove_output, "title not found");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn import_command_preserves_legacy_top_level_year_records() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("ICML.json");
    let input_path = root.join("new-style.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(
        &json_path,
        r#"{
  "2024": {
    "Old Style Paper": {
      "author": "",
      "bib": "",
      "url": ""
    }
  }
}"#,
    )
    .unwrap();
    std::fs::write(&input_path, r#"{"year":"2026","title":"New Style Paper"}"#).unwrap();

    let json_path_string = json_path.to_string_lossy().to_string();
    let input_path_string = input_path.to_string_lossy().to_string();
    let data_dir_string = data_dir.to_string_lossy().to_string();
    let import_output = run_search_without_default_data_dir(&[
        "import",
        "--path",
        &json_path_string,
        "--input",
        &input_path_string,
        "--no-backup",
    ]);
    assert_success(&import_output);

    let legacy_query = run_search_without_default_data_dir(&[
        "--data-dir",
        &data_dir_string,
        "--conference",
        "ICML",
        "--year",
        "2024",
        "old",
        "style",
    ]);
    assert_success(&legacy_query);
    assert_eq!(
        stdout_lines(&legacy_query),
        vec!["A\tICML\t2024\tOld Style Paper".to_string()]
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_reads_json_with_utf8_bom() {
    let root = temp_test_dir();
    let data_dir = root.join("PaperJson");
    let json_path = data_dir.join("A").join("BOMCONF.json");
    std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
    std::fs::write(
        &json_path,
        concat!(
            "\u{feff}",
            r#"{
  "schema_version": 1,
  "level": "A",
  "conference": "BOMCONF",
  "papers": {
    "2026": {
      "BOM Encoded Paper": {
        "author": "",
        "bib": "",
        "url": ""
      }
    }
  }
}"#
        ),
    )
    .unwrap();

    let data_dir_string = data_dir.to_string_lossy().to_string();
    let output = run_search_without_default_data_dir(&[
        "--data-dir",
        &data_dir_string,
        "--conference",
        "BOMCONF",
        "--year",
        "2026",
        "encoded",
    ]);
    assert_success(&output);
    assert_eq!(
        stdout_lines(&output),
        vec!["A\tBOMCONF\t2026\tBOM Encoded Paper".to_string()]
    );

    std::fs::remove_dir_all(root).unwrap();
}

fn temp_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("topaperlist-cli-test-{nanos}"));
    std::fs::create_dir_all(&path).unwrap();
    path
}
