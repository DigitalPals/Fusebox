#!/usr/bin/env python3
"""Report client releases daily; maintain one compatibility review issue weekly.

`version` is the bundled identity, while `reviewed` is the latest upstream release
whose compatibility was checked. A version difference alone does not mean Fusebox
needs a release. The default and --dry-run never write to GitHub. --update-issues
requires GITHUB_TOKEN and GITHUB_REPOSITORY, and updates the issue without comments
or repeated release notifications. Any failed release check prevents issue changes.
"""

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
LABEL = "client-compatibility"
LEGACY_LABEL = "client-version"
MARKER = "<!-- client-compatibility-review -->"
TITLE = "Review upstream client compatibility"
API = "https://api.github.com"


def http(url, method="GET", body=None, token=None):
    headers = {"User-Agent": "fusebox-client-versions", "Accept": "application/json"}
    if token and url.startswith(API):
        headers["Authorization"] = f"Bearer {token}"
        headers["Accept"] = "application/vnd.github+json"
    data = json.dumps(body).encode() if body is not None else None
    if data:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    with urllib.request.urlopen(req, timeout=20) as resp:
        return resp.read().decode()


def fetch(url, token=None):
    """One retry: a release channel hiccup shouldn't fail the run."""
    try:
        return http(url, token=token)
    except (urllib.error.URLError, TimeoutError):
        time.sleep(5)
        return http(url, token=token)


def latest(source, token):
    kind = source["kind"]
    if kind == "npm":
        tags = json.loads(fetch(f"https://registry.npmjs.org/-/package/{source['package']}/dist-tags"))
        version = tags["latest"]
    elif kind == "github-release":
        release = json.loads(fetch(f"{API}/repos/{source['repo']}/releases/latest", token))
        version = release["tag_name"].removeprefix(source.get("strip", ""))
    elif kind == "text":
        version = fetch(source["url"]).strip().splitlines()[0]
    elif kind == "json":
        value = json.loads(fetch(source["url"]))
        for key in source["path"].split("."):
            value = value[key]
        version = str(value)
    elif kind == "yaml-line":
        prefix = source["key"] + ":"
        line = next(l for l in fetch(source["url"]).splitlines() if l.startswith(prefix))
        version = line[len(prefix):].strip().strip("'\"")
    else:
        raise ValueError(f"unknown source kind {kind}")
    if source.get("cut"):
        version = version.split(source["cut"])[0]
    return version.strip().removeprefix("v")


def parse(version):
    """`0.161.0` -> (0, 161, 0); pre-releases (`0.162.0-alpha.2`) are not releases."""
    if not re.fullmatch(r"\d+(\.\d+)*", version):
        return None
    return tuple(int(p) for p in version.split("."))


IDENTITIES = {
    "claude-code": "Learns newer identities from real Claude Code requests; also accepts client-identities.",
    "codex": "Learns newer identities from real Codex requests; also accepts client-identities.",
    "antigravity": "Fetches its current identity from the update manifest every 3 hours; the bundled version is a fallback.",
    "grok-build": "Uses the bundled identity unless client-identities sets a newer version.",
    "devin-cli": "Uses the bundled identity unless client-identities sets a newer version.",
    "muse-code": "Uses a bundled version AND build hash; a version override alone cannot update this identity.",
}


def load_clients():
    with open(os.path.join(ROOT, ".github", "client-versions.json")) as f:
        clients = json.load(f)["clients"]
    ids = set()
    for client in clients:
        name = client["id"]
        if name not in IDENTITIES or name in ids:
            raise ValueError(f"unknown or duplicate client {name}")
        ids.add(name)
        for key in ("version", "reviewed"):
            if parse(client[key]) is None:
                raise ValueError(f"{name}: invalid {key} version")
        if parse(client["reviewed"]) < parse(client["version"]):
            raise ValueError(f"{name}: reviewed must not be older than the bundled identity")
    if ids != IDENTITIES.keys():
        raise ValueError("the client list must cover every supported client identity")
    return clients


def needs_review(client, newest):
    return parse(newest) > parse(client["reviewed"])


def issue_body(pending):
    lines = [
        MARKER,
        "Upstream releases need a compatibility review. A newer release does not by itself mean Fusebox is incompatible or needs an update.",
        "",
        "This weekly review collects all clients in one issue. Daily checks stay in the Actions summary; later releases update this issue without comments.",
    ]
    for client, newest in pending:
        lines += [
            "",
            f"### {client['name']}",
            "",
            f"Reviewed **{client['reviewed']}** → latest **{newest}**. Bundled identity: **{client['version']}**.",
            "",
            IDENTITIES[client["id"]],
            "",
            f"- Where to check: {client['where']}",
        ]
        if client.get("notes"):
            lines.append(f"- Release notes: {client['notes']}")
        if client.get("compare"):
            url = client["compare"].format(**{"from": client["reviewed"], "to": newest})
            lines.append(f"- Changes since the last review: {url}")
        source = client["source"]
        if source.get("url"):
            lines.append(f"- Release channel: {source['url']}")
    lines += [
        "",
        "Review every intervening release's authentication, headers, request bodies and WebSocket messages. "
        "Fix protocol differences and add regression coverage when needed. Then advance `reviewed` in "
        "`.github/client-versions.json` and record the evidence in `docs/client-compatibility.md`. "
        "A review with no required code change still advances `reviewed`.",
        "",
        "Only change `version` and its matching Rust constants when updating the bundled identity; "
        "automatic runtime identities can already be newer. Do not blindly change a version to claim compatibility. "
        "The issue closes after all releases have been reviewed.",
    ]
    return "\n".join(lines)


class IssuesOff(Exception):
    pass


class Issues:
    def __init__(self, repo, token):
        self.repo, self.token = repo, token
        if not json.loads(self.call(f"/repos/{repo}")).get("has_issues"):
            raise IssuesOff(repo)
        self.reviews = [issue for issue in self.list_open(LABEL) if MARKER in (issue.get("body") or "")]
        self.legacy = []
        for issue in self.list_open(LEGACY_LABEL):
            match = re.search(rf"<!-- {LEGACY_LABEL}: ([\w-]+) -->", issue.get("body") or "")
            if match and match[1] in IDENTITIES:
                self.legacy.append((issue, match[1]))

    def call(self, path, method="GET", body=None):
        return http(API + path, method, body, self.token)

    def list_open(self, label):
        found = []
        for page in range(1, 11):
            batch = json.loads(self.call(f"/repos/{self.repo}/issues?labels={label}&state=open&per_page=100&page={page}"))
            found.extend(issue for issue in batch if "pull_request" not in issue)
            if len(batch) < 100:
                return found
        raise ValueError(f"too many {label} issues to reconcile safely")

    def ensure_label(self):
        try:
            self.call(f"/repos/{self.repo}/labels/{LABEL}")
        except urllib.error.HTTPError as e:
            if e.code != 404:
                raise
            self.call(f"/repos/{self.repo}/labels", "POST",
                      {"name": LABEL, "color": "d4a72c", "description": "Weekly review of upstream client compatibility"})

    def close(self, issue, reason, note=None):
        body = {"state": "closed", "state_reason": reason}
        if note:
            body["body"] = (issue.get("body") or "") + "\n\n" + note
        self.call(f"/repos/{self.repo}/issues/{issue['number']}", "PATCH", body)

    def sync(self, pending):
        reviews = sorted(self.reviews, key=lambda issue: issue["number"])
        review = reviews[0] if reviews else None
        if pending:
            body = issue_body(pending)
            if review:
                if review["title"] != TITLE or review.get("body") != body:
                    self.call(f"/repos/{self.repo}/issues/{review['number']}", "PATCH", {"title": TITLE, "body": body})
                action = f"review issue #{review['number']} is current"
            else:
                self.ensure_label()
                review = json.loads(self.call(f"/repos/{self.repo}/issues", "POST",
                                              {"title": TITLE, "body": body, "labels": [LABEL]}))
                action = f"opened review issue #{review['number']}"
            for duplicate in reviews[1:]:
                self.close(duplicate, "not_planned", f"Consolidated into #{review['number']}.")
        else:
            for issue in reviews:
                self.close(issue, "completed")
            action = "all upstream releases have been reviewed"
        # Retire only automation-owned version alerts, after the new review is saved.
        # No comments: legacy alerts and later releases must not keep sending mail.
        pending_ids = {client["id"] for client, _ in pending}
        for issue, client in self.legacy:
            reason = "not_planned" if client in pending_ids else "completed"
            self.close(issue, reason, "Superseded by the weekly Client compatibility review. "
                       "Bundled version drift alone does not imply an incompatibility. "
                       "See README.md → Development → Client compatibility.")
        return action + (f"; retired {len(self.legacy)} legacy version alerts" if self.legacy else "")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update-issues", action="store_true", help="Reconcile the weekly compatibility review and retire legacy alerts")
    parser.add_argument("--dry-run", action="store_true", help="Report only, even with --update-issues and GitHub credentials")
    args = parser.parse_args(argv)
    token, repo = os.environ.get("GITHUB_TOKEN"), os.environ.get("GITHUB_REPOSITORY")
    update = args.update_issues and not args.dry_run
    if update and not (token and repo):
        print("::error::--update-issues requires GITHUB_TOKEN and GITHUB_REPOSITORY")
        return 1
    try:
        clients = load_clients()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"::error::Invalid client manifest: {error}")
        return 1
    rows, pending, failed = [], [], False
    for client in clients:
        try:
            newest = latest(client["source"], token)
            if parse(newest) is None:
                raise ValueError(f"unexpected version {newest!r}")
        except Exception as e:  # noqa: BLE001 - report every broken channel, then fail
            failed = True
            rows.append((client["name"], client["version"], client["reviewed"], "?", f"could not check: {e}"))
            print(f"::error::{client['name']}: release channel could not be checked: {e}")
            continue
        if needs_review(client, newest):
            pending.append((client, newest))
        rows.append((client["name"], client["version"], client["reviewed"], newest,
                     "review pending" if needs_review(client, newest) else "reviewed"))
    table = ["| Client | Bundled identity | Reviewed | Latest | Status |", "|---|---|---|---|---|"]
    table += [f"| {a} | {b} | {c} | {d} | {e} |" for a, b, c, d, e in rows]
    table += ["", "Version drift is informational; runtime identities may already be newer. "
              "Compatibility reviews are tracked separately from bundled versions."]
    if update and not failed:
        try:
            action = Issues(repo, token).sync(pending)
            table += ["", action]
        except IssuesOff:
            failed = True
            table += ["", f"::error::Issues are turned off for {repo}; the compatibility review could not be updated."]
        except (OSError, ValueError, KeyError, TypeError) as error:
            failed = True
            table += ["", f"::error::Compatibility issue update failed: {error}"]
    elif failed:
        table += ["", "Issues left unchanged because a release check failed."]
    else:
        table += ["", "Report only. The weekly run (or --update-issues) reconciles the review issue."]
    print("\n".join(table))
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
            f.write("\n".join(table) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
