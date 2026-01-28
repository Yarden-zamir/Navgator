#!/usr/bin/env python3
import json
import os
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlparse


def main() -> int:
    home = Path.home()
    items = build_items(home)
    updated = 0
    scanned = 0

    for path in items:
        if not path.is_dir():
            continue
        repo_root = git_root(path)
        if repo_root is None:
            continue
        scanned += 1

        info = gh_repo_info(repo_root)
        if info is None:
            continue
        if not info.get("isInOrganization"):
            continue

        url = info.get("url")
        org = org_from_url(url)
        if not org:
            continue

        tag = f"org/{org}"
        if update_tags(repo_root, tag):
            updated += 1

    print(f"Scanned {scanned} repos, updated {updated} files.")
    return 0


def build_items(home: Path) -> list[Path]:
    items: list[Path] = []
    items.extend(static_items(home))
    for folder in index_folders(home):
        if not folder.is_dir():
            continue
        items.append(folder)
        for child in folder.iterdir():
            if child.is_dir():
                items.append(child)
    return items


def index_folders(home: Path) -> list[Path]:
    return [home / "Github", home / "Desktop"]


def static_items(home: Path) -> list[Path]:
    return [
        home / "Desktop",
        Path("/opt/homebrew"),
        home / "Downloads",
        home
        / "Library"
        / "Application Support"
        / "ModrinthApp"
        / "profiles"
        / "Create-Prepare-to-Dye",
        home
        / "Library"
        / "Application Support"
        / "ModrinthApp"
        / "profiles"
        / "Create ptd 2",
    ]


def git_root(path: Path) -> Path | None:
    result = subprocess.run(
        ["git", "-C", str(path), "rev-parse", "--show-toplevel"],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    if result.returncode != 0:
        return None
    root = result.stdout.strip()
    return Path(root) if root else None


def gh_repo_info(path: Path) -> dict | None:
    result = subprocess.run(
        ["gh", "repo", "view", "--json", "isInOrganization,url"],
        cwd=str(path),
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def org_from_url(url: str | None) -> str | None:
    if not url:
        return None
    parsed = urlparse(url)
    parts = [part for part in parsed.path.split("/") if part]
    if len(parts) < 2:
        return None
    return parts[0]


def update_tags(repo_root: Path, tag: str) -> bool:
    config_path = repo_root / ".navgator.toml"
    if config_path.exists():
        contents = config_path.read_text()
    else:
        contents = ""

    tags = parse_tags_from_toml(contents)
    if tag in tags:
        return False
    tags.append(tag)

    updated = write_tags_into_toml(contents, tags)
    config_path.write_text(updated)
    return True


def parse_tags_from_toml(contents: str) -> list[str]:
    in_tags = False
    buffer = []
    for line in contents.splitlines():
        cleaned = line.split("#", 1)[0].strip()
        if not cleaned:
            continue
        if not in_tags:
            if "=" not in cleaned:
                continue
            key, value = cleaned.split("=", 1)
            if key.strip() != "tags":
                continue
            value = value.strip()
            buffer.append(value)
            if "[" in value:
                in_tags = True
            if "]" in value:
                break
        else:
            buffer.append(cleaned)
            if "]" in cleaned:
                break

    if not buffer:
        return []
    return extract_quoted_strings(" ".join(buffer))


def extract_quoted_strings(text: str) -> list[str]:
    tags = []
    in_string = False
    current = []
    for ch in text:
        if ch == '"':
            if in_string:
                tag = "".join(current)
                if tag:
                    tags.append(tag)
                current = []
                in_string = False
            else:
                in_string = True
        elif in_string:
            current.append(ch)
    return tags


def write_tags_into_toml(contents: str, tags: list[str]) -> str:
    line = f"tags = [{', '.join(format_tag(tag) for tag in tags)}]"
    if not contents.strip():
        return line + "\n"

    lines = contents.splitlines()
    start = None
    end = None
    for i, raw in enumerate(lines):
        cleaned = raw.split("#", 1)[0]
        if start is None:
            if "=" not in cleaned:
                continue
            key = cleaned.split("=", 1)[0].strip()
            if key == "tags":
                start = i
                if "]" in cleaned:
                    end = i
                    break
        else:
            if "]" in cleaned:
                end = i
                break

    if start is None:
        return contents.rstrip() + "\n" + line + "\n"

    if end is None:
        end = start
    new_lines = lines[:start] + [line] + lines[end + 1 :]
    return "\n".join(new_lines) + "\n"


def format_tag(tag: str) -> str:
    return '"' + tag.replace('"', '\\"') + '"'


if __name__ == "__main__":
    raise SystemExit(main())
