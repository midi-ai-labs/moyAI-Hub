"""Standalone package behavior, without building or launching the application."""
import importlib.util
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile


REPO = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("hub_package_release", REPO / "scripts/package-release.py")
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)
COMMIT = "1" * 40
DESKTOP_COMMIT = "2" * 40


class PackageReleaseTests(unittest.TestCase):
    def setUp(self):
        parent = REPO.parent / "project_sandbox/hub-release-packaging-tests"
        parent.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="package-", dir=parent)
        self.root = Path(self.temporary.name).resolve()
        self.assertTrue(self.root.is_relative_to(parent.resolve()))
        self.addCleanup(self.temporary.cleanup)
        self.repo = self.root / "Hub"
        self.repo.mkdir()
        for directory in ("scripts", "docs/release", "src", "logo"):
            (self.repo / directory).mkdir(parents=True)
        seed = {"schema": 1, "settings": {"local_port": 9080, "remote": {"mode": "off"}}}
        (self.repo / "web-management.json").write_bytes(package.json_bytes(seed))
        for name in ("Start-Hub.cmd", "Stop-Hub.cmd", "Setup-Hub.cmd", "Uninstall-Hub.cmd"):
            (self.repo / "scripts" / name).write_bytes(b"@echo off\r\n")
        for name in ("Setup-Hub.ps1", "component-install.ps1", "Setup-Component.ps1", "sandbox-service.ps1"):
            (self.repo / "scripts" / name).write_text("# product installer fixture\n", encoding="utf-8")
        (self.repo / "LICENSE").write_text("product license", encoding="utf-8")
        (self.repo / "README.md").write_text(
            "[Guide](docs/guide.md)\n![Management](logo/moyai-hub-screenshot.jpg)", encoding="utf-8",
        )
        (self.repo / "logo/moyai-hub-screenshot.jpg").write_bytes(b"screenshot fixture")
        (self.repo / "docs/guide.md").write_text("[Source](../src/)", encoding="utf-8")
        (self.repo / "docs/release/v1.0.0.md").write_text(
            "# moyAI Hub v1.0.0\n\n[Guide](../guide.md)\n", encoding="utf-8",
        )
        self.exe = self.root / "hub.exe"
        pe = bytearray(128)
        pe[:2] = b"MZ"
        struct.pack_into("<I", pe, 60, 64)
        pe[64:68] = b"PE\0\0"
        struct.pack_into("<H", pe, 68, 0x8664)
        self.exe.write_bytes(pe)
        self.exe.with_name("moyai-cleanup-hub.exe").write_bytes(pe)
        self.notices = self.root / "notices.txt"
        self.notices.write_text("existing dependency notices", encoding="utf-8")

    def files(self):
        return package.payload(self.repo, self.exe, self.notices, "1.0.0", COMMIT, DESKTOP_COMMIT)

    def test_seed_launchers_notices_docs_and_archive_have_matching_integrity_records(self):
        files, rewrites = self.files()
        self.assertIn("web-management.json", files)
        self.assertIn("Start-Hub.cmd", files)
        self.assertIn("Setup-Hub.cmd", files)
        self.assertIn("Uninstall-Hub.cmd", files)
        self.assertEqual(files["moyai-cleanup-hub.exe"], self.exe.with_name("moyai-cleanup-hub.exe").read_bytes())
        self.assertEqual(files["logo/moyai-hub-screenshot.jpg"], b"screenshot fixture")
        self.assertIn(b"](logo/moyai-hub-screenshot.jpg)", files["README.md"])
        self.assertEqual(files["THIRD_PARTY_NOTICES.txt"], self.notices.read_bytes())
        self.assertIn(f"/tree/{COMMIT}/src)".encode(), files["docs/guide.md"])
        self.assertIn(b"](docs/guide.md)", files["RELEASE_NOTES.md"])
        identity = {"version": "1.0.0", "git_commit": COMMIT, "source_clean": True,
                    "sha256": package.digest(files["moyai-hub.exe"])}
        result = package.write_package(self.root / "out", "1.0.0", files, identity, rewrites)
        folder = Path(result["directory"])
        manifest = json.loads((folder / "manifest.json").read_bytes())
        deployment = json.loads((folder / "deployment.json").read_bytes())
        self.assertEqual(deployment["components"], {"Hub": {"version": "1.0.0"}})
        self.assertEqual(deployment["product"], "moyai-hub-windows")
        for item in deployment["files"]:
            self.assertEqual(item["component"], "Hub")
            self.assertEqual(package.digest((folder / item["path"]).read_bytes()), item["sha256"])
        self.assertFalse(manifest["prerelease"])
        for item in manifest["files"]:
            body = (folder / item["path"]).read_bytes()
            self.assertEqual(package.digest(body), item["sha256"])
            self.assertEqual(len(body), item["size_bytes"])
        for line in (folder / "SHA256SUMS.txt").read_text(encoding="utf-8").splitlines():
            checksum, relative = line.split("  ", 1)
            self.assertEqual(package.digest((folder / relative).read_bytes()), checksum)
        with zipfile.ZipFile(result["zip"]) as zipped:
            self.assertIsNone(zipped.testzip())
            self.assertEqual(zipped.read(folder.name + "/web-management.json"), files["web-management.json"])
            self.assertEqual(zipped.read(folder.name + "/logo/moyai-hub-screenshot.jpg"), files["logo/moyai-hub-screenshot.jpg"])
        with self.assertRaisesRegex(ValueError, "already exists"):
            package.write_package(self.root / "out", "1.0.0", files, identity, rewrites)

    def test_missing_or_automatic_port_seed_is_rejected_before_output(self):
        seed = self.repo / "web-management.json"
        seed.write_text('{"schema":1,"settings":{"local_port":0,"remote":{"mode":"off"}}}', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "fixed port 9080"):
            self.files()
        seed.unlink()
        with self.assertRaises(FileNotFoundError):
            self.files()

    def test_desktop_links_use_pinned_commit_without_a_sibling_checkout(self):
        desktop = self.root / "moyAI"
        self.assertFalse(desktop.exists())
        (self.repo / "docs/guide.md").write_text(
            "[Desktop guide](../../moyAI/docs/user/windows-setup.md#uninstall)\n"
            "[Desktop directory](../../moyAI/docs/design/)\n"
            "[Desktop license](../../moyAI/LICENSE)\n"
            "[Desktop root](../../moyAI/)\n",
            encoding="utf-8",
        )
        files, rewrites = self.files()
        guide = files["docs/guide.md"].decode("utf-8")
        self.assertIn(f"{package.DESKTOP_URL}/blob/{DESKTOP_COMMIT}/docs/user/windows-setup.md#uninstall)", guide)
        self.assertIn(f"{package.DESKTOP_URL}/tree/{DESKTOP_COMMIT}/docs/design)", guide)
        self.assertIn(f"{package.DESKTOP_URL}/blob/{DESKTOP_COMMIT}/LICENSE)", guide)
        self.assertIn(f"{package.DESKTOP_URL}/tree/{DESKTOP_COMMIT})", guide)
        self.assertEqual(sum(item["file"] == "docs/guide.md" for item in rewrites), 4)
        self.assertFalse(desktop.exists())
        (self.repo / "docs/guide.md").write_text("[Missing Hub source](../src/missing.rs)", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Missing documentation target"):
            self.files()

    def test_cleanup_and_component_installer_are_required_package_inputs(self):
        self.exe.with_name("moyai-cleanup-hub.exe").unlink()
        with self.assertRaises(FileNotFoundError):
            self.files()
        self.exe.with_name("moyai-cleanup-hub.exe").write_bytes(self.exe.read_bytes())
        (self.repo / "scripts/component-install.ps1").unlink()
        with self.assertRaises(FileNotFoundError):
            self.files()

    def test_unresolved_documentation_and_empty_notices_are_rejected(self):
        (self.repo / "README.md").write_text("[Missing](missing.md)", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Missing documentation"):
            self.files()
        self.notices.write_bytes(b"")
        with self.assertRaisesRegex(ValueError, "notices must not be empty"):
            self.files()

    def test_release_log_rejects_foreign_version_and_incomplete_build(self):
        log = self.root / "build.jsonl"
        artifact = {"reason": "compiler-artifact", "target": {"name": "moyai-hub", "kind": ["bin"]},
                    "executable": str(self.exe), "manifest_path": str(self.repo / "Cargo.toml"),
                    "package_id": "path+file:///Hub#moyai-hub@1.0.0",
                    "profile": {"opt_level": "3", "test": False, "debug_assertions": False}}
        completed = {"reason": "build-finished", "success": True}
        log.write_bytes(package.json_bytes(artifact).replace(b"\n", b"") + b"\n" + json.dumps(completed).encode())
        self.assertEqual(package.release_artifact(log, self.repo, "1.0.0"), artifact)
        with self.assertRaisesRegex(ValueError, "this source version"):
            package.release_artifact(log, self.repo, "0.1.0")
        log.write_text(json.dumps(artifact), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "successful completed build"):
            package.release_artifact(log, self.repo, "1.0.0")

    def test_dirty_source_is_rejected_before_build_inputs_or_output_are_read(self):
        argv = ["package-release.py", "--build-log", "missing", "--notices", "missing",
                "--expected-commit", COMMIT, "--desktop-source-commit", DESKTOP_COMMIT]
        with patch.object(sys, "argv", argv), patch.object(package, "git", side_effect=[COMMIT, " M README.md"]):
            with self.assertRaisesRegex(ValueError, "clean source"):
                package.main()


if __name__ == "__main__":
    unittest.main()
