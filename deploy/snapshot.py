#!/usr/bin/env python3
"""Online snapshot of the cloud's data folder, without stopping the cloud.

    snapshot.py DATA_DIR ARCHIVE.tar.gz

The SQLite database is copied with SQLite's online backup API, which reads a consistent state
while the cloud keeps writing (it is never a raw copy of the live file or its -wal/-shm files).
The copy is checked with PRAGMA integrity_check before it goes into the archive. The other files
(signing.key, signing.pub, ban.key, media/) are write-once or replaced whole, and are packed as
they are. The archive has the same layout as backup.sh's (the folder's contents, `./...`), so
restore.sh takes it unchanged. It is written under a temporary name, synced and renamed, mode
0600, with ARCHIVE.tar.gz.sha256 next to it. Standard library only.
"""
import hashlib
import os
import shutil
import sqlite3
import sys
import tarfile
import tempfile

DATABASE = "anyplay-cloud.db"
SKIP_SUFFIXES = (".partial", "-wal", "-shm", "-journal")


def snapshot(data: str, archive: str) -> None:
    source = os.path.join(data, DATABASE)
    if not os.path.isfile(source):
        sys.exit(f"snapshot: {source} does not exist")
    out_dir = os.path.dirname(os.path.abspath(archive))
    os.makedirs(out_dir, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=out_dir, prefix=".snapshot-") as stage:
        copy = os.path.join(stage, DATABASE)
        src = sqlite3.connect(f"file:{source}?mode=ro", uri=True, timeout=30)
        try:
            dst = sqlite3.connect(copy)
            try:
                src.backup(dst)
                result = dst.execute("PRAGMA integrity_check").fetchall()
            finally:
                dst.close()
        finally:
            src.close()
        if result != [("ok",)]:
            sys.exit(f"snapshot: the copy fails its integrity check: {result[:3]}")
        for entry in sorted(os.listdir(data)):
            if entry == DATABASE or entry.endswith(SKIP_SUFFIXES):
                continue
            path = os.path.join(data, entry)
            if os.path.isdir(path):
                shutil.copytree(path, os.path.join(stage, entry), symlinks=False)
            elif os.path.isfile(path):
                shutil.copy2(path, os.path.join(stage, entry))
        partial = os.path.join(out_dir, "." + os.path.basename(archive) + ".partial")
        fd = os.open(partial, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        try:
            with os.fdopen(fd, "wb") as raw, tarfile.open(fileobj=raw, mode="w:gz") as tar:
                for entry in sorted(os.listdir(stage)):
                    tar.add(os.path.join(stage, entry), arcname="./" + entry)
                raw.flush()
                os.fsync(raw.fileno())
            os.replace(partial, archive)
        finally:
            if os.path.exists(partial):
                os.remove(partial)
    digest = hashlib.sha256()
    with open(archive, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    manifest = archive + ".sha256"
    fd = os.open(manifest, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(f"{digest.hexdigest()}  {os.path.basename(archive)}\n")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    snapshot(sys.argv[1], sys.argv[2])
    print(sys.argv[2])
