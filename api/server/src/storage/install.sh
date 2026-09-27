#!/usr/bin/env bash
# Installa relay-storage: il server di archivio dei video di Relay.
#
#   curl -fsSL https://relay.gavatech.org/api/storage/install.sh | sudo bash -s -- --key rsk_...
#
# Opzioni:
#   --key CHIAVE     la chiave del server, creata dalla pagina "Archivio" del sito (obbligatoria)
#   --dir CARTELLA   dove salvare i video (predefinita: /var/lib/relay-storage)
#   --server URL     il server centrale (predefinito: https://relay.gavatech.org)
#   --conns N        connessioni in parallelo verso il centrale (predefinite: 4)
#   --no-bbr         non attivare il controllo di congestione TCP BBR
#
# Il programma gira come servizio systemd (relay-storage.service), con un utente dedicato,
# si ricollega da solo e si aggiorna da solo quando esce una nuova versione.
set -euo pipefail

SERVER="https://relay.gavatech.org"
KEY=""
DIR="/var/lib/relay-storage"
CONNS="4"
BBR=1

while [ $# -gt 0 ]; do
  case "$1" in
    --key) KEY="$2"; shift 2 ;;
    --dir) DIR="$2"; shift 2 ;;
    --server) SERVER="${2%/}"; shift 2 ;;
    --conns) CONNS="$2"; shift 2 ;;
    --no-bbr) BBR=0; shift ;;
    *) echo "opzione sconosciuta: $1" >&2; exit 1 ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then echo "serve root: usa sudo" >&2; exit 1; fi
if [ -z "$KEY" ]; then echo "manca --key (la trovi nella pagina Archivio del sito)" >&2; exit 1; fi
case "$(uname -m)" in
  x86_64|amd64) ;;
  *) echo "per ora relay-storage esiste solo per Linux x86_64 (questo e' $(uname -m))" >&2; exit 1 ;;
esac
for c in curl sha256sum systemctl; do
  command -v "$c" >/dev/null || { echo "manca il comando $c" >&2; exit 1; }
done

echo "==> utente e cartelle"
id relay-storage >/dev/null 2>&1 || useradd --system --home /var/lib/relay-storage --shell /usr/sbin/nologin relay-storage
install -d -o relay-storage -g relay-storage -m 750 /opt/relay-storage "$DIR"
install -d -m 750 /etc/relay-storage

echo "==> scarico relay-storage"
LATEST="$(curl -fsSL "$SERVER/api/app/storage/latest")"
SHA="$(printf '%s' "$LATEST" | sed -n 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([0-9a-fA-F]*\)".*/\1/p')"
VERSION="$(printf '%s' "$LATEST" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
TMP="$(mktemp)"
curl -fsSL "$SERVER/api/app/storage/download" -o "$TMP"
echo "$SHA  $TMP" | sha256sum -c --quiet - || { echo "file scaricato danneggiato" >&2; rm -f "$TMP"; exit 1; }
install -o relay-storage -g relay-storage -m 755 "$TMP" /opt/relay-storage/relay-storage
rm -f "$TMP"
echo "    versione $VERSION"

echo "==> configurazione"
umask 077
cat > /etc/relay-storage/config.env <<EOF
RELAY_SERVER=$SERVER
RELAY_STORAGE_KEY=$KEY
RELAY_STORAGE_DIR=$DIR
RELAY_STORAGE_CONNS=$CONNS
EOF
chown root:relay-storage /etc/relay-storage/config.env
chmod 640 /etc/relay-storage/config.env

if [ "$BBR" = 1 ] && modprobe tcp_bbr 2>/dev/null; then
  echo "==> attivo BBR (TCP piu' veloce su linee che perdono pacchetti, come Starlink)"
  cat > /etc/sysctl.d/90-relay-storage.conf <<EOF
net.core.default_qdisc=fq
net.ipv4.tcp_congestion_control=bbr
net.core.rmem_max=16777216
net.core.wmem_max=16777216
EOF
  # Solo il nostro file: ricaricarli tutti mostra errori di altre impostazioni del sistema.
  sysctl -p /etc/sysctl.d/90-relay-storage.conf >/dev/null
fi

echo "==> servizio systemd"
cat > /etc/systemd/system/relay-storage.service <<EOF
[Unit]
Description=Relay storage (archivio dei video)
After=network-online.target
Wants=network-online.target

[Service]
User=relay-storage
Group=relay-storage
EnvironmentFile=/etc/relay-storage/config.env
ExecStart=/opt/relay-storage/relay-storage
Restart=always
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DIR /opt/relay-storage

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now relay-storage.service
systemctl restart relay-storage.service

echo
echo "Fatto. Stato:      systemctl status relay-storage"
echo "          Log:     journalctl -u relay-storage -f"
