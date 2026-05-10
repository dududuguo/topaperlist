# Wiki

## Data Layout

The canonical dataset lives in:

```text
PaperJson/<level>/<conference>.json
```

Each file contains one conference. The `papers` object is grouped by year and then keyed by title:

```json
{
  "schema_version": 1,
  "level": "A",
  "conference": "ICML",
  "papers": {
    "2024": {
      "Paper Title": {
        "author": "",
        "bib": "",
        "url": ""
      }
    }
  }
}
```

Per-paper entries may include extra metadata fields, but `auth`, `source`, and `sources` are not part of the schema.

## JSON API

Import records through JSON:

```bash
search import --path PaperJson/A/ICML.json --input paper.json
```

`paper.json` may contain one paper object, a list of paper objects, or a full conference document. A single-paper import looks like this:

```json
{
  "year": "2026",
  "title": "Paper Title",
  "author": "A. Author",
  "bib": "@inproceedings{paper2026,...}",
  "url": "https://example.com/paper",
  "tags": ["llm", "agent"]
}
```

When `--path` is used, the import only checks and writes that one conference JSON file. `level` is inferred from the parent directory and `conference` is inferred from the file name. If the conference JSON does not exist, import creates it.

Title is the unique key inside the target JSON. By default, import fails before writing anything if the target file already contains the same title.

Use `--force` only when you deliberately want to bypass that duplicate check:

```bash
search import --path PaperJson/A/ICML.json --input paper.json --force
```

With `--force`, the imported record is written to its JSON `year`. If the exact same year/title already exists, that entry is replaced. If the same title exists in another year, the import still adds the new year/title entry.

If a target file already has the same title in multiple years, removing such a title requires `--year`.

Imports create a `.bak` file before overwriting an existing JSON file. Pass `--no-backup` for deliberate in-place rewrites.

Remove one exact title:

```bash
search remove --path PaperJson/A/ICML.json --title "Paper Title"
```

Or remove by data directory:

```bash
search remove --data-dir PaperJson --level A --conference ICML --title "Paper Title"
```

`search remove` removes the matching title and prunes the year if it becomes empty. `--year` is optional and can be used as an extra constraint. It does not delete the whole conference JSON file.

## Python API

The Python helper uses the same JSON import model:

```bash
python scripts/paper_api.py import --path PaperJson/A/ICML.json --input paper.json --force
```

Remove one exact title:

```bash
python scripts/paper_api.py remove --path PaperJson/A/ICML.json --title "Paper Title"
```

Normalize, validate, and inspect the dataset:

```bash
python scripts/paper_api.py normalize
python scripts/paper_api.py validate
python scripts/paper_api.py stats
```

`import` and `normalize` create `.bak` files before overwriting existing JSON. Use `--no-backup` only for deliberate in-place rewrites.

## Search

Run through Cargo during development:

```bash
cargo run -- --conference AAAI --year 2024 diffusion
```

After installation, run directly:

```bash
search --conference AAAI --year 2024 diffusion
```

Override the data directory when needed:

```bash
search --data-dir /path/to/PaperJson --conference ACL --year 2024 transformer
```

Search one specific conference JSON file:

```bash
search --path PaperJson/A/ICML.json --year 2024 transformer
```

Each matching record is printed as a tab-separated row:

```text
level	conference	year	title
```

## Search Rules

- At least one filter is required.
- All filters are case-insensitive.
- `--keyword` and positional keywords are equivalent.
- Repeated values are deduplicated automatically.
- Comma-separated values are supported for every repeatable filter.
- `level`, `conference`, and `year` filters use exact matching after normalization.
- Title keyword matching is token-based: the title is split on whitespace, and each keyword must appear as a substring of at least one token.
- Exclude filters remove matches after include filters are applied.

## Supported Filters

All of the following options are repeatable and also accept comma-separated lists:

- Title include keywords: `-k`, `--keyword`, or positional arguments
- Title exclude keywords: `-x`, `--exclude`, `--exclude-keyword`
- Level include filter: `-l`, `--level`
- Level exclude filter: `--exclude-level`
- Conference include filter: `-n`, `--conference`
- Conference exclude filter: `--exclude-conference`
- Year include filter: `-y`, `--year`
- Year exclude filter: `--exclude-year`

## Sorting

Sort with `--sort <field>:<order>`. Supported fields are `level`, `conference`, `year`, and `title`. Supported orders are `asc` and `desc`.

```bash
search diffusion --sort conference:asc --sort year:desc
```

If no sort rule is provided, records are emitted in repository traversal order.

## Columns

The default output columns are:

```text
level -> conference -> year -> title
```

Use `--columns` to print a subset:

```bash
search --columns conference,year,title diffusion
```

Output order always follows the canonical order `level, conference, year, title`.

## Install

Install Rust/Cargo first:

https://rustwiki.org/en/cargo/getting-started/installation.html

Windows / PowerShell:

```powershell
.\install.ps1
```

Linux / macOS:

```bash
sh ./install.sh
```

By default, the install scripts build the release binary, install `search`, copy `PaperJson/`, and run a smoke test.

To avoid a command name conflict:

```powershell
.\install.ps1 -CommandName topaper-search
```

```bash
COMMAND_NAME=topaper-search sh ./install.sh
```

## Development

```bash
cargo test
cargo build --release
python scripts/paper_api.py validate
```

Integration-style CLI checks live in `tests/`.

## Releases

GitHub Releases are created from version tags. Pushing a tag such as `v1.0.0` runs the release workflow, builds Windows and Linux packages, and uploads them to the GitHub Release page.
