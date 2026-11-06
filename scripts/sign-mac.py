#!/usr/bin/env python3
"""Persistent local code-signing identity. Private keys stay in login.keychain."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile

CONFIG = Path.home() / "Library/Application Support/agent-monitor/signing.json"
NAME = "Agent Monitor Local Signing"


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, **kwargs).stdout.strip()


def identity():
    if os.environ.get("AGENT_MONITOR_SIGNING_IDENTITY"):
        config = {"identity": os.environ["AGENT_MONITOR_SIGNING_IDENTITY"],
                "keychain": run("security", "default-keychain", "-d", "user").strip('"'),
                "distribution": True}
    else:
        if not CONFIG.exists():
            raise SystemExit("Run python3 scripts/sign-mac.py --setup once before packaging.")
        config = json.loads(CONFIG.read_text())
    if config["identity"] == "-":
        raise SystemExit("Ad-hoc signing is not allowed: it invalidates saved permissions on rebuild.")
    identities = run("security", "find-identity", "-p", "codesigning", config["keychain"])
    if config["identity"] not in identities:
        raise SystemExit("The saved signing identity is unavailable. Restore its keychain; do not create a replacement identity.")
    return config


def setup():
    if CONFIG.exists():
        identity()
        print("Existing signing identity is ready; no certificate was replaced.")
        return
    keychain = run("security", "default-keychain", "-d", "user").strip('"')
    found = subprocess.run(["security", "find-certificate", "-c", NAME, keychain], capture_output=True)
    if found.returncode == 0:
        raise SystemExit("A local certificate already exists without its signing.json. Recover that configuration instead of replacing the certificate.")
    # Temporary private material is owner-only and deleted after Keychain import.
    with tempfile.TemporaryDirectory(prefix="agent-monitor-signing-") as tmp:
        directory = Path(tmp)
        cfg = directory / "certificate.cnf"
        cfg.write_text("[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ext\n"
                       f"[dn]\nCN={NAME}\n[ext]\nbasicConstraints=critical,CA:FALSE\n"
                       "keyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n")
        key, cert, p12 = [directory / name for name in ("key.pem", "cert.pem", "identity.p12")]
        run("openssl", "req", "-new", "-x509", "-newkey", "rsa:3072", "-nodes",
            "-days", "3650", "-config", str(cfg), "-keyout", str(key), "-out", str(cert))
        password = secrets.token_urlsafe(32)
        env = dict(os.environ, AM_CERT_PASSWORD=password)
        run("openssl", "pkcs12", "-export", "-legacy", "-inkey", str(key), "-in", str(cert),
            "-out", str(p12), "-passout", "env:AM_CERT_PASSWORD", env=env)
        fingerprint = run("openssl", "x509", "-in", str(cert), "-noout", "-fingerprint", "-sha1").split("=", 1)[1].replace(":", "")
        run("security", "import", str(p12), "-k", keychain, "-P", password, "-T", "/usr/bin/codesign")
        CONFIG.parent.mkdir(parents=True, exist_ok=True)
        pending = CONFIG.with_suffix(".tmp")
        pending.write_text(json.dumps({"identity": fingerprint, "keychain": keychain}, indent=2) + "\n")
        os.replace(pending, CONFIG)
    print("Created persistent local identity in the login keychain.")


def sign(app):
    config = identity()
    marker = app / "Contents/Resources/local-signing"
    marker.parent.mkdir(parents=True, exist_ok=True)
    options = []
    if config.get("distribution"):
        marker.unlink(missing_ok=True)
        options = ["--timestamp"]
    else:
        marker.write_text("Local build: rebuild with the saved signing identity.\n")
        requirement = f'designated => identifier "com.agent.monitor" and certificate leaf = H"{config["identity"]}"'
        options = ["--timestamp=none", "--requirements", "=" + requirement]
    run("codesign", "--force", "--sign", config["identity"], "--keychain", config["keychain"],
        *options, str(app))
    run("codesign", "--verify", "--deep", "--strict", str(app))
    print("Signed and verified with persistent identity:", config["identity"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--setup", action="store_true")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("app", nargs="?", type=Path)
    args = parser.parse_args()
    try:
        if args.setup:
            setup()
        elif args.check:
            identity()
            print("Persistent signing identity is available.")
        elif args.app:
            sign(args.app)
        else:
            parser.error("provide --setup, --check, or an app bundle")
    except subprocess.CalledProcessError as error:
        # Never print argv: Keychain import arguments include a temporary password.
        raise SystemExit(error.stderr.strip() or "Signing command failed.") from None
