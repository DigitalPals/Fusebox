"""Release monitoring must be quiet, idempotent and fail without losing reviews."""

import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import unittest
from unittest import mock
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlparse


SCRIPT = Path(__file__).resolve().parents[1] / ".github/scripts/client_versions.py"
SPEC = importlib.util.spec_from_file_location("client_versions", SCRIPT)
versions = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(versions)


class GitHub:
    def __init__(self):
        self.issues = []
        self.calls = []
        self.has_issues = True
        self.create_fails = False

    def issue(self, number, body, label=None):
        issue = {"number": number, "title": versions.TITLE, "body": body,
                 "labels": [label or versions.LABEL], "state": "open"}
        self.issues.append(issue)
        return issue

    def __call__(self, url, method="GET", body=None, token=None):
        path = urlparse(url).path
        self.calls.append((path, method, copy.deepcopy(body)))
        if method == "GET":
            if path == "/repos/test/fusebox":
                return json.dumps({"has_issues": self.has_issues})
            if "/labels/" in path:
                return "{}"
            label = parse_qs(urlparse(url).query)["labels"][0]
            return json.dumps([i for i in self.issues if i["state"] == "open" and label in i["labels"]])
        if method == "POST":
            if self.create_fails:
                raise HTTPError(url, 403, "Forbidden", {}, None)
            number = max((i["number"] for i in self.issues), default=0) + 1
            issue = self.issue(number, body["body"])
            issue.update(body)
            return json.dumps(issue)
        if method == "PATCH":
            issue = next(i for i in self.issues if i["number"] == int(path.rsplit("/", 1)[1]))
            issue.update(body)
            return json.dumps(issue)
        raise AssertionError((path, method))

    def mutations(self):
        return [call for call in self.calls if call[1] != "GET"]


class ClientVersionsTests(unittest.TestCase):
    def setUp(self):
        self.clients = versions.load_clients()
        self.github = GitHub()
        self.http = mock.patch.object(versions, "http", self.github)
        self.http.start()
        self.addCleanup(self.http.stop)

    def run_main(self, args=(), newest=None, env=None):
        output = io.StringIO()
        with mock.patch.dict(os.environ, env or {"GITHUB_TOKEN": "test", "GITHUB_REPOSITORY": "test/fusebox"}, clear=True), \
                mock.patch.object(versions, "load_clients", return_value=self.clients), \
                mock.patch.object(versions, "latest", side_effect=newest or [c["reviewed"] for c in self.clients]), \
                contextlib.redirect_stdout(output):
            status = versions.main(list(args))
        return status, output.getvalue()

    def pending(self):
        return [(c, c["reviewed"] + ".1") for c in self.clients]

    def test_daily_and_dry_runs_never_touch_issues_even_with_credentials(self):
        for args in [(), ("--dry-run",), ("--update-issues", "--dry-run")]:
            status, output = self.run_main(args, [v for _, v in self.pending()])
            self.assertEqual(status, 0)
            self.assertIn("review pending", output)
            self.assertEqual(self.github.calls, [])

    def test_review_baseline_is_independent_of_bundled_identity(self):
        self.clients[0]["version"] = "1.0.0"
        status, output = self.run_main()
        self.assertEqual(status, 0)
        self.assertNotIn("review pending", output)
        self.assertIn("| 1.0.0 |", output)

    def test_failed_or_invalid_release_check_preserves_all_issues(self):
        for failure in [OSError("channel unavailable"), "0.165.0-alpha.1"]:
            newest = [c["reviewed"] for c in self.clients]
            newest[1] = failure
            status, output = self.run_main(("--update-issues",), newest)
            self.assertEqual(status, 1)
            self.assertIn("Issues left unchanged", output)
            self.assertIn(self.clients[-1]["name"], output)
            self.assertEqual(self.github.calls, [])

    def test_issue_updates_require_explicit_credentials(self):
        status, output = self.run_main(("--update-issues",), env={"GITHUB_REPOSITORY": "test/fusebox"})
        self.assertEqual(status, 1)
        self.assertIn("requires GITHUB_TOKEN", output)
        self.assertEqual(self.github.calls, [])

    def test_all_pending_clients_share_one_issue_and_no_comments(self):
        status, _ = self.run_main(("--update-issues",), [v for _, v in self.pending()])
        self.assertEqual(status, 0)
        self.assertEqual(len(self.github.issues), 1)
        body = self.github.issues[0]["body"]
        for client in self.clients:
            self.assertIn("### " + client["name"], body)
        self.assertIn("Learns newer identities", body)
        self.assertIn("every 3 hours", body)
        self.assertIn("version AND build hash", body)
        self.assertEqual(len(self.github.mutations()), 1)

    def test_later_releases_edit_the_same_issue_without_notifications(self):
        issues = versions.Issues("test/fusebox", "test")
        issues.sync(self.pending())
        self.github.calls.clear()
        pending = self.pending()
        pending[1] = (pending[1][0], "0.200.0")
        issues = versions.Issues("test/fusebox", "test")
        issues.sync(pending)
        self.assertEqual(len(self.github.issues), 1)
        mutations = self.github.mutations()
        self.assertEqual(len(mutations), 1)
        self.assertEqual(mutations[0][1], "PATCH")
        self.assertIn("0.200.0", mutations[0][2]["body"])
        self.github.calls.clear()
        versions.Issues("test/fusebox", "test").sync(pending)
        self.assertEqual(self.github.mutations(), [])

    def test_review_completion_closes_digest_without_comments(self):
        issue = self.github.issue(1, versions.MARKER)
        versions.Issues("test/fusebox", "test").sync([])
        self.assertEqual(issue["state"], "closed")
        self.assertEqual(issue["state_reason"], "completed")
        self.assertEqual(len(self.github.mutations()), 1)

    def test_only_owned_legacy_alerts_and_duplicate_digests_are_retired(self):
        legacy = self.github.issue(2, "<!-- client-version: codex -->", versions.LEGACY_LABEL)
        human = self.github.issue(3, "A real compatibility bug", versions.LEGACY_LABEL)
        primary = self.github.issue(4, versions.MARKER)
        duplicate = self.github.issue(5, versions.MARKER)
        pull_request = self.github.issue(6, versions.MARKER)
        pull_request["pull_request"] = {}
        versions.Issues("test/fusebox", "test").sync(self.pending())
        self.assertEqual(primary["state"], "open")
        self.assertEqual(duplicate["state"], "closed")
        self.assertEqual(legacy["state_reason"], "not_planned")
        self.assertEqual(human["state"], "open")
        self.assertEqual(pull_request["state"], "open")
        self.assertFalse(any("/comments" in path for path, _, _ in self.github.calls))

    def test_failed_digest_creation_does_not_retire_legacy_alerts(self):
        legacy = self.github.issue(2, "<!-- client-version: codex -->", versions.LEGACY_LABEL)
        self.github.create_fails = True
        status, output = self.run_main(("--update-issues",), [v for _, v in self.pending()])
        self.assertEqual(status, 1)
        self.assertIn("issue update failed", output)
        self.assertEqual(legacy["state"], "open")

    def test_reviewed_legacy_alerts_close_as_completed(self):
        legacy = self.github.issue(2, "<!-- client-version: codex -->", versions.LEGACY_LABEL)
        status, output = self.run_main(("--update-issues",))
        self.assertEqual(status, 0)
        self.assertEqual(legacy["state_reason"], "completed")
        self.assertIn("retired 1 legacy version alerts", output)

    def test_issues_disabled_is_a_visible_failure(self):
        self.github.has_issues = False
        status, output = self.run_main(("--update-issues",))
        self.assertEqual(status, 1)
        self.assertIn("Issues are turned off", output)

    def test_compare_link_covers_every_release_since_the_last_review(self):
        codex = copy.deepcopy(next(c for c in self.clients if c["id"] == "codex"))
        codex.update(version="0.160.0", reviewed="0.164.0")
        body = versions.issue_body([(codex, "0.166.0")])
        self.assertIn("rust-v0.164.0...rust-v0.166.0", body)
        self.assertNotIn("rust-v0.160.0...", body)

    def test_manifest_requires_complete_unique_clients_and_valid_review_baselines(self):
        for mutate in [
                lambda clients: clients.pop(),
                lambda clients: clients.append(clients[0]),
                lambda clients: clients[0].update(reviewed="0.0.1"),
                lambda clients: clients[0].update(reviewed="invalid")]:
            clients = copy.deepcopy(self.clients)
            mutate(clients)
            with mock.patch("builtins.open", mock.mock_open(read_data=json.dumps({"clients": clients}))):
                with self.assertRaises(ValueError):
                    versions.load_clients()


if __name__ == "__main__":
    unittest.main()
