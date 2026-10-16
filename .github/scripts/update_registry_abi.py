#!/usr/bin/env python3
"""Keep the `abi` map in plugin-registry.yaml current (bddkit/bddkit#122).

For each plugin: resolve the latest release through the `releases/latest`
redirect (no API call), read its `bddkit-plugin.json` and record the release
under the ABI it declares. Entries for other ABIs are never removed. Then check
the whole file: it parses, and every listed release exists and declares the ABI
it is listed under. Exit 1 on a failed check; the file is written only when the
checks pass. A release without the asset is skipped, not guessed at.

The registry is edited as text, not re-dumped, so its comments survive.
"""
import json
import os
import re
import sys
import urllib.error
import urllib.request

import yaml

GITHUB = os.environ.get("BDDKIT_GITHUB", "https://github.com")


def fetch(url):
    """Body of `url`, or None on 404. Redirects are followed."""
    try:
        with urllib.request.urlopen(url, timeout=30) as r:
            return r.geturl(), r.read()
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def latest(repo):
    got = fetch(f"{GITHUB}/{repo}/releases/latest")
    if got is None or "/tag/" not in got[0]:
        return None
    return got[0].rsplit("/tag/", 1)[1].removeprefix("v")


def declared_abi(repo, version):
    got = fetch(f"{GITHUB}/{repo}/releases/download/v{version}/bddkit-plugin.json")
    return None if got is None else int(json.loads(got[1])["abi"])


def splice(text, name, abi_map):
    """Replace (or add) the `abi:` block of plugin `name`, leaving the rest alone."""
    lines = text.splitlines(keepends=True)
    start = next(i for i, l in enumerate(lines) if re.fullmatch(rf"  - name: {re.escape(name)}\s*", l))
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("  - name:")), len(lines))
    block = [l for l in lines[start:end] if not re.match(r'    abi:|      "\d+":', l)]
    while block and not block[-1].strip():
        block.pop()
    block.append("    abi:\n")
    block += [f'      "{k}": {v}\n' for k, v in sorted(abi_map.items(), key=lambda kv: int(kv[0]))]
    return "".join(lines[:start] + block + lines[end:])


def main(path):
    text = open(path).read()
    problems = []
    for entry in yaml.safe_load(text)["plugin"]:
        name, repo = entry["name"], entry["repo"]
        abi_map = {str(k): str(v) for k, v in (entry.get("abi") or {}).items()}
        version = latest(repo)
        abi = declared_abi(repo, version) if version else None
        if abi is None:
            print(f"{name}: latest release of {repo} has no bddkit-plugin.json, not recorded")
        else:
            abi_map[str(abi)] = version
            text = splice(text, name, abi_map)

    # Checks run on the text that would be written, not on the old file.
    for entry in yaml.safe_load(text)["plugin"]:
        for abi, version in (entry.get("abi") or {}).items():
            got = declared_abi(entry["repo"], version)
            if got is None:
                problems.append(f"{entry['name']} {version}: release or bddkit-plugin.json does not exist")
            elif got != int(abi):
                problems.append(f"{entry['name']} {version}: listed under ABI {abi}, declares ABI {got}")
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    open(path, "w").write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "plugin-registry.yaml"))
