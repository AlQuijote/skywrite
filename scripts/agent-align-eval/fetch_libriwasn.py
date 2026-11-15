"""Fetch single recordings from the LibriWASN corpus without the 4.5 GB archives.

LibriWASN (Schmalenstroeer et al., 2023; https://zenodo.org/records/7960972,
CC BY 4.0) replays LibriSpeech meetings through loudspeakers in a real room and
records them with independent devices (smartphones, microphone arrays) whose
sampling clocks are not synchronized: real offsets and real clock drift
between real microphones. Each archive is a ZIP; this reads its central
directory and the chosen members with HTTP range requests (Python stdlib).

    python3 -I fetch_libriwasn.py list OUT_DIR [--archive LibriWASN_200_0L.zip]
    python3 -I fetch_libriwasn.py get OUT_DIR MEMBER [MEMBER ...] [--archive ...]

Downloads are written under OUT_DIR (never into the repository) with an
ATTRIBUTION.txt. Members are untrusted data: they are only decoded by FFmpeg.
"""

from __future__ import annotations

import argparse
import io
import json
import urllib.request
import zipfile
from pathlib import Path

RECORD = "https://zenodo.org/records/7960972/files/{name}?download=1"
LIMIT = 2 * 1024**3  # refuse members larger than 2 GiB


class RangeFile(io.RawIOBase):
    """A seekable read-only view of a remote file through HTTP Range requests."""

    def __init__(self, url: str):
        self.url = url
        self.pos = 0
        req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": "skywrite-eval"})
        with urllib.request.urlopen(req, timeout=60) as r:
            self.size = int(r.headers["Content-Length"])
        self.cache: dict[int, bytes] = {}

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, offset, whence=0):
        self.pos = {0: offset, 1: self.pos + offset, 2: self.size + offset}[whence]
        return self.pos

    def _chunk(self, index: int, size: int = 1 << 20) -> bytes:
        if index not in self.cache:
            lo = index * size
            hi = min(self.size, lo + size) - 1
            req = urllib.request.Request(self.url, headers={"Range": f"bytes={lo}-{hi}",
                                                            "User-Agent": "skywrite-eval"})
            with urllib.request.urlopen(req, timeout=120) as r:
                if r.status != 206 or not r.headers.get("Content-Range", "").startswith(f"bytes {lo}-{hi}/"):
                    raise OSError(f"server did not honour the range request ({r.status})")
                data = r.read(hi - lo + 1)
            if len(data) != hi - lo + 1:
                raise OSError(f"short range response: {len(data)} of {hi - lo + 1} bytes")
            if len(self.cache) > 64:
                self.cache.clear()
            self.cache[index] = data
        return self.cache[index]

    def readinto(self, buffer):
        n = min(len(buffer), self.size - self.pos)
        out = bytearray()
        while len(out) < n:
            index, within = divmod(self.pos + len(out), 1 << 20)
            piece = self._chunk(index)[within:within + n - len(out)]
            if not piece:
                raise OSError("range read made no progress")
            out += piece
        buffer[:n] = out
        self.pos += n
        return n


def open_zip(archive: str) -> zipfile.ZipFile:
    return zipfile.ZipFile(io.BufferedReader(RangeFile(RECORD.format(name=archive)), 1 << 20))


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("cmd", choices=["list", "get"])
    p.add_argument("out", type=Path)
    p.add_argument("members", nargs="*")
    p.add_argument("--archive", default="LibriWASN_200_0L.zip")
    a = p.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    z = open_zip(a.archive)
    if a.cmd == "list":
        listing = [{"name": i.filename, "bytes": i.file_size} for i in z.infolist()]
        (a.out / f"{a.archive}.listing.json").write_text(json.dumps(listing, indent=1))
        for item in listing:
            print(item["bytes"], item["name"])
        return
    for member in a.members:
        info = z.getinfo(member)
        if info.file_size > LIMIT:
            raise SystemExit(f"{member}: {info.file_size} bytes exceeds the limit")
        target = a.out / Path(member).name
        if target.exists() and target.stat().st_size == info.file_size:
            print(f"{target}: exists")
            continue
        with z.open(info) as src, open(target, "wb") as dst:
            while chunk := src.read(1 << 20):
                dst.write(chunk)
        print(f"{target}: {info.file_size} bytes")
    (a.out / "ATTRIBUTION.txt").write_text(
        "LibriWASN, J. Schmalenstroeer, T. Gburrek, R. Haeb-Umbach, Paderborn University, 2023.\n"
        "https://zenodo.org/records/7960972 - licensed CC BY 4.0 "
        "(https://creativecommons.org/licenses/by/4.0/).\n"
        f"Archive {a.archive}; members: {', '.join(a.members)}. Unmodified.\n")


if __name__ == "__main__":
    main()
