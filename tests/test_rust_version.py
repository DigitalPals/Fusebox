"""One Rust version everywhere: rust-toolchain.toml names it, and every place that picks a Rust
image or toolchain by itself must name the same one. Without this, a new Rust release reaches
The Beast, GitHub or the Docker image before The Dragon, and its Clippy fails there on code that
passed here."""

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def text(path):
    return (ROOT / path).read_text(encoding="utf-8")


class RustVersionTests(unittest.TestCase):
    def setUp(self):
        found = re.findall(r'^channel = "(\d+\.\d+\.\d+)"$', text("rust-toolchain.toml"), re.M)
        self.assertEqual(len(found), 1, "rust-toolchain.toml names one exact version, such as 1.100.0")
        self.version = found[0]

    def test_the_beast_runs_the_same_rust(self):
        image = json.loads(text(".fast-test.json"))["beast"]["image"]
        self.assertEqual(image, f"docker.io/library/rust:{self.version}-trixie")

    def test_the_docker_image_is_built_with_the_same_rust(self):
        self.assertEqual(re.findall(r"^FROM rust:(\S+)", text("Dockerfile"), re.M), [f"{self.version}-bookworm"])

    def test_the_release_builds_with_the_same_rust(self):
        images = re.findall(r'container: "rust:([^"]+)"', text(".github/workflows/release.yml"))
        self.assertEqual(set(images), {f"{self.version}-bookworm"})

    def test_no_workflow_takes_whatever_rust_is_newest(self):
        for workflow in sorted((ROOT / ".github/workflows").glob("*.yml")):
            body = workflow.read_text(encoding="utf-8")
            self.assertNotRegex(body, r"rust-toolchain@(stable|beta|nightly)", workflow.name)
            self.assertNotRegex(body, r"\brust:1-", workflow.name)

    def test_the_crate_builds_with_it(self):
        minimum = re.search(r'^rust-version = "([\d.]+)"$', text("Cargo.toml"), re.M).group(1)
        as_numbers = lambda v: [int(part) for part in v.split(".")]
        self.assertGreaterEqual(as_numbers(self.version), as_numbers(minimum))


if __name__ == "__main__":
    unittest.main()
