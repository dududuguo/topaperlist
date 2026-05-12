#!/usr/bin/env python3
"""Enrich PaperJson entries with public OpenReview or AAAI OJS metadata.

The command is dry-run by default. Pass --apply to rewrite the target JSON file.
It only updates empty author/bib/url fields unless --overwrite is passed.
"""

from __future__ import annotations

import argparse
import html
import http.client
import json
import re
import shutil
import ssl
import sys
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, replace
from datetime import datetime
from html.parser import HTMLParser
from pathlib import Path
from typing import Any


API2_BASE_URL = "https://api2.openreview.net"
API1_BASE_URL = "https://api.openreview.net"
FORUM_BASE_URL = "https://openreview.net/forum?id="
AAAI_OJS_BASE_URL = "https://ojs.aaai.org/index.php/AAAI"
DEFAULT_CONFIG_PATH = Path("scripts/enrichment_config.json")
RETRYABLE_HTTP_CODES = {429, 500, 502, 503, 504}
RETRYABLE_NETWORK_ERRORS = (
    urllib.error.URLError,
    http.client.RemoteDisconnected,
    TimeoutError,
    ConnectionResetError,
)

VENUE_ID_TEMPLATES = {
    "AAAI": "AAAI.org/{year}/Conference",
    "ICLR": "ICLR.cc/{year}/Conference",
    "ICML": "ICML.cc/{year}/Conference",
    "NEURIPS": "NeurIPS.cc/{year}/Conference",
}


@dataclass(frozen=True)
class OpenReviewCandidate:
    note_id: str
    forum_id: str
    title: str
    authors: list[str]
    venue_id: str
    raw: dict[str, Any]

    @property
    def forum_url(self) -> str:
        return f"{FORUM_BASE_URL}{urllib.parse.quote(self.forum_id)}"


@dataclass(frozen=True)
class OjsCandidate:
    article_id: str
    title: str
    authors: list[str]
    venue_id: str
    year: str
    url: str
    pages: str
    bibtex: str
    raw: dict[str, Any]

    @property
    def note_id(self) -> str:
        return self.article_id

    @property
    def key(self) -> str:
        return self.article_id

    @property
    def forum_url(self) -> str:
        return self.url


def normalize_title(value: str) -> str:
    value = unicodedata.normalize("NFKC", value)
    value = re.sub(r"\s+", " ", value).strip()
    value = value.rstrip(".\u3002")
    return value.casefold()


def content_value(content: dict[str, Any], key: str) -> Any:
    value = content.get(key)
    if isinstance(value, dict) and "value" in value:
        return value["value"]
    return value


def require_str(value: Any, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise SystemExit(f"{name} must be a non-empty string")
    return value.strip()


def load_config(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"default": {"sources": ["openreview"]}, "conferences": {}}
    with path.open("r", encoding="utf-8") as handle:
        config = json.load(handle)
    if not isinstance(config, dict):
        raise SystemExit(f"{path} must contain a JSON object")
    return config


def conference_config(config: dict[str, Any], conference: str) -> dict[str, Any]:
    default = config.get("default", {})
    conferences = config.get("conferences", {})
    merged: dict[str, Any] = default.copy() if isinstance(default, dict) else {}
    if isinstance(conferences, dict):
        specific = conferences.get(conference.upper()) or conferences.get(conference)
        if isinstance(specific, dict):
            for key, value in specific.items():
                if isinstance(value, dict) and isinstance(merged.get(key), dict):
                    nested = merged[key].copy()
                    nested.update(value)
                    merged[key] = nested
                else:
                    merged[key] = value
    return merged


def source_order(args: argparse.Namespace, conf_config: dict[str, Any]) -> list[str]:
    if args.source != "auto":
        return [args.source]
    sources = conf_config.get("sources", ["openreview"])
    if isinstance(sources, str):
        sources = [sources]
    if not isinstance(sources, list) or not sources:
        raise SystemExit("config sources must be a non-empty string or list")
    normalized = [str(source).strip().lower() for source in sources if str(source).strip()]
    for source in normalized:
        if source not in {"openreview", "aaai_ojs"}:
            raise SystemExit(f"unsupported source in config: {source}")
    return normalized


def json_request(
    method: str,
    url: str,
    *,
    params: dict[str, Any] | None = None,
    body: dict[str, Any] | None = None,
    timeout: int = 30,
) -> dict[str, Any]:
    if params:
        query = urllib.parse.urlencode(params, doseq=True)
        url = f"{url}?{query}"

    data = None
    headers = {"Accept": "application/json", "User-Agent": "topaperlist-openreview-enrich"}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"

    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            payload = response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"OpenReview request failed: HTTP {error.code} {url}\n{detail}") from error
    except urllib.error.URLError as error:
        raise RuntimeError(f"OpenReview request failed: {url}\n{error}") from error

    parsed = json.loads(payload)
    if not isinstance(parsed, dict):
        raise RuntimeError(f"OpenReview returned a non-object response from {url}")
    return parsed


def note_to_candidate(note: dict[str, Any]) -> OpenReviewCandidate | None:
    content = note.get("content")
    if not isinstance(content, dict):
        return None

    title = content_value(content, "title")
    if not isinstance(title, str) or not title.strip():
        return None

    raw_authors = content_value(content, "authors")
    authors: list[str] = []
    if isinstance(raw_authors, list):
        authors = [str(author).strip() for author in raw_authors if str(author).strip()]
    elif isinstance(raw_authors, str) and raw_authors.strip():
        authors = [part.strip() for part in raw_authors.split(",") if part.strip()]

    note_id = str(note.get("id") or "")
    forum_id = str(note.get("forum") or note_id)
    venue_id = content_value(content, "venueid")
    if not isinstance(venue_id, str):
        venue_id = ""

    return OpenReviewCandidate(
        note_id=note_id,
        forum_id=forum_id,
        title=title.strip(),
        authors=authors,
        venue_id=venue_id,
        raw=note,
    )


def dedupe_candidates(candidates: list[OpenReviewCandidate]) -> list[OpenReviewCandidate]:
    seen: set[tuple[str, str]] = set()
    deduped: list[OpenReviewCandidate] = []
    for candidate in candidates:
        key = (candidate.note_id, normalize_title(candidate.title))
        if key in seen:
            continue
        seen.add(key)
        deduped.append(candidate)
    return deduped


def fetch_notes_by_title_search(
    base_url: str,
    venue_id: str,
    title: str,
    *,
    limit: int,
    timeout: int,
) -> list[OpenReviewCandidate]:
    url = f"{base_url.rstrip('/')}/notes/search"
    attempts = [
        (
            "POST",
            {
                "term": title,
                "content": {
                    "title": {
                        "terms": [title],
                        "matchMethod": "match",
                    }
                },
                "venueid": venue_id,
                "source": "forum",
                "limit": limit,
                "offset": 0,
            },
            None,
        ),
        (
            "GET",
            None,
            {
                "term": title,
                "content": "title",
                "group": venue_id,
                "source": "forum",
                "limit": limit,
                "offset": 0,
            },
        ),
        (
            "POST",
            {
                "term": title,
                "content": {
                    "title": {
                        "terms": [title],
                        "matchMethod": "match",
                    }
                },
                "group": venue_id,
                "source": "forum",
                "limit": limit,
                "offset": 0,
            },
            None,
        ),
    ]

    candidates: list[OpenReviewCandidate] = []
    errors: list[str] = []
    for method, body, params in attempts:
        try:
            data = json_request(method, url, params=params, body=body, timeout=timeout)
        except RuntimeError as error:
            errors.append(str(error))
            continue
        for note in data.get("notes", []):
            if isinstance(note, dict):
                candidate = note_to_candidate(note)
                if candidate is not None:
                    candidates.append(candidate)
        if candidates:
            break

    if not candidates and errors:
        print(errors[-1], file=sys.stderr)
    return dedupe_candidates(candidates)


def fetch_note_by_id(base_url: str, note_id: str, *, timeout: int) -> OpenReviewCandidate | None:
    data = json_request(
        "GET",
        f"{base_url.rstrip('/')}/notes",
        params={"id": note_id},
        timeout=timeout,
    )
    notes = data.get("notes", [])
    if not isinstance(notes, list) or not notes:
        return None
    if not isinstance(notes[0], dict):
        return None
    return note_to_candidate(notes[0])


def fallback_base_urls(base_url: str) -> list[str]:
    normalized = base_url.rstrip("/")
    fallbacks = [normalized]
    if normalized == API2_BASE_URL:
        fallbacks.append(API1_BASE_URL)
    return fallbacks


def fetch_notes_by_venue(
    base_url: str,
    venue_id: str,
    *,
    page_size: int,
    max_notes: int,
    timeout: int,
    sleep_seconds: float,
) -> list[OpenReviewCandidate]:
    notes_url = f"{base_url.rstrip('/')}/notes"
    candidates: list[OpenReviewCandidate] = []
    offset = 0

    while True:
        try:
            data = json_request(
                "GET",
                notes_url,
                params={
                    "content.venueid": venue_id,
                    "limit": page_size,
                    "offset": offset,
                },
                timeout=timeout,
            )
        except RuntimeError as error:
            print(error, file=sys.stderr)
            break

        notes = data.get("notes", [])
        if not isinstance(notes, list) or not notes:
            break

        for note in notes:
            if isinstance(note, dict):
                candidate = note_to_candidate(note)
                if candidate is not None:
                    candidates.append(candidate)

        offset += len(notes)
        count = data.get("count")
        if len(candidates) >= max_notes:
            break
        if isinstance(count, int) and offset >= count:
            break
        if len(notes) < page_size:
            break
        if sleep_seconds > 0:
            time.sleep(sleep_seconds)

    return dedupe_candidates(candidates[:max_notes])


def fetch_notes_by_invitation(
    base_url: str,
    invitation: str,
    *,
    page_size: int,
    max_notes: int,
    timeout: int,
    sleep_seconds: float,
) -> list[OpenReviewCandidate]:
    notes_url = f"{base_url.rstrip('/')}/notes"
    candidates: list[OpenReviewCandidate] = []
    offset = 0

    while True:
        data = json_request(
            "GET",
            notes_url,
            params={
                "invitation": invitation,
                "limit": page_size,
                "offset": offset,
            },
            timeout=timeout,
        )

        notes = data.get("notes", [])
        if not isinstance(notes, list) or not notes:
            break

        for note in notes:
            if isinstance(note, dict):
                candidate = note_to_candidate(note)
                if candidate is not None:
                    candidates.append(candidate)

        offset += len(notes)
        count = data.get("count")
        if len(candidates) >= max_notes:
            break
        if isinstance(count, int) and offset >= count:
            break
        if len(notes) < page_size:
            break
        if sleep_seconds > 0:
            time.sleep(sleep_seconds)

    return dedupe_candidates(candidates[:max_notes])


def unique_values(values: list[str]) -> list[str]:
    seen: set[str] = set()
    unique: list[str] = []
    for value in values:
        key = value.casefold()
        if value and key not in seen:
            seen.add(key)
            unique.append(value)
    return unique


def clean_html_text(value: str) -> str:
    return re.sub(r"\s+", " ", html.unescape(value)).strip()


def attrs_to_dict(attrs: list[tuple[str, str | None]]) -> dict[str, str]:
    return {name: value or "" for name, value in attrs}


def has_css_class(attrs: dict[str, str], class_name: str) -> bool:
    return class_name in attrs.get("class", "").split()


def ojs_text_request(
    url: str,
    *,
    timeout: int,
    verify_tls: bool,
    accept: str = "text/html",
    retries: int = 2,
) -> str:
    headers = {"Accept": accept, "User-Agent": "topaperlist-metadata-enrich"}
    context = None if verify_tls else ssl._create_unverified_context()
    for attempt in range(retries + 1):
        request = urllib.request.Request(url, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=timeout, context=context) as response:
                return response.read().decode("utf-8", errors="replace")
        except urllib.error.HTTPError as error:
            if error.code in RETRYABLE_HTTP_CODES and attempt < retries:
                retry_after = error.headers.get("Retry-After")
                wait_seconds = (
                    int(retry_after)
                    if retry_after and retry_after.isdigit()
                    else min(10, 2 ** attempt)
                )
                time.sleep(wait_seconds)
                continue
            detail = error.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"AAAI OJS request failed: HTTP {error.code} {url}\n{detail}") from error
        except RETRYABLE_NETWORK_ERRORS as error:
            if attempt < retries:
                time.sleep(min(10, 2 ** attempt))
                continue
            raise RuntimeError(f"AAAI OJS request failed: {url}\n{error}") from error
    raise RuntimeError(f"AAAI OJS request failed after retries: {url}")


class OjsLinkParser(HTMLParser):
    def __init__(self, base_url: str):
        super().__init__(convert_charrefs=True)
        self.base_url = base_url
        self.links: list[dict[str, str]] = []
        self.current: dict[str, Any] | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag != "a" or self.current is not None:
            return
        attrs_dict = attrs_to_dict(attrs)
        href = attrs_dict.get("href", "").strip()
        if href:
            self.current = {
                "href": urllib.parse.urljoin(self.base_url, html.unescape(href)),
                "text_parts": [],
            }

    def handle_data(self, data: str) -> None:
        if self.current is not None:
            self.current["text_parts"].append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag != "a" or self.current is None:
            return
        self.links.append(
            {
                "href": self.current["href"],
                "text": clean_html_text(" ".join(self.current["text_parts"])),
            }
        )
        self.current = None


class OjsArticleParser(HTMLParser):
    def __init__(self, base_url: str, year: str, issue_url: str):
        super().__init__(convert_charrefs=True)
        self.base_url = base_url
        self.year = year
        self.issue_url = issue_url
        self.articles: list[OjsCandidate] = []
        self.current: dict[str, Any] | None = None
        self.depth = 0
        self.capture: str | None = None
        self.capture_parts: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attrs_dict = attrs_to_dict(attrs)
        if (
            self.current is None
            and tag == "div"
            and has_css_class(attrs_dict, "obj_article_summary")
        ):
            self.current = {
                "article_id": "",
                "title": "",
                "authors": "",
                "pages": "",
                "url": "",
            }
            self.depth = 1
            return

        if self.current is None:
            return

        self.depth += 1
        if tag == "a" and not self.current["title"]:
            href = attrs_dict.get("href", "").strip()
            full_url = urllib.parse.urljoin(self.base_url, html.unescape(href))
            parsed = urllib.parse.urlparse(full_url)
            match = re.search(r"/article/view/(\d+)$", parsed.path)
            if match:
                self.current["article_id"] = match.group(1)
                self.current["url"] = full_url
                self.capture = "title"
                self.capture_parts = []
        elif tag == "div" and has_css_class(attrs_dict, "authors"):
            self.capture = "authors"
            self.capture_parts = []
        elif tag == "div" and has_css_class(attrs_dict, "pages"):
            self.capture = "pages"
            self.capture_parts = []

    def handle_data(self, data: str) -> None:
        if self.current is not None and self.capture is not None:
            self.capture_parts.append(data)

    def handle_endtag(self, tag: str) -> None:
        if self.current is None:
            return

        if self.capture == "title" and tag == "a":
            self.current["title"] = clean_html_text(" ".join(self.capture_parts))
            self.capture = None
        elif self.capture in {"authors", "pages"} and tag == "div":
            self.current[self.capture] = clean_html_text(" ".join(self.capture_parts))
            self.capture = None

        self.depth -= 1
        if self.depth == 0:
            title = self.current["title"]
            url = self.current["url"]
            article_id = self.current["article_id"]
            if title and url and article_id:
                authors = [
                    author.strip()
                    for author in self.current["authors"].split(",")
                    if author.strip()
                ]
                self.articles.append(
                    OjsCandidate(
                        article_id=article_id,
                        title=title,
                        authors=authors,
                        venue_id="AAAI OJS",
                        year=self.year,
                        url=url,
                        pages=self.current["pages"],
                        bibtex="",
                        raw={"issue_url": self.issue_url, "pages": self.current["pages"]},
                    )
                )
            self.current = None
            self.capture = None
            self.capture_parts = []


def parse_ojs_links(payload: str, base_url: str) -> list[dict[str, str]]:
    parser = OjsLinkParser(base_url)
    parser.feed(payload)
    return parser.links


def parse_ojs_issue_articles(payload: str, issue_url: str, year: str) -> list[OjsCandidate]:
    parser = OjsArticleParser(issue_url, year, issue_url)
    parser.feed(payload)
    return parser.articles


def ojs_base_url(source_config: dict[str, Any]) -> str:
    return str(source_config.get("base_url", AAAI_OJS_BASE_URL)).rstrip("/")


def ojs_archive_url(source_config: dict[str, Any]) -> str:
    configured = source_config.get("archive_url")
    if isinstance(configured, str) and configured.strip():
        return configured.strip()
    return f"{ojs_base_url(source_config)}/issue/archive"


def configured_ojs_issue_urls(source_config: dict[str, Any], year: str) -> list[str]:
    issue_urls = source_config.get("issue_urls")
    if isinstance(issue_urls, dict):
        value = issue_urls.get(year)
        if isinstance(value, str) and value.strip():
            return [value.strip()]
        if isinstance(value, list):
            return [str(url).strip() for url in value if str(url).strip()]
    return []


def fetch_ojs_issue_urls_for_year(
    year: str,
    source_config: dict[str, Any],
    *,
    timeout: int,
    verify_tls: bool,
) -> list[str]:
    configured = configured_ojs_issue_urls(source_config, year)
    if configured:
        return unique_values(configured)

    target = f"AAAI-{year[-2:]}"
    max_archive_pages = int(source_config.get("max_archive_pages", 20))
    next_url: str | None = ojs_archive_url(source_config)
    visited: set[str] = set()
    issue_urls: list[str] = []

    for _ in range(max_archive_pages):
        if next_url is None or next_url in visited:
            break
        visited.add(next_url)
        payload = ojs_text_request(
            next_url,
            timeout=timeout,
            verify_tls=verify_tls,
        )
        links = parse_ojs_links(payload, next_url)
        for link in links:
            href = link["href"]
            text = link["text"]
            if "/issue/view/" in href and target in text:
                issue_urls.append(href)

        next_candidates = [
            link["href"]
            for link in links
            if link["text"].casefold() == "next" and "/issue/archive" in link["href"]
        ]
        next_url = next_candidates[0] if next_candidates else None

    return unique_values(issue_urls)


def fetch_ojs_bibtex(
    candidate: OjsCandidate,
    *,
    timeout: int,
    verify_tls: bool,
) -> str:
    payload = ojs_text_request(
        candidate.url,
        timeout=timeout,
        verify_tls=verify_tls,
    )
    links = parse_ojs_links(payload, candidate.url)
    bibtex_urls = [
        link["href"]
        for link in links
        if "/citationstylelanguage/download/bibtex" in link["href"]
    ]
    if not bibtex_urls:
        return ""
    return ojs_text_request(
        bibtex_urls[0],
        timeout=timeout,
        verify_tls=verify_tls,
        accept="text/plain",
    ).strip()


def ensure_ojs_bibtex(
    candidate: OjsCandidate,
    *,
    timeout: int,
    verify_tls: bool,
) -> tuple[OjsCandidate, bool]:
    if candidate.bibtex.strip():
        return candidate, False
    bibtex = fetch_ojs_bibtex(candidate, timeout=timeout, verify_tls=verify_tls)
    if not bibtex:
        return candidate, False
    return replace(candidate, bibtex=bibtex), True


def fetch_ojs_candidates_for_year(
    year: str,
    source_config: dict[str, Any],
    *,
    timeout: int,
    verify_tls: bool,
    sleep_seconds: float,
) -> tuple[list[OjsCandidate], list[str], list[str]]:
    issue_urls = fetch_ojs_issue_urls_for_year(
        year,
        source_config,
        timeout=timeout,
        verify_tls=verify_tls,
    )
    candidates: list[OjsCandidate] = []
    errors: list[str] = []
    for issue_url in issue_urls:
        try:
            payload = ojs_text_request(
                issue_url,
                timeout=timeout,
                verify_tls=verify_tls,
            )
        except RuntimeError as error:
            errors.append(str(error))
            continue
        candidates.extend(parse_ojs_issue_articles(payload, issue_url, year))
        if sleep_seconds > 0:
            time.sleep(sleep_seconds)

    seen: set[tuple[str, str]] = set()
    deduped: list[OjsCandidate] = []
    for candidate in candidates:
        key = (candidate.article_id, normalize_title(candidate.title))
        if key in seen:
            continue
        seen.add(key)
        deduped.append(candidate)
    return deduped, issue_urls, errors


def select_ojs_candidate(
    candidates: list[OjsCandidate],
    title: str,
    year: str,
) -> OjsCandidate | None:
    title_key = normalize_title(title)
    exact = [
        candidate
        for candidate in candidates
        if normalize_title(candidate.title) == title_key and candidate.year == year
    ]
    if len(exact) == 1:
        return exact[0]
    if len(exact) > 1:
        keys = ", ".join(candidate.article_id for candidate in exact)
        raise SystemExit(f"multiple exact AAAI OJS matches found for {title!r}: {keys}")
    return None


def infer_json_path(data_dir: Path, level: str, conference: str) -> Path:
    return data_dir / level / f"{conference}.json"


def infer_venue_id(conference: str, year: str) -> str:
    template = VENUE_ID_TEMPLATES.get(conference.upper())
    if template is None:
        raise SystemExit(
            f"cannot infer OpenReview venue id for {conference}; pass --venue-id explicitly"
        )
    return template.format(year=year)


def load_conference_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8-sig") as handle:
        document = json.load(handle)
    if not isinstance(document, dict):
        raise SystemExit(f"{path} must contain a JSON object")
    return document


def find_entry(document: dict[str, Any], year: str, title: str) -> tuple[str, dict[str, Any]]:
    papers = document.get("papers")
    if not isinstance(papers, dict):
        raise SystemExit("target JSON does not contain a papers object")
    year_entries = papers.get(year)
    if not isinstance(year_entries, dict):
        raise SystemExit(f"year not found in target JSON: {year}")

    target_key = normalize_title(title)
    matches = [
        (existing_title, entry)
        for existing_title, entry in year_entries.items()
        if normalize_title(existing_title) == target_key
    ]
    if not matches:
        raise SystemExit(f"title not found in {year}: {title}")
    if len(matches) > 1:
        raise SystemExit(f"title is ambiguous in {year}: {title}")

    existing_title, entry = matches[0]
    if not isinstance(entry, dict):
        raise SystemExit(f"entry for {existing_title!r} must be an object")
    return existing_title, entry


def make_bibtex(candidate: OpenReviewCandidate, conference: str, year: str) -> str:
    candidate_bibtex = getattr(candidate, "bibtex", "")
    if isinstance(candidate_bibtex, str) and candidate_bibtex.strip():
        return candidate_bibtex.strip()

    for key in ("bibtex", "_bibtex"):
        raw_bibtex = content_value(candidate.raw.get("content", {}), key)
        if isinstance(raw_bibtex, str) and raw_bibtex.strip():
            return raw_bibtex.strip()

    first_author = candidate.authors[0].split()[-1] if candidate.authors else "openreview"
    key_seed = f"{first_author}{year}{candidate.title.split()[0] if candidate.title.split() else 'paper'}"
    bib_key = re.sub(r"[^A-Za-z0-9_:-]+", "", key_seed) or f"openreview{year}"
    author = " and ".join(candidate.authors)
    lines = [
        f"@inproceedings{{{bib_key},",
        f"  title = {{{candidate.title}}},",
    ]
    if author:
        lines.append(f"  author = {{{author}}},")
    lines.extend(
        [
            f"  booktitle = {{{conference} {year}}},",
            f"  year = {{{year}}},",
            f"  url = {{{candidate.forum_url}}}",
            "}",
        ]
    )
    return "\n".join(lines)


def backup_file(path: Path) -> Path:
    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    backup_path = path.with_name(f"{path.name}.{timestamp}.bak")
    shutil.copy2(path, backup_path)
    return backup_path


def write_conference_json(path: Path, document: dict[str, Any], *, backup: bool) -> Path | None:
    backup_path = backup_file(path) if backup and path.exists() else None
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        json.dump(document, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    return backup_path


def select_exact_candidate(
    candidates: list[OpenReviewCandidate],
    venue_id: str,
    title: str,
    *,
    allow_venue_mismatch: bool,
) -> OpenReviewCandidate | None:
    title_key = normalize_title(title)
    exact = [candidate for candidate in candidates if normalize_title(candidate.title) == title_key]
    if not allow_venue_mismatch:
        exact = [
            candidate
            for candidate in exact
            if not candidate.venue_id or candidate.venue_id == venue_id
        ]
    if len(exact) == 1:
        return exact[0]
    if len(exact) > 1:
        raise SystemExit(
            "multiple exact OpenReview matches found; rerun with a more specific --title "
            "or inspect with --dump-candidates"
        )
    return None


def parse_year_list(value: str) -> list[str]:
    years = [part.strip() for part in value.split(",") if part.strip()]
    for year in years:
        if not year.isdigit() or len(year) != 4:
            raise SystemExit(f"invalid year in --years: {year}")
    return list(dict.fromkeys(years))


def candidate_index(
    candidates: list[OpenReviewCandidate],
    venue_id: str,
    *,
    allow_venue_mismatch: bool,
) -> tuple[dict[str, OpenReviewCandidate], set[str]]:
    grouped: dict[str, list[OpenReviewCandidate]] = {}
    for candidate in candidates:
        if not allow_venue_mismatch and candidate.venue_id and candidate.venue_id != venue_id:
            continue
        grouped.setdefault(normalize_title(candidate.title), []).append(candidate)

    unique: dict[str, OpenReviewCandidate] = {}
    ambiguous: set[str] = set()
    for title_key, items in grouped.items():
        note_ids = {item.note_id for item in items}
        if len(note_ids) == 1:
            unique[title_key] = items[0]
        else:
            ambiguous.add(title_key)
    return unique, ambiguous


def local_year_entries(
    document: dict[str, Any], year: str
) -> tuple[dict[str, tuple[str, dict[str, Any]]], set[str]]:
    papers = document.get("papers")
    if not isinstance(papers, dict):
        raise SystemExit("target JSON does not contain a papers object")
    year_entries = papers.get(year)
    if not isinstance(year_entries, dict):
        return {}, set()

    grouped: dict[str, list[tuple[str, dict[str, Any]]]] = {}
    for title, entry in year_entries.items():
        if isinstance(title, str) and isinstance(entry, dict):
            grouped.setdefault(normalize_title(title), []).append((title, entry))

    unique: dict[str, tuple[str, dict[str, Any]]] = {}
    ambiguous: set[str] = set()
    for title_key, items in grouped.items():
        if len(items) == 1:
            unique[title_key] = items[0]
        else:
            ambiguous.add(title_key)
    return unique, ambiguous


def entry_changes(
    entry: dict[str, Any],
    candidate: OpenReviewCandidate,
    conference: str,
    year: str,
    *,
    overwrite: bool,
) -> dict[str, str]:
    updates = {
        "author": ", ".join(candidate.authors),
        "url": candidate.forum_url,
        "bib": make_bibtex(candidate, conference, year),
    }
    changed: dict[str, str] = {}
    for field, value in updates.items():
        if not value:
            continue
        if overwrite or not str(entry.get(field, "")).strip():
            if entry.get(field) != value:
                changed[field] = value
    return changed


def fetch_notes_by_venue_with_fallback(
    base_url: str,
    venue_id: str,
    *,
    page_size: int,
    max_notes: int,
    timeout: int,
    sleep_seconds: float,
) -> tuple[list[OpenReviewCandidate], str | None]:
    errors: list[str] = []
    for candidate_base_url in fallback_base_urls(base_url):
        try:
            candidates = fetch_notes_by_venue(
                candidate_base_url,
                venue_id,
                page_size=page_size,
                max_notes=max_notes,
                timeout=timeout,
                sleep_seconds=sleep_seconds,
            )
        except RuntimeError as error:
            errors.append(str(error))
            continue
        if candidates:
            return candidates, candidate_base_url
    for error in errors:
        print(error, file=sys.stderr)
    return [], None


def configured_legacy_invitation(source_config: dict[str, Any], year: str) -> str | None:
    invitations = source_config.get("legacy_invitations")
    if isinstance(invitations, dict):
        value = invitations.get(year)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def fetch_notes_by_legacy_invitation_with_fallback(
    base_url: str,
    invitation: str,
    *,
    page_size: int,
    max_notes: int,
    timeout: int,
    sleep_seconds: float,
) -> tuple[list[OpenReviewCandidate], str | None]:
    errors: list[str] = []
    for candidate_base_url in fallback_base_urls(base_url):
        try:
            candidates = fetch_notes_by_invitation(
                candidate_base_url,
                invitation,
                page_size=page_size,
                max_notes=max_notes,
                timeout=timeout,
                sleep_seconds=sleep_seconds,
            )
        except RuntimeError as error:
            errors.append(str(error))
            continue
        if candidates:
            return candidates, candidate_base_url
    for error in errors:
        print(error, file=sys.stderr)
    return [], None


def find_openreview_candidate(
    args: argparse.Namespace,
    title: str,
    venue_id: str,
) -> tuple[OpenReviewCandidate | None, list[OpenReviewCandidate]]:
    candidates: list[OpenReviewCandidate] = []
    candidate = None

    if args.forum_id:
        forum_errors: list[str] = []
        for base_url in fallback_base_urls(args.base_url):
            try:
                fetched = fetch_note_by_id(base_url, args.forum_id, timeout=args.timeout)
            except RuntimeError as error:
                forum_errors.append(str(error))
                continue
            if fetched is None:
                continue
            candidates.append(fetched)
            candidate = select_exact_candidate(
                candidates,
                venue_id,
                title,
                allow_venue_mismatch=args.allow_venue_mismatch,
            )
            if candidate is not None:
                break
        if candidate is None:
            for error in forum_errors:
                print(error, file=sys.stderr)

    if candidate is None:
        for base_url in fallback_base_urls(args.base_url):
            candidates = dedupe_candidates(
                candidates
                + fetch_notes_by_title_search(
                    base_url,
                    venue_id,
                    title,
                    limit=args.limit,
                    timeout=args.timeout,
                )
            )
            candidate = select_exact_candidate(
                candidates,
                venue_id,
                title,
                allow_venue_mismatch=args.allow_venue_mismatch,
            )
            if candidate is not None:
                break

    if candidate is None and args.scan_venue:
        venue_candidates = fetch_notes_by_venue(
            args.base_url,
            venue_id,
            page_size=args.page_size,
            max_notes=args.max_notes,
            timeout=args.timeout,
            sleep_seconds=args.sleep,
        )
        candidates = dedupe_candidates(candidates + venue_candidates)
        candidate = select_exact_candidate(
            candidates,
            venue_id,
            title,
            allow_venue_mismatch=args.allow_venue_mismatch,
        )

    return candidate, candidates


def find_ojs_candidate(
    args: argparse.Namespace,
    title: str,
    year: str,
    source_config: dict[str, Any],
) -> tuple[OjsCandidate | None, list[OjsCandidate]]:
    candidates, _, errors = fetch_ojs_candidates_for_year(
        year,
        source_config,
        timeout=args.timeout,
        verify_tls=not args.insecure,
        sleep_seconds=float(source_config.get("sleep", args.sleep)),
    )
    for error in errors:
        print(error, file=sys.stderr)
    candidate = select_ojs_candidate(candidates, title, year)
    if candidate is not None and bool(source_config.get("fetch_bib", True)):
        try:
            candidate, _ = ensure_ojs_bibtex(
                candidate,
                timeout=args.timeout,
                verify_tls=not args.insecure,
            )
        except RuntimeError as error:
            print(f"{error}\nUsing synthesized BibTeX for this title.", file=sys.stderr)
    return candidate, candidates


def available_years(document: dict[str, Any]) -> list[str]:
    papers = document.get("papers")
    if not isinstance(papers, dict):
        raise SystemExit("target JSON does not contain a papers object")
    return sorted(year for year in papers if isinstance(year, str) and year.isdigit() and len(year) == 4)


def run_openreview_batch(
    args: argparse.Namespace,
    document: dict[str, Any],
    json_path: Path,
    conference: str,
    source_config: dict[str, Any],
) -> None:
    years = parse_year_list(args.years) if args.years else available_years(document)
    total_local = 0
    total_candidates = 0
    total_matched = 0
    total_entries_changed = 0
    total_fields_changed = 0
    total_missing = 0
    total_local_ambiguous = 0
    total_openreview_ambiguous = 0

    print(f"Target JSON: {json_path}")
    print(f"Conference: {conference}")
    print(f"Years: {', '.join(years)}")

    for year in years:
        venue_template = args.venue_id or source_config.get("venue_id")
        venue_id = str(venue_template).format(year=year) if venue_template else infer_venue_id(conference, year)
        local_entries, local_ambiguous = local_year_entries(document, year)
        invitation = configured_legacy_invitation(source_config, year)
        if invitation is not None:
            candidates, used_base_url = fetch_notes_by_legacy_invitation_with_fallback(
                args.base_url,
                invitation,
                page_size=args.page_size,
                max_notes=args.max_notes,
                timeout=args.timeout,
                sleep_seconds=args.sleep,
            )
            source_label = f"{used_base_url or 'none'} invitation={invitation}"
        else:
            candidates, used_base_url = fetch_notes_by_venue_with_fallback(
                args.base_url,
                venue_id,
                page_size=args.page_size,
                max_notes=args.max_notes,
                timeout=args.timeout,
                sleep_seconds=args.sleep,
            )
            source_label = used_base_url or "none"
        openreview_entries, openreview_ambiguous = candidate_index(
            candidates,
            venue_id,
            allow_venue_mismatch=args.allow_venue_mismatch,
        )

        matched = sorted(set(local_entries) & set(openreview_entries))
        missing = len(local_entries) - len(matched)
        entries_changed = 0
        fields_changed = 0

        for title_key in matched:
            _, entry = local_entries[title_key]
            candidate = openreview_entries[title_key]
            changed = entry_changes(
                entry,
                candidate,
                conference,
                year,
                overwrite=args.overwrite,
            )
            if changed:
                entries_changed += 1
                fields_changed += len(changed)
                if args.apply:
                    entry.update(changed)

        total_local += len(local_entries)
        total_candidates += len(candidates)
        total_matched += len(matched)
        total_entries_changed += entries_changed
        total_fields_changed += fields_changed
        total_missing += missing
        total_local_ambiguous += len(local_ambiguous)
        total_openreview_ambiguous += len(openreview_ambiguous)

        print(
            f"{year}: local={len(local_entries)} openreview={len(candidates)} "
            f"matched={len(matched)} changed_entries={entries_changed} "
            f"changed_fields={fields_changed} missing={missing} source={source_label}"
        )
        if local_ambiguous:
            print(f"{year}: skipped {len(local_ambiguous)} ambiguous local title keys")
        if openreview_ambiguous:
            print(f"{year}: skipped {len(openreview_ambiguous)} ambiguous OpenReview title keys")

    print(
        "Summary: "
        f"local={total_local} openreview={total_candidates} matched={total_matched} "
        f"changed_entries={total_entries_changed} changed_fields={total_fields_changed} "
        f"missing={total_missing} local_ambiguous={total_local_ambiguous} "
        f"openreview_ambiguous={total_openreview_ambiguous}"
    )

    if not args.apply:
        print("Dry-run only. Re-run with --apply to write the JSON file.")
        return
    if total_fields_changed == 0:
        print("No fields need updating.")
        return

    backup_path = write_conference_json(json_path, document, backup=not args.no_backup)
    print(f"Updated {json_path}")
    if backup_path is not None:
        print(f"Backup written to {backup_path}")


def run_ojs_batch(
    args: argparse.Namespace,
    document: dict[str, Any],
    json_path: Path,
    conference: str,
    source_config: dict[str, Any],
) -> None:
    years = parse_year_list(args.years) if args.years else available_years(document)
    sleep_seconds = float(source_config.get("sleep", args.sleep))
    max_records = args.max_records
    processed = 0
    total_local = 0
    total_candidates = 0
    total_matched = 0
    total_entries_changed = 0
    total_fields_changed = 0
    total_missing = 0
    total_local_ambiguous = 0
    total_ojs_ambiguous = 0
    total_errors = 0

    print(f"Target JSON: {json_path}")
    print(f"Conference: {conference}")
    print("Source: AAAI OJS")
    print(f"Years: {', '.join(years)}")

    for year in years:
        local_entries, local_ambiguous = local_year_entries(document, year)
        try:
            candidates, issue_urls, issue_errors = fetch_ojs_candidates_for_year(
                year,
                source_config,
                timeout=args.timeout,
                verify_tls=not args.insecure,
                sleep_seconds=sleep_seconds,
            )
        except RuntimeError as error:
            print(f"{error}\nSkipping {year}.", file=sys.stderr)
            total_errors += 1
            continue

        for issue_error in issue_errors:
            print(issue_error, file=sys.stderr)

        ojs_entries, ojs_ambiguous = candidate_index(
            candidates,
            "AAAI OJS",
            allow_venue_mismatch=args.allow_venue_mismatch,
        )
        matched = sorted(
            set(local_entries) & set(ojs_entries),
            key=lambda title_key: local_entries[title_key][0].casefold(),
        )
        missing = len(local_entries) - len(matched)
        entries_changed = 0
        fields_changed = 0
        errors = len(issue_errors)

        for title_key in matched:
            if max_records is not None and processed >= max_records:
                break
            local_title, entry = local_entries[title_key]
            candidate = ojs_entries[title_key]

            needs_bib = args.overwrite or not str(entry.get("bib", "")).strip()
            if bool(source_config.get("fetch_bib", True)) and needs_bib:
                try:
                    candidate, fetched_bib = ensure_ojs_bibtex(
                        candidate,
                        timeout=args.timeout,
                        verify_tls=not args.insecure,
                    )
                    if fetched_bib and sleep_seconds > 0:
                        time.sleep(sleep_seconds)
                except RuntimeError as error:
                    print(
                        f"AAAI OJS BibTeX fetch failed for {local_title!r}: {error}\n"
                        "Using synthesized BibTeX for this title.",
                        file=sys.stderr,
                    )
                    errors += 1

            changed = entry_changes(
                entry,
                candidate,
                conference,
                year,
                overwrite=args.overwrite,
            )
            processed += 1
            if changed:
                entries_changed += 1
                fields_changed += len(changed)
                if args.apply:
                    entry.update(changed)

        total_local += len(local_entries)
        total_candidates += len(candidates)
        total_matched += len(matched)
        total_entries_changed += entries_changed
        total_fields_changed += fields_changed
        total_missing += missing
        total_local_ambiguous += len(local_ambiguous)
        total_ojs_ambiguous += len(ojs_ambiguous)
        total_errors += errors

        print(
            f"{year}: local={len(local_entries)} issues={len(issue_urls)} "
            f"ojs={len(candidates)} matched={len(matched)} "
            f"changed_entries={entries_changed} changed_fields={fields_changed} "
            f"missing={missing} errors={errors}"
        )
        if local_ambiguous:
            print(f"{year}: skipped {len(local_ambiguous)} ambiguous local title keys")
        if ojs_ambiguous:
            print(f"{year}: skipped {len(ojs_ambiguous)} ambiguous AAAI OJS title keys")
        if max_records is not None and processed >= max_records:
            break

    print(
        "Summary: "
        f"local={total_local} ojs={total_candidates} matched={total_matched} "
        f"changed_entries={total_entries_changed} changed_fields={total_fields_changed} "
        f"missing={total_missing} local_ambiguous={total_local_ambiguous} "
        f"ojs_ambiguous={total_ojs_ambiguous} errors={total_errors}"
    )

    if not args.apply:
        print("Dry-run only. Re-run with --apply to write the JSON file.")
        return
    if total_fields_changed == 0:
        print("No fields need updating.")
        return

    backup_path = write_conference_json(json_path, document, backup=not args.no_backup)
    print(f"Updated {json_path}")
    if backup_path is not None:
        print(f"Backup written to {backup_path}")


def run_batch(
    args: argparse.Namespace,
    document: dict[str, Any],
    json_path: Path,
    conference: str,
    conf_config: dict[str, Any],
) -> None:
    sources = source_order(args, conf_config)
    if len(sources) != 1:
        raise SystemExit("--all currently requires exactly one configured source")
    source = sources[0]
    if source == "openreview":
        openreview_config = conf_config.get("openreview", {})
        if not isinstance(openreview_config, dict):
            openreview_config = {}
        run_openreview_batch(args, document, json_path, conference, openreview_config)
    elif source == "aaai_ojs":
        ojs_config = conf_config.get("aaai_ojs", {})
        if not isinstance(ojs_config, dict):
            ojs_config = {}
        run_ojs_batch(args, document, json_path, conference, ojs_config)
    else:
        raise SystemExit(f"unsupported source: {source}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Dry-run or apply OpenReview/AAAI OJS author/bib/url enrichment to a PaperJson entry."
    )
    target = parser.add_mutually_exclusive_group(required=False)
    target.add_argument("--path", type=Path, help="Target conference JSON path")
    target.add_argument(
        "--data-dir",
        type=Path,
        default=Path("PaperJson"),
        help="PaperJson root when --path is not used",
    )
    parser.add_argument("-l", "--level", default="A", help="PaperJson level when using --data-dir")
    parser.add_argument("-n", "--conference", help="Conference name")
    parser.add_argument("-y", "--year", help="Four-digit conference year")
    parser.add_argument("--years", help="Comma-separated years for --all")
    parser.add_argument("--title", help="Exact local PaperJson title to enrich")
    parser.add_argument("--all", action="store_true", help="Enrich every title in the target JSON")
    parser.add_argument(
        "--config",
        type=Path,
        default=DEFAULT_CONFIG_PATH,
        help="Provider config path",
    )
    parser.add_argument(
        "--source",
        choices=["auto", "openreview", "aaai_ojs"],
        default="auto",
        help="Metadata source. auto uses --config.",
    )
    parser.add_argument("--venue-id", help="OpenReview venue id, e.g. AAAI.org/2025/Conference")
    parser.add_argument("--forum-id", help="OpenReview forum/note id for a known paper")
    parser.add_argument("--base-url", default=API2_BASE_URL, help="OpenReview API base URL")
    parser.add_argument("--apply", action="store_true", help="Write the JSON file")
    parser.add_argument("--overwrite", action="store_true", help="Replace non-empty fields too")
    parser.add_argument("--no-backup", action="store_true", help="Do not create a .bak when writing")
    parser.add_argument(
        "--scan-venue",
        action="store_true",
        help="Fetch accepted notes by venueid and match locally if title search is insufficient",
    )
    parser.add_argument("--limit", type=int, default=50, help="Title search result limit")
    parser.add_argument("--page-size", type=int, default=1000, help="Venue scan page size")
    parser.add_argument("--max-notes", type=int, default=10000, help="Maximum notes to scan")
    parser.add_argument("--max-records", type=int, help="Maximum local records to query in --all mode")
    parser.add_argument("--sleep", type=float, default=0.2, help="Seconds between venue scan pages")
    parser.add_argument("--timeout", type=int, default=30, help="HTTP timeout in seconds")
    parser.add_argument("--insecure", action="store_true", help="Disable TLS certificate verification")
    parser.add_argument("--dump-candidates", action="store_true", help="Print fetched candidates")
    parser.add_argument(
        "--allow-venue-mismatch",
        action="store_true",
        help="Allow exact title matches whose venueid differs from --venue-id",
    )
    return parser


def main() -> None:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")

    args = build_parser().parse_args()
    if args.path is not None:
        json_path = args.path
        conference = require_str(args.conference or args.path.stem, "--conference")
    else:
        conference = require_str(args.conference or "AAAI", "--conference")
        json_path = infer_json_path(args.data_dir, args.level, conference)

    config = load_config(args.config)
    conf_config = conference_config(config, conference)
    document = load_conference_json(json_path)

    if args.all:
        run_batch(args, document, json_path, conference, conf_config)
        return

    year = require_str(args.year, "--year")
    if not year.isdigit() or len(year) != 4:
        raise SystemExit("--year must be a four-digit year")
    if not args.title:
        raise SystemExit("--title is required unless --all is used")

    sources = source_order(args, conf_config)
    openreview_config = conf_config.get("openreview", {})
    if not isinstance(openreview_config, dict):
        openreview_config = {}
    ojs_config = conf_config.get("aaai_ojs", {})
    if not isinstance(ojs_config, dict):
        ojs_config = {}

    local_title, entry = find_entry(document, year, args.title)

    candidate = None
    candidates: list[Any] = []
    source_used = None

    venue_template = args.venue_id or openreview_config.get("venue_id")
    venue_id = str(venue_template).format(year=year) if venue_template else None

    for source in sources:
        if source == "openreview":
            candidate, candidates = find_openreview_candidate(
                args,
                local_title,
                venue_id or infer_venue_id(conference, year),
            )
        elif source == "aaai_ojs":
            candidate, candidates = find_ojs_candidate(
                args,
                local_title,
                year,
                ojs_config,
            )
        else:
            raise SystemExit(f"unsupported source: {source}")
        if candidate is not None:
            source_used = source
            break

    if args.dump_candidates:
        for item in candidates:
            print(
                json.dumps(
                    {
                        "title": item.title,
                        "authors": item.authors,
                        "venue_id": item.venue_id,
                        "url": item.forum_url,
                        "key": getattr(item, "key", getattr(item, "note_id", "")),
                    },
                    ensure_ascii=False,
                )
            )

    if candidate is None:
        raise SystemExit(
            f"no exact metadata match found for {local_title!r} using sources {sources!r}"
        )

    changed = entry_changes(
        entry,
        candidate,
        conference,
        year,
        overwrite=args.overwrite,
    )

    print(f"Target JSON: {json_path}")
    print(f"Source: {source_used}")
    if source_used == "openreview":
        print(f"OpenReview venue: {venue_id or infer_venue_id(conference, year)}")
    print(f"Matched title: {candidate.title}")
    print(f"Matched URL: {candidate.forum_url}")
    if not changed:
        print("No fields need updating.")
        return

    for field, value in changed.items():
        preview = value.replace("\n", "\\n")
        if len(preview) > 240:
            preview = f"{preview[:237]}..."
        print(f"Would set {field}: {preview}")

    if not args.apply:
        print("Dry-run only. Re-run with --apply to write the JSON file.")
        return

    entry.update(changed)
    backup_path = write_conference_json(json_path, document, backup=not args.no_backup)
    print(f"Updated {json_path}")
    if backup_path is not None:
        print(f"Backup written to {backup_path}")


if __name__ == "__main__":
    main()
