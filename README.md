# Top Conferences Paper List

Accepted paper titles are stored in JSON and queried with the `search` CLI.

## Quick Start

Search:

```bash
search --conference AAAI --year 2024 diffusion
```

Import paper metadata from JSON:

```bash
search import --path PaperJson/A/ICML.json --input paper.json --force
```

Remove a paper:

```bash
search remove --path PaperJson/A/ICML.json --title "Paper Title"
```

Validate the dataset:

```bash
python scripts/paper_api.py validate
```

## Data

Canonical data lives in:

```text
PaperJson/<level>/<conference>.json
```

Each conference file stores papers grouped by year and keyed by title.

## Install

```powershell
.\install.ps1
```

```bash
sh ./install.sh
```

## Docs

See [WIKI.md](WIKI.md) for the JSON schema, full command reference, install options, and development notes.
