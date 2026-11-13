#!/usr/bin/env python3
"""Say when a newer stable Rust is out than the one Fusebox builds with.

Fusebox builds with the one version in rust-toolchain.toml and doesn't move by itself, so
nothing else tells. The default only reports. --update-issues keeps one issue open while a
newer Rust is out and closes it once Fusebox builds with it; it needs GITHUB_TOKEN and
GITHUB_REPOSITORY. A release that can't be read changes no issue.
"""

import argparse
import json
import os
import re
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
API = "https://api.github.com"
LABEL = "rust-version"
MARKER = "<!-- rust-version -->"
# No version in the title: a later Rust edits the same issue and sends no second notice.
TITLE = "A newer Rust is out"


def http(path, method="GET", body=None, token=None):
    headers = {"User-Agent": "fusebox-rust-release", "Accept": "application/vnd.github+json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(API + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.loads(response.read().decode())


def numbers(version):
    """`1.100.0` -> (1, 100, 0); anything else (a beta, a nightly) isn't a stable release."""
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"unexpected Rust version {version!r}")
    return tuple(int(part) for part in version.split("."))


def used():
    with open(os.path.join(ROOT, "rust-toolchain.toml"), encoding="utf-8") as f:
        return re.search(r'^channel = "(.*)"$', f.read(), re.M).group(1)


def newest(token):
    return http("/repos/rust-lang/rust/releases/latest", token=token)["tag_name"]


def issue_body(ours, latest):
    return "\n".join([
        MARKER,
        f"Rust **{latest}** is out. Fusebox builds with **{ours}** everywhere and stays on it until someone "
        "moves it. Nothing breaks by waiting.",
        "",
        "To move: change the version in `rust-toolchain.toml` and in the image names that "
        "`tests/test_rust_version.py` lists, in one commit, and fix what the new Clippy finds. Run `fast-test`, "
        "then `gh workflow run ci.yml --ref <branch>` for Windows and macOS (`AGENTS.md`, Rust version).",
        "",
        f"- Release notes: https://github.com/rust-lang/rust/releases/tag/{latest}",
        "",
        "This issue closes by itself once `main` builds with the newest Rust.",
    ])


def sync(repo, token, ours, latest):
    """One open issue while a newer Rust is out, none otherwise. Returns what it did."""
    found = http(f"/repos/{repo}/issues?labels={LABEL}&state=open&per_page=100", token=token)
    issues = sorted((i for i in found if MARKER in (i.get("body") or "")), key=lambda i: i["number"])
    if numbers(latest) <= numbers(ours):
        for issue in issues:
            http(f"/repos/{repo}/issues/{issue['number']}", "PATCH", {"state": "closed", "state_reason": "completed"}, token)
        return "closed the Rust issue" if issues else "no issue needed"
    body = issue_body(ours, latest)
    if not issues:
        issue = http(f"/repos/{repo}/issues", "POST", {"title": TITLE, "body": body, "labels": [LABEL]}, token)
        return f"opened issue #{issue['number']}"
    if issues[0].get("body") != body:
        http(f"/repos/{repo}/issues/{issues[0]['number']}", "PATCH", {"title": TITLE, "body": body}, token)
    return f"issue #{issues[0]['number']} is current"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update-issues", action="store_true", help="Open, edit or close the issue about a newer Rust")
    args = parser.parse_args(argv)
    token, repo = os.environ.get("GITHUB_TOKEN"), os.environ.get("GITHUB_REPOSITORY")
    if args.update_issues and not (token and repo):
        print("::error::--update-issues requires GITHUB_TOKEN and GITHUB_REPOSITORY")
        return 1
    try:
        ours, latest = used(), newest(token)
        newer = numbers(latest) > numbers(ours)
        lines = [f"Rust: Fusebox builds with {ours}, the newest stable is {latest}" + (" (newer)." if newer else ".")]
        if args.update_issues:
            lines.append(sync(repo, token, ours, latest))
    except Exception as error:  # noqa: BLE001 - whatever went wrong, say it and change nothing further
        print(f"::error::The Rust release could not be checked: {error}")
        return 1
    print("\n".join(lines))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write("\n\n" + "\n\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
