import importlib.util
import io
from pathlib import Path
import random
import struct
import unittest
import zlib

SPEC = importlib.util.spec_from_file_location("antigravity_build", Path(__file__).resolve().parents[1] / "scripts/antigravity-build.py")
build = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build)

MANIFEST = """version: 2.33.0
files:
  - url: https://storage.googleapis.com/antigravity-public/antigravity-hub/2.33.0-4731201358004224/darwin-arm/Antigravity.zip
    sha512: akGKBoAWOxZmddrQyvSHDxmnCtaaLnz7jaBTDg+8g8vG/lytBldvIkHRPxXA+WWsm6Xq2FP+fSzWD/BKFmsBvQ==
    size: 208556255
path: Antigravity.zip
"""


def language_server():
    """Bytes that don't compress, with a near miss and then the stamp 40% in."""
    data = bytearray(random.Random(7).randbytes(3_000_000))
    for at, text in [(500_000, b"changelist 77 with baseline 1 "), (1_200_000, b"changelist 1023160026 with baseline 1022452901\0")]:
        data[at:at + len(text)] = text
    return bytes(data)


def zip_of(entries):
    out, directory = b"", b""
    for name, data, deflate in entries:
        body = zlib.compress(data, 1)[2:-4] if deflate else data
        method, offset = (8 if deflate else 0), len(out)
        sizes = struct.pack("<II", len(body), len(data))
        out += b"PK\x03\x04\x14\0\0\0" + struct.pack("<H", method) + bytes(8) + sizes
        out += struct.pack("<HH", len(name), 5) + name + b"UT\x01\x00\x07" + body
        directory += b"PK\x01\x02\x14\0\x14\0\0\0" + struct.pack("<H", method) + bytes(8) + sizes
        directory += struct.pack("<H", len(name)) + bytes(12) + struct.pack("<I", offset) + name
    return out + directory + b"PK\x05\x06" + bytes(4) + struct.pack("<HHIIH", len(entries), len(entries), len(directory), len(out), 0)


class Counted(io.BytesIO):
    def __init__(self, data, log):
        super().__init__(data)
        self.log = log

    def read(self, size=-1):
        out = super().read(size)
        self.log.append(len(out))
        return out


def serving(blob, reads):
    """A ranged reader over blob that records how many bytes were read."""
    def get(url, spec):
        first, last = spec.split("-")
        first, last = (len(blob) - int(last), len(blob) - 1) if not first else (int(first), int(last))
        return Counted(blob[first:last + 1], reads)
    return get


class AntigravityBuildTests(unittest.TestCase):
    def test_the_manifest_names_googles_zip_for_its_release(self):
        url = build.release_url(MANIFEST, "2.33.0")
        self.assertTrue(url.endswith("/2.33.0-4731201358004224/darwin-arm/Antigravity.zip"))
        self.assertIsNone(build.release_url(MANIFEST, "2.34.0"))
        self.assertIsNone(build.release_url(MANIFEST.replace("storage.googleapis.com", "example.com"), "2.33.0"))

    def test_reads_the_stamp_from_the_language_server_without_reading_the_rest(self):
        server = language_server()
        for deflate in (True, False):
            blob = zip_of([(b"Antigravity.app/Contents/Info.plist", b"<plist/>", deflate),
                           (build.LANGUAGE_SERVER, server, deflate),
                           (b"Antigravity.app/Contents/Resources/app.asar", random.Random(8).randbytes(2_000_000), deflate)])
            reads = []
            self.assertEqual(build.read_build("zip", serving(blob, reads)), ("1023160026", "1022452901"))
            # The end record, the directory, the entry header, then the language server
            # only as far as its stamp: two of its three megabytes.
            self.assertLess(sum(reads), 2_200_000)

    def test_a_release_without_a_language_server_fails(self):
        blob = zip_of([(b"Antigravity.app/Contents/Info.plist", b"<plist/>", True)])
        with self.assertRaisesRegex(ValueError, "no language server"):
            build.read_build("zip", serving(blob, []))


if __name__ == "__main__":
    unittest.main()
