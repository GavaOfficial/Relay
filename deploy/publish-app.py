#!/usr/bin/env python3
"""Pubblica l'app (e il suo ffmpeg) sul server: sul sito compare il pulsante di download e l'app si
aggiorna da sola.

    python deploy/publish-app.py                       # app: target/release/relay-app.exe, versione di Cargo.toml
    python deploy/publish-app.py --notes "Audio e ripresa dopo un crash"
    python deploy/publish-app.py --ffmpeg C:\\percorso\\ffmpeg.exe   # anche ffmpeg (una volta, o quando cambia)
    python deploy/publish-app.py --no-app --ffmpeg ffmpeg.exe      # solo ffmpeg
    python deploy/publish-app.py --local ./dati        # prova: scrive in ./dati/app invece che sulla VPS

Sulla VPS (SFTP del server API) servono le variabili SFTP_USER e SFTP_PASS; SFTP_HOST e SFTP_PORT
sono opzionali (node2.gavatech.org, 2022). La firma usa la chiave privata in
~/.relay-signing/update-key.pem (o RELAY_SIGNING_KEY): tienila al sicuro e non caricarla mai sul
server. Chi ha quella chiave puo' far installare qualsiasi programma agli utenti.

In <dati>/app/ vengono scritti: relay-app-<versione>.exe + latest.json (app) e
ffmpeg-<versione>.zip + ffmpeg.json (ffmpeg). Il server legge i .json.
"""
import argparse
import datetime
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

def workspace_version() -> str:
    text = open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8").read()
    return re.search(r"\[workspace\.package\][^\[]*?version\s*=\s*\"([^\"]+)\"", text, re.S).group(1)

def ffmpeg_version(exe: str) -> str:
    out = subprocess.run([exe, "-version"], capture_output=True, text=True).stdout
    m = re.search(r"ffmpeg version n?(\d+(?:\.\d+)+)", out)
    if not m:
        raise SystemExit("non riesco a leggere la versione di ffmpeg: usa --ffmpeg-version")
    return m.group(1)

def sign(kind: str, version: str, sha256: str) -> str:
    from cryptography.hazmat.primitives import serialization

    key_path = os.environ.get("RELAY_SIGNING_KEY") or os.path.join(os.path.expanduser("~"), ".relay-signing", "update-key.pem")
    key = serialization.load_pem_private_key(open(key_path, "rb").read(), None)

    return key.sign(f"{kind}|{version}|{sha256.lower()}".encode()).hex()

class Target:
    """Dove si scrive: una cartella locale (prove) o la cartella dati del server via SFTP."""

    def __init__(self, local):
        self.local = local
        if local:
            os.makedirs(os.path.join(local, "app"), exist_ok=True)
            return
        import paramiko

        host, port = os.environ.get("SFTP_HOST", "node2.gavatech.org"), int(os.environ.get("SFTP_PORT", "2022"))
        self.t = paramiko.Transport((host, port))
        self.t.connect(username=os.environ["SFTP_USER"], password=os.environ["SFTP_PASS"])
        self.t.set_keepalive(15)
        self.s = paramiko.SFTPClient.from_transport(self.t)
        top = {e.filename for e in self.s.listdir_attr(".")}
        assert "Cargo.toml" in top and "api" in top, "questo non sembra il server API: mi fermo"
        for d in ("data", "data/app"):
            try:
                self.s.stat(d)
            except IOError:
                self.s.mkdir(d)

    def put(self, name: str, content: bytes, verify: bool = False) -> None:
        if self.local:
            open(os.path.join(self.local, "app", name), "wb").write(content)
            return
        remote, tmp = f"data/app/{name}", f"data/app/{name}.part"
        with self.s.file(tmp, "wb") as f:
            f.set_pipelined(True)
            for i in range(0, len(content), 1 << 20):
                f.write(content[i : i + (1 << 20)])
        assert self.s.stat(tmp).st_size == len(content), "dimensione diversa dopo l'upload"
        if verify:
            h = hashlib.sha256()
            with self.s.file(tmp, "rb") as f:
                f.prefetch()
                while chunk := f.read(1 << 20):
                    h.update(chunk)
            assert h.hexdigest() == hashlib.sha256(content).hexdigest(), "il file sul server non e' identico"
        try:
            self.s.remove(remote)
        except IOError:
            pass
        self.s.rename(tmp, remote)

    def prune(self, prefix: str, keep: str) -> None:
        """Tiene solo il file nuovo e quello precedente."""
        if self.local:
            return
        olds = sorted(e.filename for e in self.s.listdir_attr("data/app") if e.filename.startswith(prefix) and e.filename != keep)
        for old in olds[:-1]:
            self.s.remove(f"data/app/{old}")

    def close(self):
        if not self.local:
            self.s.close()
            self.t.close()

def publish(target: Target, kind: str, prefix: str, ext: str, manifest: str, version: str, data: bytes, notes: str) -> None:
    sha = hashlib.sha256(data).hexdigest()
    name = f"{prefix}-{version}.{ext}"
    print(f"{kind} {version}: {len(data) / 1e6:.1f} MB, sha256 {sha[:16]}... -> carico")
    target.put(name, data, verify=True)
    doc = {
        "version": version,
        "file": name,
        "sha256": sha,
        "size": len(data),
        "signature": sign(kind, version, sha),
        "notes": notes,
        "published_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }

    target.put(manifest, json.dumps(doc, indent=2).encode())
    target.prune(prefix + "-", name)
    print(f"  fatto: {name} pubblicato" + (" e verificato (hash identico)" if not target.local else ""))

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", default=os.path.join(ROOT, "target", "release", "relay-app.exe"))
    ap.add_argument("--version", default=None)
    ap.add_argument("--notes", default="")
    ap.add_argument("--no-app", action="store_true", help="non pubblicare l'app")
    ap.add_argument("--ffmpeg", default=None, help="ffmpeg.exe da pubblicare (viene impacchettato in uno zip)")
    ap.add_argument("--ffmpeg-version", default=None)
    ap.add_argument("--capture", default=None, help="relay-capture.exe (build --features recorder) da pubblicare")
    ap.add_argument("--capture-version", default=None)
    ap.add_argument("--local", default=None, help="cartella dati locale (per prove) invece della VPS")
    a = ap.parse_args()

    target = Target(a.local)
    try:
        if a.ffmpeg:
            v = a.ffmpeg_version or ffmpeg_version(a.ffmpeg)
            buf = io.BytesIO()
            with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
                z.write(a.ffmpeg, "ffmpeg.exe")
            publish(target, "relay-ffmpeg", "ffmpeg", "zip", "ffmpeg.json", v, buf.getvalue(), "")
        if a.capture:
            v = a.capture_version or workspace_version()
            publish(target, "relay-capture", "relay-capture", "exe", "capture.json", v, open(a.capture, "rb").read(), "")
        if not a.no_app:
            version = a.version or workspace_version()
            if not re.fullmatch(r"[0-9]+(\.[0-9]+){1,3}", version):
                print("versione non valida:", version)
                return 1
            publish(target, "relay-app", "relay-app", "exe", "latest.json", version, open(a.exe, "rb").read(), a.notes)
    finally:
        target.close()
    return 0

if __name__ == "__main__":
    sys.exit(main())
