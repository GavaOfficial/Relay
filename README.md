# Relay

Registrazione sincronizzata delle finestre di gioco di più PC, con replay affiancato e sincronizzato sul sito.

Ogni giocatore registra la propria finestra di gioco (video e audio del gioco, microfono opzionale) a segmenti da 4 secondi allineati a un orologio comune, e li carica sul server. Quando la partita finisce il server unisce i segmenti in un video per giocatore, e il sito mostra le visuali insieme, con un solo Play e una sola barra del tempo.

## Componenti

| Cartella | Cosa è |
| --- | --- |
| `common` | Tipi condivisi tra server e client (Rust) |
| `api/server` | Server API (Rust, axum): partite, caricamento dei segmenti, conversione in MP4, download e streaming |
| `api/web` | Sito (Next.js): elenco delle partite, replay sincronizzato, download dell'app |
| `client/agent` | Libreria e riga di comando che registrano e caricano (Rust, ffmpeg) |
| `client/app` | App desktop per Windows (Tauri): creare una partita, entrare, scegliere la finestra, aggiornarsi da sola |
| `deploy` | Esempi di configurazione (systemd, nginx, docker) e script di pubblicazione dell'app |

## Requisiti

- Rust stabile
- Node.js 22 o successivo
- ffmpeg: sul server serve per creare gli MP4; sul PC di chi gioca serve una build con il filtro `gfxcapture` (ffmpeg 8 o successivo, versione "full")
- Il client di registrazione funziona solo su Windows 10/11

## Compilare e provare

```sh
cargo build --workspace --release
cargo test --workspace

cd api/web
npm ci
npm run build
```

Per provare tutto in locale, con un utente di prova e qualche partita di esempio:

```sh
cd api/web
npm run dev:local
```

## Configurazione del server

Le variabili d'ambiente sono in `deploy/relay.env.example`. In produzione il login passa da un provider OpenID Connect (`GAVAAUTH_ISSUER`); senza, il server accetta solo i token di sviluppo (`RELAY_DEV_TOKENS`), da non usare fuori dai test.

## Licenza

Distribuito con licenza [GNU GPL v3 o successiva](LICENSE).
