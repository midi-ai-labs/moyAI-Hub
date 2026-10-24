"""Assemble a Windows Hub release from a clean, already built source commit."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tomllib
from urllib.parse import unquote, urlsplit
import zipfile


REPO = Path(__file__).resolve().parent.parent
HUB_URL = "https://github.com/midi-ai-labs/moyAI-Hub"
DESKTOP_URL = "https://github.com/midi-ai-labs/moyAI"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def git(*args):
    return subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True,
        text=True, encoding="utf-8",
    ).stdout.strip()


def source_version(repo):
    cargo = tomllib.loads((repo / "Cargo.toml").read_text(encoding="utf-8"))
    lock = tomllib.loads((repo / "Cargo.lock").read_text(encoding="utf-8"))
    npm = json.loads((repo / "package.json").read_text(encoding="utf-8"))
    npm_lock = json.loads((repo / "package-lock.json").read_text(encoding="utf-8"))
    version = cargo["package"]["version"]
    own = [p["version"] for p in lock["package"] if p["name"] == "moyai-hub"]
    if own != [version] or any(v != version for v in (
        npm["version"], npm_lock["version"], npm_lock["packages"][""]["version"],
    )):
        raise ValueError("Cargo and npm package versions must all match")
    return version


def release_artifact(log, repo, version):
    messages = []
    for line in log.read_text(encoding="utf-8-sig").splitlines():
        try:
            messages.append(json.loads(line))
        except ValueError:
            continue
    if not any(m.get("reason") == "build-finished" and m.get("success") is True for m in messages):
        raise ValueError("Cargo log does not contain a successful completed build")
    artifacts = [m for m in messages if m.get("reason") == "compiler-artifact"
                 and m.get("target", {}).get("name") == "moyai-hub"
                 and "bin" in m.get("target", {}).get("kind", [])
                 and m.get("executable")]
    if len(artifacts) != 1:
        raise ValueError("Cargo log must identify exactly one Hub executable")
    artifact = artifacts[0]
    if (Path(artifact["manifest_path"]).resolve() != (repo / "Cargo.toml").resolve()
            or not artifact["package_id"].endswith("@" + version)
            or artifact["profile"]["opt_level"] != "3"
            or artifact["profile"]["test"]
            or artifact["profile"]["debug_assertions"]):
        raise ValueError("Cargo artifact must be this source version's release executable")
    return artifact


def x64_executable(path):
    body = path.read_bytes()
    if len(body) < 64 or body[:2] != b"MZ":
        raise ValueError("Hub executable is not a Windows PE file")
    offset = struct.unpack_from("<I", body, 60)[0]
    if (offset + 6 > len(body) or body[offset:offset + 4] != b"PE\0\0"
            or struct.unpack_from("<H", body, offset + 4)[0] != 0x8664):
        raise ValueError("Hub executable must target Windows x64")
    return body


def document_bytes(source, destination, documents, repo, commit, desktop, desktop_commit, rewrites):
    def replace(match):
        target = match.group(1)
        url = urlsplit(target)
        if url.scheme or url.netloc or not url.path:
            return match.group(0)
        resolved = (source.parent / unquote(url.path)).resolve()
        if not resolved.exists():
            raise ValueError(f"Missing documentation target: {source.name}: {target}")
        if resolved in documents:
            replacement = Path(os.path.relpath(documents[resolved], destination.parent)).as_posix()
            if url.query:
                replacement += "?" + url.query
        else:
            if resolved.is_relative_to(repo.resolve()):
                relative, base, revision = resolved.relative_to(repo.resolve()), HUB_URL, commit
            elif resolved.is_relative_to(desktop.resolve()):
                relative, base, revision = resolved.relative_to(desktop.resolve()), DESKTOP_URL, desktop_commit
            else:
                raise ValueError(f"Documentation link leaves product repositories: {source.name}: {target}")
            view = "tree" if resolved.is_dir() else "blob"
            replacement = f"{base}/{view}/{revision}/{relative.as_posix()}"
        if url.fragment:
            replacement += "#" + url.fragment
        if replacement != target:
            rewrites.append({"file": destination.as_posix(), "original": target, "packaged": replacement})
        return "](" + replacement + ")"

    return re.sub(r"\]\(([^)]+)\)", replace, source.read_text(encoding="utf-8-sig")).encode("utf-8")


def payload(repo, executable, notices, version, commit, desktop_commit):
    seed = (repo / "web-management.json").read_bytes()
    initial = json.loads(seed)
    if initial != {"schema": 1, "settings": {"local_port": 9080, "remote": {"mode": "off"}}}:
        raise ValueError("Release initial Web settings must use fixed port 9080 and remote off")
    files = {"moyai-hub.exe": x64_executable(executable), "web-management.json": seed}
    for source in (repo / "scripts/Start-Hub.cmd", repo / "scripts/Stop-Hub.cmd", repo / "LICENSE"):
        files[source.name] = source.read_bytes()
    files["THIRD_PARTY_NOTICES.txt"] = notices.read_bytes()
    if not files["THIRD_PARTY_NOTICES.txt"].strip():
        raise ValueError("Third-party notices must not be empty")
    sources = [repo / "README.md", *sorted((repo / "docs").rglob("*.md"))]
    images = [repo / "logo/moyai-hub-screenshot.jpg"]
    documents = {p.resolve(): p.relative_to(repo) for p in [*sources, *images]}
    for source in images:
        files[source.relative_to(repo).as_posix()] = source.read_bytes()
    rewrites = []
    for source in sources:
        destination = source.relative_to(repo)
        files[destination.as_posix()] = document_bytes(
            source, destination, documents, repo, commit, repo.parent / "moyAI", desktop_commit, rewrites,
        )
    notes = repo / f"docs/release/v{version}.md"
    if notes.read_text(encoding="utf-8-sig").splitlines()[0] != f"# moyAI Hub v{version}":
        raise ValueError("Release notes heading does not match the package version")
    files["RELEASE_NOTES.md"] = document_bytes(
        notes, Path("RELEASE_NOTES.md"), documents, repo, commit, repo.parent / "moyAI", desktop_commit, rewrites,
    )
    return files, rewrites


def write_package(output, version, files, identity, rewrites, prerelease=False):
    name = f"moyAI-Hub-v{version}-windows-x86_64"
    directory, archive = output / name, output / (name + ".zip")
    manifest_path, checksum = output / (name + ".manifest.json"), output / (name + ".zip.sha256")
    if any(p.exists() for p in (directory, archive, manifest_path, checksum)):
        raise ValueError("Release output already exists; select a new output directory")
    files = dict(files)
    files["build-identity.json"] = json_bytes(identity)
    manifest = {"schema": "moyai-hub.standalone-release.v1", "name": name, **identity,
                "prerelease": prerelease,
                "files": [{"path": path, "sha256": digest(body), "size_bytes": len(body)}
                          for path, body in sorted(files.items())],
                "documentation_link_rewrites": rewrites, "runtime_files_bundled": False,
                "packaged_utc": datetime.now(timezone.utc).isoformat()}
    files["manifest.json"] = json_bytes(manifest)
    files["SHA256SUMS.txt"] = "".join(
        f"{digest(body)}  {path}\n" for path, body in sorted(files.items())
    ).encode("utf-8")
    directory.mkdir(parents=True)
    for relative, body in files.items():
        destination = directory / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(body)
    with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zipped:
        for relative in sorted(files):
            zipped.write(directory / relative, name + "/" + relative)
    manifest_path.write_bytes(files["manifest.json"])
    checksum.write_text(f"{digest(archive.read_bytes())}  {archive.name}\n", encoding="utf-8")
    return {"directory": str(directory), "zip": str(archive), "zip_sha256": digest(archive.read_bytes()),
            "manifest": str(manifest_path), "binary_sha256": identity["sha256"], "files": len(files)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-log", type=Path, required=True, help="Successful Cargo --message-format=json release build log")
    parser.add_argument("--notices", type=Path, required=True, help="Verified notices for this build's existing dependencies")
    parser.add_argument("--expected-commit", required=True, help="Clean merged source commit selected for release")
    parser.add_argument("--desktop-source-commit", required=True, help="Desktop source revision used only to pin documentation links")
    parser.add_argument("--output", type=Path, default=REPO.parent / "project_sandbox/releases")
    parser.add_argument("--version", help="Expected version; defaults to Cargo.toml")
    parser.add_argument("--prerelease", action="store_true")
    args = parser.parse_args()
    for revision in (args.expected_commit, args.desktop_source_commit):
        if not re.fullmatch(r"[a-f0-9]{40}", revision):
            raise ValueError("Source revisions must be full lowercase commit hashes")
    commit = git("rev-parse", "HEAD")
    if commit != args.expected_commit or git("status", "--porcelain", "--untracked-files=all"):
        raise ValueError("Release requires the expected commit and a clean source worktree")
    version = source_version(REPO)
    if args.version and args.version != version:
        raise ValueError("Requested version does not match the source")
    artifact = release_artifact(args.build_log, REPO, version)
    files, rewrites = payload(REPO, Path(artifact["executable"]), args.notices, version, commit, args.desktop_source_commit)
    identity = {"version": version, "git_commit": commit, "source_clean": True,
                "target": "windows-x86_64", "sha256": digest(files["moyai-hub.exe"]),
                "size_bytes": len(files["moyai-hub.exe"]), "source_branch": git("branch", "--show-current"),
                "compiler_profile": artifact["profile"], "compiler_features": artifact["features"],
                "compiler_artifact_log_sha256": digest(args.build_log.read_bytes()),
                "built_from_current_source": True, "frontend_embedded_in_executable": True,
                "documentation_desktop_commit": args.desktop_source_commit,
                "runtime_requirements": ["Windows x64", "x64 VC++ runtime already installed", "Supported web browser"],
                "webview2_required_by_hub": False}
    if git("rev-parse", "HEAD") != commit or git("status", "--porcelain", "--untracked-files=all"):
        raise ValueError("Source changed during package preparation")
    print(json.dumps(write_package(args.output.resolve(), version, files, identity, rewrites, args.prerelease), ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Hub package failed: {error}", file=sys.stderr)
        sys.exit(1)
