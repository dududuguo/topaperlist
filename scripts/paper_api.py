#!/usr/bin/env python3
"""Maintain PaperJson data safely.

Examples:
  python scripts/paper_api.py import --path PaperJson/A/ICML.json --input paper.json --force
  python scripts/paper_api.py remove --path PaperJson/A/ICML.json --title "My Paper"
  python scripts/paper_api.py normalize
  python scripts/paper_api.py validate
  python scripts/paper_api.py stats
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from paperjson import (
    DEFAULT_FIELDS,
    ConferenceDocument,
    load_document,
    sort_years_and_titles,
    write_document,
)


def conference_json_path(json_dir: Path, level: str, conference: str) -> Path:
    return json_dir / level / f"{conference}.json"


def resolve_conference_target(args: argparse.Namespace) -> tuple[Path, str, str]:
    if args.path is not None:
        level = args.level or args.path.parent.name
        conference = args.conference or args.path.stem
        if not level or not conference:
            raise SystemExit("--path requires an inferable level and conference")
        return args.path, level, conference

    if not args.level:
        raise SystemExit("missing --level")
    if not args.conference:
        raise SystemExit("missing --conference")
    return conference_json_path(args.json_dir, args.level, args.conference), args.level, args.conference


def normalize_title_key(title: str) -> str:
    return title.strip().casefold()


def require_year(value: Any, context: str) -> str:
    if isinstance(value, int):
        value = str(value)
    if not isinstance(value, str) or not value.isdigit() or len(value) != 4:
        raise SystemExit(f"{context}: invalid year {value!r}")
    return value


def require_title(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise SystemExit(f"{context}: title must be a non-empty string")
    return value.strip()


def validate_entry(entry: dict[str, Any], context: str) -> None:
    for key, value in entry.items():
        if not key.strip():
            raise SystemExit(f"{context}: empty field key")
        if key in {"auth", "source", "sources"}:
            raise SystemExit(f"{context}: disallowed field {key!r}")
        if key in {"year", "title"}:
            raise SystemExit(f"{context}: identity field {key!r} cannot be metadata")
        if key in {"author", "bib", "url"} and not isinstance(value, str):
            raise SystemExit(f"{context}: field {key!r} must be a string")


def parse_record(raw: Any, context: str) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise SystemExit(f"{context}: record must be an object")
    raw = raw.copy()
    if "year" not in raw:
        raise SystemExit(f"{context}: missing year")
    if "title" not in raw:
        raise SystemExit(f"{context}: missing title")
    year = require_year(raw.pop("year"), context)
    title = require_title(raw.pop("title"), context)
    validate_entry(raw, context)
    return {"year": year, "title": title, "entry": raw}


def parse_conference_records(papers: dict[str, Any]) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for year, titles in papers.items():
        year = require_year(year, "papers")
        if not isinstance(titles, dict):
            raise SystemExit(f"papers[{year!r}] must be an object")
        for title, entry in titles.items():
            title = require_title(title, f"papers[{year!r}]")
            if not isinstance(entry, dict):
                raise SystemExit(f"entry for {title!r} must be an object")
            validate_entry(entry, f"entry for {title!r}")
            records.append({"year": year, "title": title, "entry": entry.copy()})
    return records


def records_from_json(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, list):
        records = [parse_record(item, f"root[{index}]") for index, item in enumerate(value)]
    elif isinstance(value, dict) and "papers" in value:
        papers = value["papers"]
        if isinstance(papers, list):
            records = [parse_record(item, f"papers[{index}]") for index, item in enumerate(papers)]
        elif isinstance(papers, dict):
            records = parse_conference_records(papers)
        else:
            raise SystemExit("papers must be an array or an object")
    elif isinstance(value, dict):
        records = [parse_record(value, "root")]
    else:
        raise SystemExit("import JSON root must be an object or an array")

    seen: set[str] = set()
    for record in records:
        key = normalize_title_key(record["title"])
        if key in seen:
            raise SystemExit(f"duplicate title in import JSON: {record['title']}")
        seen.add(key)
    if not records:
        raise SystemExit("import JSON does not contain any paper records")
    return records


def read_import_records(args: argparse.Namespace) -> list[dict[str, Any]]:
    if (args.input is None) == (args.json is None):
        raise SystemExit("provide exactly one of --input or --json")
    if args.input is not None:
        with args.input.open("r", encoding="utf-8-sig") as handle:
            value = json.load(handle)
    else:
        value = json.loads(args.json)
    return records_from_json(value)


def title_locations(papers: dict[str, Any], title: str) -> list[tuple[str, str]]:
    locations: list[tuple[str, str]] = []
    needle = normalize_title_key(title)
    for year, titles in papers.items():
        if not isinstance(titles, dict):
            raise SystemExit(f"papers[{year!r}] must be an object")
        for title in titles:
            if normalize_title_key(title) == needle:
                locations.append((year, title))
    return locations


def title_exists(papers: dict[str, Any], title: str) -> bool:
    return bool(title_locations(papers, title))


def find_title_location(
    papers: dict[str, Any], title: str, preferred_year: str | None = None
) -> tuple[str, str] | None:
    locations = title_locations(papers, title)
    if len(locations) <= 1:
        return locations[0] if locations else None

    if preferred_year is not None:
        same_year = [location for location in locations if location[0] == preferred_year]
        if len(same_year) == 1:
            return same_year[0]

    raise SystemExit(
        f"target JSON contains multiple entries for title {title!r}; "
        "use --year for remove or clean the duplicates before import"
    )


def insert_record(papers: dict[str, Any], record: dict[str, Any]) -> None:
    papers.setdefault(record["year"], {})[record["title"]] = {
        **DEFAULT_FIELDS,
        **record["entry"],
    }


def remove_record(papers: dict[str, Any], year: str, title: str) -> None:
    year_entries = papers.get(year)
    if not isinstance(year_entries, dict):
        raise SystemExit(f"year does not exist: {year}")
    if title not in year_entries:
        raise SystemExit(f"title not found in {year}: {title}")
    del year_entries[title]
    if not year_entries:
        del papers[year]


def find_title_in_year(papers: dict[str, Any], year: str, title: str) -> str | None:
    titles = papers.get(year)
    if titles is None:
        return None
    if not isinstance(titles, dict):
        raise SystemExit(f"papers[{year!r}] must be an object")
    needle = normalize_title_key(title)
    for existing_title in titles:
        if normalize_title_key(existing_title) == needle:
            return existing_title
    return None


def apply_import_records(
    papers: dict[str, Any], records: list[dict[str, Any]], force: bool
) -> dict[str, int]:
    if not force:
        for record in records:
            if title_exists(papers, record["title"]):
                raise SystemExit(f"duplicate title already exists in target JSON: {record['title']}")

    stats = {"inserted": 0, "replaced": 0}
    for record in records:
        existing_title = find_title_in_year(papers, record["year"], record["title"])
        if existing_title is not None:
            remove_record(papers, record["year"], existing_title)
            stats["replaced"] += 1
        else:
            stats["inserted"] += 1
        insert_record(papers, record)
    return stats


def command_import(args: argparse.Namespace) -> None:
    output_path, level, conference = resolve_conference_target(args)
    records = read_import_records(args)
    document = load_document(output_path, level, conference)
    papers = document.setdefault("papers", {})
    stats = apply_import_records(papers, records, args.force)
    changed = stats["inserted"] + stats["replaced"] > 0

    backup_path = None
    if changed:
        document["papers"] = sort_years_and_titles(papers)
        backup_path = write_document(output_path, document, backup=not args.no_backup)

    print(
        "Imported "
        f"{stats['inserted']} inserted, {stats['replaced']} replaced into {output_path}"
    )
    if backup_path is not None:
        print(f"Backed up previous JSON to {backup_path}")


def command_remove(args: argparse.Namespace) -> None:
    output_path, level, conference = resolve_conference_target(args)
    if not output_path.exists():
        raise SystemExit(f"conference JSON does not exist: {output_path}")

    document = load_document(output_path, level, conference)
    papers = document.get("papers", {})
    if args.year is not None:
        year, title = args.year, args.title
    else:
        location = find_title_location(papers, args.title)
        if location is None:
            raise SystemExit(f"title not found: {args.title}")
        year, title = location

    remove_record(papers, year, title)

    backup_path = write_document(output_path, document, backup=not args.no_backup)
    print(f"Removed {args.title}")
    if backup_path is not None:
        print(f"Backed up previous JSON to {backup_path}")


def iter_conference_documents(json_dir: Path) -> list[tuple[Path, ConferenceDocument]]:
    documents: list[tuple[Path, ConferenceDocument]] = []

    for json_path in sorted(json_dir.glob("*/*.json")):
        level = json_path.parent.name
        conference = json_path.stem
        documents.append((json_path, load_document(json_path, level, conference)))

    return documents


def validate_document(path: Path, document: ConferenceDocument) -> list[str]:
    errors: list[str] = []

    if document.get("schema_version") != 1:
        errors.append(f"{path}: schema_version must be 1")
    if not isinstance(document.get("level"), str) or not document["level"].strip():
        errors.append(f"{path}: level must be a non-empty string")
    if (
        not isinstance(document.get("conference"), str)
        or not document["conference"].strip()
    ):
        errors.append(f"{path}: conference must be a non-empty string")

    papers = document.get("papers")
    if not isinstance(papers, dict):
        errors.append(f"{path}: papers must be an object")
        return errors

    for year, titles in papers.items():
        if not isinstance(year, str) or not year.isdigit() or len(year) != 4:
            errors.append(f"{path}: invalid year key {year!r}")
            continue
        if not isinstance(titles, dict):
            errors.append(f"{path}: papers[{year!r}] must be an object")
            continue
        for title, entry in titles.items():
            if not isinstance(title, str) or not title.strip():
                errors.append(f"{path}: empty title in year {year}")
            if not isinstance(entry, dict):
                errors.append(f"{path}: entry for {title!r} must be an object")
                continue
            for disallowed_key in ("auth", "source", "sources"):
                if disallowed_key in entry:
                    errors.append(
                        f"{path}: entry for {title!r} contains disallowed field {disallowed_key!r}"
                    )

    return errors


def command_validate(args: argparse.Namespace) -> None:
    errors: list[str] = []
    json_paths = sorted(args.json_dir.glob("*/*.json"))

    for path in json_paths:
        with path.open("r", encoding="utf-8-sig") as handle:
            document = json.load(handle)
        if not isinstance(document, dict):
            errors.append(f"{path}: root must be an object")
            continue
        errors.extend(validate_document(path, document))

    if errors:
        for error in errors:
            print(error)
        raise SystemExit(1)

    print(f"Validated {len(json_paths)} conference JSON files")


def command_normalize(args: argparse.Namespace) -> None:
    documents = iter_conference_documents(args.json_dir)

    for path, document in documents:
        write_document(path, document, backup=not args.no_backup)

    print(f"Normalized {len(documents)} conference JSON files")


def command_stats(args: argparse.Namespace) -> None:
    documents = iter_conference_documents(args.json_dir)
    year_count = 0
    title_count = 0

    for _, document in documents:
        papers = document.get("papers", {})
        year_count += len(papers)
        title_count += sum(len(titles) for titles in papers.values())

    print(f"conferences: {len(documents)}")
    print(f"years: {year_count}")
    print(f"title entries: {title_count}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Import, remove, and validate PaperJson data.")
    parser.add_argument(
        "--json-dir",
        default=Path("PaperJson"),
        type=Path,
        help="Directory containing per-conference JSON files. Default: PaperJson",
    )

    subparsers = parser.add_subparsers(dest="command", required=True)

    import_parser = subparsers.add_parser("import", help="Import paper records from JSON")
    import_parser.add_argument("--path", type=Path)
    import_parser.add_argument("--level")
    import_parser.add_argument("--conference")
    import_parser.add_argument("--input", type=Path)
    import_parser.add_argument("--json")
    import_parser.add_argument(
        "--force",
        action="store_true",
        help="Import even if the target JSON already has the same title.",
    )
    import_parser.add_argument(
        "--no-backup",
        action="store_true",
        help="Rewrite in place without creating a .bak file.",
    )
    import_parser.set_defaults(func=command_import)

    remove_parser = subparsers.add_parser("remove", help="Remove one paper entry")
    remove_parser.add_argument("--path", type=Path)
    remove_parser.add_argument("--level")
    remove_parser.add_argument("--conference")
    remove_parser.add_argument("--year")
    remove_parser.add_argument("--title", required=True)
    remove_parser.add_argument(
        "--no-backup",
        action="store_true",
        help="Rewrite in place without creating a .bak file.",
    )
    remove_parser.set_defaults(func=command_remove)

    validate_parser = subparsers.add_parser("validate", help="Validate JSON files")
    validate_parser.set_defaults(func=command_validate)

    normalize_parser = subparsers.add_parser(
        "normalize", help="Rewrite JSON files with the canonical schema"
    )
    normalize_parser.add_argument(
        "--no-backup",
        action="store_true",
        help="Rewrite in place without creating .bak files.",
    )
    normalize_parser.set_defaults(func=command_normalize)

    stats_parser = subparsers.add_parser("stats", help="Print dataset statistics")
    stats_parser.set_defaults(func=command_stats)

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
