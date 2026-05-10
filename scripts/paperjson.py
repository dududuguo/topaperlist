#!/usr/bin/env python3
"""Shared helpers for the PaperJson dataset."""

from __future__ import annotations

import json
import shutil
from datetime import datetime
from pathlib import Path
from typing import Any


DEFAULT_FIELDS: dict[str, Any] = {
    "author": "",
    "bib": "",
    "url": "",
}


ConferencePapers = dict[str, dict[str, dict[str, Any]]]
ConferenceDocument = dict[str, Any]


def base_document(level: str, conference: str) -> ConferenceDocument:
    return {
        "schema_version": 1,
        "level": level,
        "conference": conference,
        "papers": {},
    }


def normalize_entry(raw_entry: Any) -> dict[str, Any]:
    if isinstance(raw_entry, dict):
        entry = raw_entry.copy()
    else:
        entry = {"value": raw_entry}

    if "auth" in entry and "author" not in entry:
        entry["author"] = entry["auth"]

    entry.pop("auth", None)
    entry.pop("source", None)
    entry.pop("sources", None)

    return {**DEFAULT_FIELDS, **entry}


def normalize_papers(raw_papers: Any) -> ConferencePapers:
    if not isinstance(raw_papers, dict):
        return {}

    normalized: ConferencePapers = {}

    for year, titles in raw_papers.items():
        if not isinstance(year, str) or not isinstance(titles, dict):
            continue

        normalized_titles: dict[str, dict[str, Any]] = {}
        for title, entry in titles.items():
            if not isinstance(title, str):
                continue
            normalized_titles[title] = normalize_entry(entry)

        normalized[year] = normalized_titles

    return normalized


def sort_years_and_titles(papers: ConferencePapers) -> ConferencePapers:
    return {
        year: dict(sorted(titles.items(), key=lambda item: item[0].casefold()))
        for year, titles in sorted(papers.items(), key=lambda item: item[0])
    }


def load_document(json_path: Path, level: str, conference: str) -> ConferenceDocument:
    if not json_path.exists():
        return base_document(level, conference)

    with json_path.open("r", encoding="utf-8-sig") as handle:
        data = json.load(handle)

    if not isinstance(data, dict):
        raise ValueError(f"{json_path} must contain a JSON object")

    document = {**base_document(level, conference), **data}
    papers = normalize_papers(document.get("papers", {}))
    for key, value in data.items():
        if is_year_key(key):
            papers.setdefault(key, {})
            for title, entry in normalize_papers({key: value}).get(key, {}).items():
                papers[key].setdefault(title, entry)

    document["schema_version"] = 1
    document["level"] = level
    document["conference"] = conference
    document["papers"] = papers
    return document


def is_year_key(value: str) -> bool:
    return len(value) == 4 and value.isdigit()


def backup_existing(json_path: Path) -> Path | None:
    if not json_path.exists():
        return None

    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    backup_path = json_path.with_name(f"{json_path.name}.{timestamp}.bak")
    counter = 1
    while backup_path.exists():
        backup_path = json_path.with_name(f"{json_path.name}.{timestamp}.{counter}.bak")
        counter += 1
    shutil.copy2(json_path, backup_path)
    return backup_path


def write_document(
    json_path: Path,
    document: ConferenceDocument,
    *,
    backup: bool = True,
) -> Path | None:
    json_path.parent.mkdir(parents=True, exist_ok=True)
    backup_path = backup_existing(json_path) if backup else None
    document["papers"] = sort_years_and_titles(normalize_papers(document.get("papers", {})))

    with json_path.open("w", encoding="utf-8", newline="\n") as handle:
        json.dump(document, handle, ensure_ascii=False, indent=2)
        handle.write("\n")

    return backup_path
