#!/usr/bin/env bash
# Installa relay-ops: il server delle operazioni di Relay (unisce, riduce e taglia i video con ffmpeg).
#
#   curl -fsSL https://relay.gavatech.org/api/ops/install.sh | sudo bash -s -- --key rok_...
#
# Opzioni:
#   --key CHIAVE     la chiave del server, creata dalla pagina "Archivio" del sito (obbligatoria)
#   --dir CARTELLA   cartella di lavoro per i file temporanei (predefinita: /var/lib/relay-ops)
#   --server URL     il server centrale (predefinito: https://relay.gavatech.org)
#   --threads N      thread da usare per ffmpeg (predefiniti: tutti)
#
# Il programma gira come servizio systemd (relay-ops.service), con un utente dedicato,
# chiede da solo i lavori al centrale e si aggiorna da solo quando esce una nuova versione.
set -euo pipefail

SERVER="https://relay.gavatech.org"
KEY=""
DIR="/var/lib/relay-ops"
THREADS=""

while [ $# -gt 0 ]; do
  case "$1" in
    --key) KEY="$2"; shift 2 ;;
    --dir) DIR="$2"; shift 2 ;;
    --server) SERVER="${2%/}"; shift 2 ;;
    --threads) THREADS="$2"; shift 2 ;;
    *) echo "opzione sconosciuta: $1" >&2; exit 1 ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then echo "serve root: usa sudo" >&2; exit 1; fi
if [ -z "$KEY" ]; then echo "manca --key (la trovi nella pagina Archivio del sito)" >&2; exit 1; fi
case "$(uname -m)" in
  x86_64|amd64) ;;
  *) echo "per ora relay-ops esiste solo per Linux x86_64 (questo e' $(uname -m))" >&2; exit 1 ;;
esac
for c in curl sha256sum systemctl; do
  command -v "$c" >/dev/null || { echo "manca il comando $c" >&2; exit 1; }
done

if ! command -v ffmpeg >/dev/null; then
  echo "==> installo ffmpeg"
  if command -v apt-get >/dev/null; then
    DEBIAN_FRONTEND=noninteractive apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ffmpeg >/dev/null
  else
    echo "installa ffmpeg con il gestore di pacchetti del sistema e rilancia lo script" >&2; exit 1
  fi
fi
FFMPEG="$(command -v ffmpeg)"
echo "    $("$FFMPEG" -version | head -n 1)"

echo "==> utente e cartelle"
id relay-ops >/dev/null 2>&1 || useradd --system --home "$DIR" --shell /usr/sbin/nologin relay-ops
install -d -o relay-ops -g relay-ops -m 750 /opt/relay-ops "$DIR"
install -d -m 750 /etc/relay-ops

echo "==> scarico relay-ops"
LATEST="$(curl -fsSL "$SERVER/api/app/ops/latest")"
SHA="$(printf '%s' "$LATEST" | sed -n 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([0-9a-fA-F]*\)".*/\1/p')"
VERSION="$(printf '%s' "$LATEST" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
TMP="$(mktemp)"
curl -fsSL "$SERVER/api/app/ops/download" -o "$TMP"
echo "$SHA  $TMP" | sha256sum -c --quiet - || { echo "file scaricato danneggiato" >&2; rm -f "$TMP"; exit 1; }
install -o relay-ops -g relay-ops -m 755 "$TMP" /opt/relay-ops/relay-ops
rm -f "$TMP"
echo "    versione $VERSION"

echo "==> configurazione"
umask 077
cat > /etc/relay-ops/config.env <<EOF
RELAY_SERVER=$SERVER
RELAY_OPS_KEY=$KEY
RELAY_OPS_DIR=$DIR
RELAY_FFMPEG=$FFMPEG
EOF
if [ -n "$THREADS" ]; then echo "RELAY_OPS_THREADS=$THREADS" >> /etc/relay-ops/config.env; fi
chown root:relay-ops /etc/relay-ops/config.env
chmod 640 /etc/relay-ops/config.env

echo "==> servizio systemd"
cat > /etc/systemd/system/relay-ops.service <<EOF
[Unit]
Description=Relay ops (operazioni sui video con ffmpeg)
After=network-online.target
Wants=network-online.target

[Service]
User=relay-ops
Group=relay-ops
EnvironmentFile=/etc/relay-ops/config.env
ExecStart=/opt/relay-ops/relay-ops
Restart=always
RestartSec=5
Nice=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DIR /opt/relay-ops

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now relay-ops.service
systemctl restart relay-ops.service

echo
echo "Fatto. Stato:      systemctl status relay-ops"
echo "          Log:     journalctl -u relay-ops -f"
