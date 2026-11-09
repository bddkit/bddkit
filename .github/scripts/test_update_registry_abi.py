"""Run: python3 .github/scripts/test_update_registry_abi.py (needs PyYAML)."""
import http.server
import os
import tempfile
import threading
import unittest

RELEASES = {"o/new": ("0.3.0", '{"abi": 2}'), "o/bad": ("0.1.0", '{"abi": 3}')}


class Github(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        parts = self.path.strip("/").split("/")
        repo = "/".join(parts[:2])
        version, asset = RELEASES[repo]
        if parts[2:] == ["releases", "latest"]:
            self.send_response(302)
            self.send_header("Location", f"/{repo}/releases/tag/v{version}")
            self.end_headers()
        elif parts[2:4] == ["releases", "tag"]:
            self.send_response(200)
            self.end_headers()
        elif parts[-1] == "bddkit-plugin.json" and parts[-2] == f"v{version}":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(asset.encode())
        elif (repo, parts[-2]) == ("o/new", "v0.0.9"):  # an older release, ABI 1
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"abi": 1}')
        else:
            self.send_response(404)
            self.end_headers()


REGISTRY = """# keep me
plugin:
  - name: new
    repo: o/new
    description: d
    abi:
      "1": 0.0.9
  - name: bad
    repo: o/bad
    description: d
"""


class Job(unittest.TestCase):
    def run_job(self, registry, check=False):
        server = http.server.HTTPServer(("127.0.0.1", 0), Github)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        os.environ["BDDKIT_GITHUB"] = f"http://127.0.0.1:{server.server_port}"
        import importlib, sys
        sys.path.insert(0, os.path.dirname(__file__))
        import update_registry_abi as job
        importlib.reload(job)
        with tempfile.NamedTemporaryFile("w+", suffix=".yaml") as f:
            f.write(registry)
            f.flush()
            code = job.main(f.name, check)
            return code, open(f.name).read()

    def test_new_release_is_recorded_and_older_abi_kept(self):
        global RELEASES
        saved = dict(RELEASES)
        RELEASES = {"o/new": saved["o/new"], "o/bad": ("0.1.0", '{"abi": 2}')}
        try:
            code, out = self.run_job(REGISTRY)
        finally:
            RELEASES = saved
        self.assertEqual(code, 0)
        self.assertIn('      "1": 0.0.9\n      "2": 0.3.0\n', out)
        self.assertIn('  - name: bad\n    repo: o/bad\n    description: d\n    abi:\n      "2": 0.1.0\n', out)
        self.assertTrue(out.startswith("# keep me\n"))

    def test_a_listed_release_declaring_another_abi_is_reported_not_recorded(self):
        listed = REGISTRY.replace("    repo: o/bad\n    description: d\n",
                                  '    repo: o/bad\n    description: d\n    abi:\n      "2": 0.1.0\n')
        code, out = self.run_job(listed)
        self.assertEqual(code, 1)
        self.assertNotIn("0.3.0", out)

    def with_releases(self, releases, registry, check=False):
        global RELEASES
        saved = dict(RELEASES)
        RELEASES = releases
        try:
            return self.run_job(registry, check)
        finally:
            RELEASES = saved

    def one(self, groups_line, asset):
        registry = f"plugin:\n  - name: p\n    repo: o/p\n{groups_line}    description: d\n"
        return self.with_releases({"o/p": ("1.0.0", asset)}, registry)

    def test_equal_groups_as_a_set_pass(self):
        code, _ = self.one("    groups: [a, b]\n", '{"abi": 4, "groups": ["b", "a"]}')
        self.assertEqual(code, 0)

    def test_differing_groups_fail_and_write_nothing(self):
        code, out = self.one("    groups: [a]\n", '{"abi": 4, "groups": ["a", "b"]}')
        self.assertEqual(code, 1)
        self.assertNotIn("1.0.0", out)

    def test_a_release_without_groups_is_not_checked(self):
        code, _ = self.one("    groups: [a]\n", '{"abi": 3}')
        self.assertEqual(code, 0)

    def test_a_release_with_groups_and_an_entry_without_fails(self):
        code, _ = self.one("", '{"abi": 4, "groups": ["a"]}')
        self.assertEqual(code, 1)

    def test_groups_that_are_not_a_list_fail(self):
        code, _ = self.one("    groups: [a]\n", '{"abi": 4, "groups": "a"}')
        self.assertEqual(code, 1)

    def test_check_mode_fails_on_differing_groups_too(self):
        registry = "plugin:\n  - name: p\n    repo: o/p\n    groups: [a]\n    description: d\n"
        code, _ = self.with_releases({"o/p": ("1.0.0", '{"abi": 4, "groups": ["b"]}')}, registry, check=True)
        self.assertEqual(code, 1)

    def test_check_mode_writes_nothing(self):
        registry = "plugin:\n  - name: p\n    repo: o/p\n    description: d\n"
        code, out = self.with_releases({"o/p": ("1.0.0", '{"abi": 3}')}, registry, check=True)
        self.assertEqual(code, 0)
        self.assertEqual(out, registry)


if __name__ == "__main__":
    unittest.main()
