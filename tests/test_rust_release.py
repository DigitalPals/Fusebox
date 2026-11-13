"""A newer Rust gets one issue, edited rather than repeated, and closed once Fusebox builds with it."""

import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import unittest
from unittest import mock
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("rust_release", ROOT / ".github/scripts/rust_release.py")
rust = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rust)


class GitHub:
    """Stands in for GitHub: Rust's latest release and this repository's issues."""

    def __init__(self, latest):
        self.latest = latest
        self.issues = []
        self.calls = []

    def __call__(self, path, method="GET", body=None, token=None):
        path = urlparse(path).path
        self.calls.append((path, method, copy.deepcopy(body)))
        if path == "/repos/rust-lang/rust/releases/latest":
            if isinstance(self.latest, Exception):
                raise self.latest
            return {"tag_name": self.latest}
        if method == "GET":
            return [i for i in self.issues if i["state"] == "open"]
        if method == "POST":
            issue = {"number": len(self.issues) + 1, "state": "open", **body}
            self.issues.append(issue)
            return issue
        issue = next(i for i in self.issues if i["number"] == int(path.rsplit("/", 1)[1]))
        issue.update(body)
        return issue

    def changes(self):
        return [call for call in self.calls if call[1] != "GET"]


class RustReleaseTests(unittest.TestCase):
    def run_main(self, github, *args, ours="1.100.0"):
        output = io.StringIO()
        env = {"GITHUB_TOKEN": "test", "GITHUB_REPOSITORY": "test/fusebox"}
        with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(rust, "http", github), \
                mock.patch.object(rust, "used", return_value=ours), contextlib.redirect_stdout(output):
            status = rust.main(list(args))
        return status, output.getvalue()

    def test_it_reads_the_version_fusebox_builds_with(self):
        toolchain = (ROOT / "rust-toolchain.toml").read_text(encoding="utf-8")
        self.assertIn(f'channel = "{rust.used()}"', toolchain)
        rust.numbers(rust.used())

    def test_a_report_changes_no_issue(self):
        github = GitHub("1.101.0")
        status, output = self.run_main(github)
        self.assertEqual(status, 0)
        self.assertIn("1.100.0", output)
        self.assertIn("1.101.0", output)
        self.assertEqual(github.changes(), [])

    def test_a_newer_rust_opens_one_issue_and_later_ones_edit_it(self):
        github = GitHub("1.101.0")
        self.assertEqual(self.run_main(github, "--update-issues")[0], 0)
        self.assertEqual(len(github.issues), 1)
        issue = github.issues[0]
        self.assertEqual(issue["labels"], [rust.LABEL])
        self.assertIn("1.100.0", issue["body"])
        self.assertIn("1.101.0", issue["body"])
        self.assertNotIn("1.101.0", issue["title"])

        github.calls.clear()
        self.run_main(github, "--update-issues")
        self.assertEqual(github.changes(), [])

        github.latest = "1.102.0"
        self.run_main(github, "--update-issues")
        self.assertEqual([call[1] for call in github.changes()], ["PATCH"])
        self.assertEqual(len(github.issues), 1)
        self.assertIn("1.102.0", issue["body"])

    def test_the_issue_closes_once_fusebox_builds_with_the_newest(self):
        github = GitHub("1.101.0")
        self.run_main(github, "--update-issues")
        for ours in ("1.101.0", "1.101.1"):
            github.issues[0]["state"] = "open"
            self.run_main(github, "--update-issues", ours=ours)
            self.assertEqual(github.issues[0]["state"], "closed")

    def test_no_newer_rust_means_no_issue(self):
        github = GitHub("1.100.0")
        self.assertEqual(self.run_main(github, "--update-issues")[0], 0)
        self.assertEqual(github.issues, [])
        self.assertEqual(github.changes(), [])

    def test_a_release_that_cannot_be_read_fails_and_changes_nothing(self):
        for latest in (OSError("unreachable"), "1.101.0-beta.2", "nightly"):
            github = GitHub("1.101.0")
            self.run_main(github, "--update-issues")
            github.calls.clear()
            github.latest = latest
            status, output = self.run_main(github, "--update-issues")
            self.assertEqual(status, 1, latest)
            self.assertIn("::error::", output)
            self.assertEqual(github.changes(), [])
            self.assertEqual(github.issues[0]["state"], "open")

    def test_issue_changes_need_credentials(self):
        github = GitHub("1.101.0")
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(rust, "http", github), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(rust.main(["--update-issues"]), 1)
        self.assertEqual(github.calls, [])

    def test_the_daily_workflow_runs_it(self):
        workflow = (ROOT / ".github/workflows/client-versions.yml").read_text(encoding="utf-8")
        self.assertIn(".github/scripts/rust_release.py", workflow)


if __name__ == "__main__":
    unittest.main()
