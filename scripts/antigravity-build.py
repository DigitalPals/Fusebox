#!/usr/bin/env python3
"""Print an Antigravity release's build number: the `cl=` of its user agent.

Reads Google's update manifest, then only what it needs of the release ZIP: the
ZIP directory and the language server's compressed bytes, inflated as they arrive
until its stamp (`changelist N with baseline M`) appears, about 37 MB of 208 MB.
Nothing is saved or run. Fusebox does the same at run time
(src/antigravity/release.rs); this is for reviews, to add a release to
CHANGELISTS in src/antigravity.rs.

    python3 scripts/antigravity-build.py           # the manifest's current release
    python3 scripts/antigravity-build.py URL       # a release ZIP you name
"""

import re
import struct
import sys
import urllib.request
import zlib

MANIFEST = "https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/manifest/latest-arm64-mac.yml"
STORAGE = "https://storage.googleapis.com/antigravity-public/antigravity-hub/"
LANGUAGE_SERVER = b"Antigravity.app/Contents/Resources/bin/language_server"
TAIL = 22 + 65_535
STAMP = re.compile(rb"changelist (\d{6,15}) with baseline (\d+)(?=\D)")


def ranged(url, spec):
    """`bytes=spec` of url; anything but a partial answer fails."""
    req = urllib.request.Request(url, headers={"Range": f"bytes={spec}", "Accept-Encoding": "identity",
                                               "User-Agent": "fusebox-antigravity-build"})
    resp = urllib.request.urlopen(req, timeout=300)
    if resp.status != 206:
        raise ValueError(f"the release answered {resp.status} to a range request")
    return resp


def release_url(manifest, version):
    """The macOS ZIP the manifest names for version, when it is in Google's storage."""
    for line in manifest.splitlines():
        line = line.strip().removeprefix("- ")
        if line.startswith("url:"):
            url = line[4:].strip().strip("'\"")
            if url.startswith(f"{STORAGE}{version}-") and url.endswith("/Antigravity.zip") and ".." not in url:
                return url
    return None


def read_build(url, get=ranged):
    """(changelist, baseline) stamped into the release's language server."""
    tail = get(url, f"-{TAIL}").read(TAIL + 1)
    at = tail.rfind(b"PK\x05\x06")
    if at < 0 or len(tail) < at + 22:
        raise ValueError("no ZIP end record")
    size, offset = struct.unpack_from("<II", tail, at + 12)
    if 0xFFFFFFFF in (size, offset):
        raise ValueError("a ZIP64 release")
    directory = get(url, f"{offset}-{offset + size - 1}").read(size + 1)
    p = 0
    while directory[p:p + 4] == b"PK\x01\x02":
        method, = struct.unpack_from("<H", directory, p + 10)
        compressed, = struct.unpack_from("<I", directory, p + 20)
        name_len, extra_len, comment_len = struct.unpack_from("<HHH", directory, p + 28)
        local, = struct.unpack_from("<I", directory, p + 42)
        if directory[p + 46:p + 46 + name_len] == LANGUAGE_SERVER:
            break
        p += 46 + name_len + extra_len + comment_len
    else:
        raise ValueError("no language server in the release")
    if method not in (0, 8) or 0xFFFFFFFF in (compressed, local) or not compressed:
        raise ValueError(f"an unreadable language server entry (method {method})")
    header = get(url, f"{local}-{local + 29}").read(31)
    if not header.startswith(b"PK\x03\x04") or len(header) < 30:
        raise ValueError("no ZIP entry header")
    start = local + 30 + sum(struct.unpack_from("<HH", header, 26))
    stream = get(url, f"{start}-{start + compressed - 1}")
    inflate = zlib.decompressobj(-15) if method == 8 else None
    carry, read = b"", 0
    while chunk := stream.read(1 << 20):
        read += len(chunk)
        if read > compressed:
            raise ValueError("more language server than the ZIP directory says")
        hay = carry + (inflate.decompress(chunk) if inflate else chunk)
        if found := STAMP.search(hay):
            return found[1].decode(), found[2].decode()
        carry = hay[-128:]
    raise ValueError("no build stamp in the language server")


def main(argv):
    if argv:
        version, url = "?", argv[0]
    else:
        with urllib.request.urlopen(urllib.request.Request(MANIFEST, headers={"User-Agent": "electron-builder"}),
                                    timeout=20) as resp:
            manifest = resp.read(65_536).decode()
        version = next((line[8:].strip().strip("'\"") for line in manifest.splitlines() if line.startswith("version:")), "")
        url = release_url(manifest, version)
        if not url:
            print(f"The manifest names no release in Google's storage for {version!r}.", file=sys.stderr)
            return 1
    changelist, baseline = read_build(url)
    print(f'{version}: cl={changelist} (baseline {baseline})\nCHANGELISTS entry: ("{version}", "{changelist}")')
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
