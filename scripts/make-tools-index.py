#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
# nix-eval-jobs --meta output on stdin, a tools.tsv (pkgs/mortar-index/src/compact.rs)
# on stdout: what nixpkgs builds for this system and cache.nixos.org really holds.
# Only names and hashes as plain text, so the list has no references and fetching it
# fetches nothing else.
#
# Every kept path is asked for at the cache. A list is no use if it offers what can't
# be downloaded, and one that shrank because the network failed mustn't be published,
# so a request that keeps failing stops the run, and so does a list under --min.
#
#   make-tools-index.py --system aarch64-linux --deny REGEX [--min N] [--offline] > tools.tsv
import argparse
import concurrent.futures
import http.client
import json
import re
import sys
import threading
import time

HEADER = "#mortar-tools\t1"
ATTR = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9._+-]{0,127}$")
GENERATION = re.compile(r"-\d+-link$")
PATH = re.compile(r"^/nix/store/([0-9a-df-np-sv-z]{32})-([A-Za-z0-9+._?=-]+)$")
MAX_DESCRIPTION = 300


def clean(text):
    return " ".join((text or "").split())[:MAX_DESCRIPTION]


def entry(job, system, deny):
    """A line's columns, or None when it isn't something to offer."""
    if "error" in job or job.get("system") != system:
        return None
    attr = job.get("attr", "")
    if not ATTR.match(attr) or GENERATION.search(attr):
        return None
    outputs = job.get("outputs") or {}
    meta = job.get("meta") or {}
    # what `nix profile install` takes: jq's program is in `bin`, not `out`
    wanted = list(meta.get("outputsToInstall") or []) + ["out"]
    path = next((outputs[o] for o in wanted if outputs.get(o)), None)
    match = PATH.match(path or "")
    if not match:
        return None
    digest, name = match.groups()
    if deny.search("-" + attr) or deny.search("-" + name):
        return None
    program = clean(meta.get("mainProgram")).replace(" ", "")
    return attr, name, digest, program, clean(meta.get("description"))


local = threading.local()


def cached(digest):
    """Is it on the cache? Raises when the cache won't say."""
    for attempt in range(6):
        try:
            if not hasattr(local, "conn"):
                local.conn = http.client.HTTPSConnection("cache.nixos.org", timeout=30)
            local.conn.request("HEAD", f"/{digest}.narinfo")
            response = local.conn.getresponse()
            response.read()
            if response.status == 200:
                return True
            if response.status == 404:
                return False
            problem = f"HTTP {response.status}"
        except (OSError, http.client.HTTPException) as e:
            problem = str(e)
        if hasattr(local, "conn"):
            local.conn.close()
            del local.conn
        time.sleep(2**attempt)
    raise RuntimeError(f"{digest}: {problem}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--system", required=True)
    parser.add_argument("--deny", required=True, help="a regex for names never to list")
    parser.add_argument("--min", type=int, default=0)
    parser.add_argument("--offline", action="store_true", help="for tests: ask no cache")
    parser.add_argument("--threads", type=int, default=64)
    args = parser.parse_args()
    deny = re.compile(args.deny)

    rows = {}
    for line in sys.stdin:
        row = entry(json.loads(line), args.system, deny)
        if row:
            rows[row[0]] = row

    digests = sorted({row[2] for row in rows.values()})
    if args.offline:
        held = set(digests)
    else:
        with concurrent.futures.ThreadPoolExecutor(args.threads) as pool:
            held = {d for d, ok in zip(digests, pool.map(cached, digests)) if ok}

    kept = [rows[a] for a in sorted(rows, key=lambda a: a.encode()) if rows[a][2] in held]
    print(f"{len(rows)} packages, {len(digests)} paths, {len(held)} on the cache, {len(kept)} listed", file=sys.stderr)
    if len(kept) < args.min:
        sys.exit(f"only {len(kept)} packages, fewer than {args.min}: not publishing that")
    print(HEADER)
    for row in kept:
        print("\t".join(row))


if __name__ == "__main__":
    main()
