#!/usr/bin/env python3
"""Download a Gemma MLX model into the contained HF cache, with byte progress.

Run by the Rust setup() step with HF_HOME pointed at the app's data dir. Prints
``PROGRESS <downloaded_bytes> <total_bytes>`` lines to stdout (parsed by Rust to
drive the Settings progress bar) so "Set up engine" finishes only once the ~8 GB
model is actually on disk — making the "Ready" badge honest. Idempotent:
snapshot_download resumes/skips already-cached files.

Usage: python gemma_prefetch.py <repo_id>
"""
import os
import sys
import threading


def dir_size(path: str) -> int:
    total = 0
    for root, _dirs, files in os.walk(path):
        for name in files:
            try:
                total += os.path.getsize(os.path.join(root, name))
            except OSError:
                pass
    return total


def main() -> int:
    repo = sys.argv[1]
    from huggingface_hub import HfApi, snapshot_download

    # Total size = sum of all sibling file sizes in the repo.
    info = HfApi().model_info(repo, files_metadata=True)
    total = sum((s.size or 0) for s in (info.siblings or []))
    if total <= 0:
        total = 1  # avoid div-by-zero downstream; still emits a final 100%.

    # Where snapshot_download writes (HF_HOME/hub by default).
    hub_dir = os.path.join(os.environ.get("HF_HOME", ""), "hub")

    done = threading.Event()

    def report() -> None:
        # Poll the cache size ~1/s; counts in-progress *.incomplete blobs too,
        # which is fine for a monotonic-enough progress bar.
        while not done.wait(1.0):
            print(f"PROGRESS {dir_size(hub_dir)} {total}", flush=True)

    print(f"PROGRESS 0 {total}", flush=True)
    t = threading.Thread(target=report, daemon=True)
    t.start()
    try:
        snapshot_download(repo)
    finally:
        done.set()

    # Final exact 100% so the bar lands cleanly regardless of polling timing.
    print(f"PROGRESS {total} {total}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
