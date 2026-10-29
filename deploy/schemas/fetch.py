"""Fetch the schemas that kubeconform reads, at the commits pinned in sources.json, and write them with a lock (09/T3; S19).

Developer machine only. CI never fetches: it checks the committed files against the lock (verify.mjs), so a schema cannot change
without a diff that a person reads.

    python3 -B -I deploy/schemas/fetch.py         # fetch everything, write the schemas and schemas.lock.json
    python3 -B -I deploy/schemas/fetch.py --check # fetch again and compare with the lock and the files; write nothing

Needs PyYAML (to read the CRD files). Only raw.githubusercontent.com is contacted, over HTTPS, with a timeout, a size limit and a
bounded number of attempts. Every file's sha256 comes from the bytes downloaded here; a file whose sha256 differs from the one
already in the lock is reported, never silently replaced.
"""

import hashlib
import importlib.util
import json
import os
import sys
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

# Loaded by path: isolated mode (-I) keeps the script's directory off sys.path, and that is kept.
_spec = importlib.util.spec_from_file_location("crd2schema", os.path.join(HERE, "crd2schema.py"))
crd2schema = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(crd2schema)
HOST = "https://raw.githubusercontent.com"
MAX_BYTES = 16 * 1024 * 1024
ATTEMPTS = 3
TIMEOUT_SECONDS = 60


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def fetch(url):
    if not url.startswith(HOST + "/"):
        raise SystemExit(f"fetch.py only contacts {HOST}: {url}")
    last = None
    for attempt in range(ATTEMPTS):
        try:
            with urllib.request.urlopen(url, timeout=TIMEOUT_SECONDS) as response:  # noqa: S310 (https, checked above)
                data = response.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                raise SystemExit(f"{url} is larger than {MAX_BYTES} bytes")
            return data
        except urllib.error.HTTPError as error:
            if error.code in (400, 401, 403, 404):
                raise SystemExit(f"{url}: HTTP {error.code}") from error
            last = error
        except (urllib.error.URLError, TimeoutError) as error:
            last = error
        time.sleep(1 + attempt)
    raise SystemExit(f"{url}: {last}")


def render_json(schema):
    return (json.dumps(schema, indent=1, sort_keys=True) + "\n").encode()


def planned(sources):
    """Yield (relative path, bytes, source url, source bytes, how) for every schema the sources describe."""
    import yaml

    k8s = sources["kubernetes"]
    for kind in k8s["kinds"]:
        url = f"{HOST}/{k8s['repo']}/{k8s['commit']}/{k8s['directory']}/{kind}.json"
        data = fetch(url)
        json.loads(data)
        yield f"k8s/{k8s['directory']}/{kind}.json", data, url, data, "copied"
    for project in sources["crds"]:
        for path in project["files"]:
            url = f"{HOST}/{project['repo']}/{project['commit']}/{path}"
            raw = fetch(url)
            documents = [d for d in yaml.safe_load_all(raw) if d]
            converted = [item for d in documents for item in crd2schema.convert_crd(d)]
            if not converted:
                raise SystemExit(f"{url}: no CustomResourceDefinition with a schema in it")
            for group, kind, version, schema in converted:
                yield f"crds/{crd2schema.schema_path(group, kind, version)}", render_json(schema), url, raw, "crd2schema.py"
    for project in sources["ready"]:
        for path in project["files"]:
            url = f"{HOST}/{project['repo']}/{project['commit']}/{path}"
            data = fetch(url)
            json.loads(data)
            yield f"crds/{path}", data, url, data, "copied"


def main(argv):
    check = argv == ["--check"]
    if argv and not check:
        raise SystemExit(__doc__)
    with open(os.path.join(HERE, "sources.json"), encoding="utf-8") as f:
        sources = json.load(f)
    lock_path = os.path.join(HERE, "schemas.lock.json")
    old = {}
    if os.path.exists(lock_path):
        with open(lock_path, encoding="utf-8") as f:
            old = json.load(f).get("files", {})
    files = {}
    problems = []
    for relative, data, url, raw, how in planned(sources):
        entry = {"sha256": sha256(data), "bytes": len(data), "source": url, "sourceSha256": sha256(raw), "how": how}
        if relative in files:
            raise SystemExit(f"two sources make {relative}")
        files[relative] = entry
        before = old.get(relative)
        if before and before["sourceSha256"] == entry["sourceSha256"] and before["sha256"] != entry["sha256"]:
            problems.append(f"{relative}: the same source now converts to different bytes")
        if before and before["source"] == entry["source"] and before["sourceSha256"] != entry["sourceSha256"]:
            problems.append(f"{relative}: the file at a pinned commit changed (sha256 {before['sourceSha256']} -> {entry['sourceSha256']})")
        target = os.path.join(HERE, relative)
        if check:
            if not os.path.exists(target) or open(target, "rb").read() != data:
                problems.append(f"{relative}: differs from the committed file")
        else:
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(target, "wb") as f:
                f.write(data)
    if check:
        if set(old) != set(files):
            problems.append(f"the lock lists different files: {sorted(set(old) ^ set(files))}")
        for problem in problems:
            print(f"fetch.py --check: {problem}", file=sys.stderr)
        if problems:
            return 1
        print(f"fetch.py --check: {len(files)} schemas equal the lock")
        return 0
    if problems:
        for problem in problems:
            print(f"fetch.py: {problem}", file=sys.stderr)
        return 1
    # Files that the sources no longer make are removed, so the directory is exactly the lock.
    for relative in sorted(set(old) - set(files)):
        stale = os.path.join(HERE, relative)
        if os.path.exists(stale):
            os.remove(stale)
    lock = {"schema": 1, "kubernetes": sources["kubernetes"]["version"], "files": dict(sorted(files.items()))}
    with open(lock_path, "w", encoding="utf-8") as f:
        f.write(json.dumps(lock, indent=1, sort_keys=True) + "\n")
    total = sum(e["bytes"] for e in files.values())
    print(f"fetch.py: {len(files)} schemas, {total} bytes, lock written")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
