# Verifica V-Sync — 29 settembre 2026

## Correzione dell'avvio

Encoder, primo sample e prima conversione vengono preparati prima di fissare
l'origine temporale di una registrazione locale. Il sample preparato viene
riutilizzato per il primo fotogramma. Un'origine condivisa esplicitamente
configurata mantiene il valore originale.

Verifiche dopo la correzione:

- `record.py --startup`: due avvii a 60 fps, primo segmento di 1,000 s,
  181 immagini in ciascuna prova da circa 3 secondi, nessun avviso o salto.
- Geometry Dash 1080p60 NVENC, prova `GeometryDash-20260929-221606`:
  602 immagini in circa 10 secondi, primi dieci segmenti tutti di 1,000 s,
  nessun avviso, blocco rilevato o discontinuità a regime; sempre hook.
- 30 test unitari del recorder passati; tre test interattivi esclusi dalla suite.

Il salto e il primo segmento lungo riportati sotto si riferiscono alle prove
precedenti alla correzione.

## OpenGL

Il produttore usa tre coppie di texture interop/condivise sullo stesso dispositivo
D3D11. Pubblica ogni copia soltanto dopo una query EVENT completata, verificata
con GetData(DONOTFLUSH), e conserva la texture attualmente pubblicata.
Il recorder mantiene una cache di tre importazioni NT.

`Flush` invia i comandi e non ne garantisce il completamento:
[documentazione Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-flush).

### Zoo a schermo intero

```powershell
python client/capture/recorder/tests/zoo/record.py --runtime dist/capture-runtime --zoo target/dxvk/release/examples/relay-zoo-opengl.exe --api opengl --exclusive --vsync
```

Risultato locale `dist/capture-runtime/zoo-e2e-9268-0`: 3.601 fotogrammi,
60 secondi, contatori consecutivi, nessun intervallo mancante dopo i primi due
secondi. Segmenti a regime di un secondo. Un fotogramma saltato all'avvio;
primo segmento di 1,983333 secondi, prima della correzione dell'avvio.

Il test DXGI equivalente è disponibile sostituendo l'eseguibile con
`relay-zoo-dxgi.exe` e `--api dxgi`; usa Present(1, 0). La verifica da 60 secondi
DXGI non è ancora stata eseguita.

### Geometry Dash reale, 1080p60, NVENC

```powershell
python client/capture/recorder/tests/zoo/real_game.py --runtime dist/capture-runtime --exe GeometryDash.exe --pid <PID> --seconds 60 --height 1080 --focus-once
```

Il gioco deve avere V-Sync attivo e schermo intero; lo script non modifica queste
impostazioni. `--focus-once` ripristina la finestra una volta prima della prova.
Il processo raccoglie da solo stato finestra, contatore hook ed eventi: evitare
di aprire terminali durante il minuto, perché possono minimizzare il gioco.
I campioni dello stato finestra sono diagnostici (lettura condivisa senza lock),
non una verifica atomica del protocollo.

Prova `GeometryDash-20260929-220941`:

- Finestra sempre in primo piano nei campioni ogni mezzo secondo, mai minimizzata.
- Sorgente sempre hook, 3.603 fotogrammi in circa 60 secondi.
- Nessuna discontinuità temporale a regime e nessun blocco rilevato da freezedetect.
- Segmenti a regime di 1 secondo; un salto all'avvio e primo segmento di 1,983333 s.
- Massimi CPU delle chiamate: AcquireSync 241 µs; CopyResource 60 µs;
  Flush 297 µs; conversione 508 µs; invio NVENC 1.131 µs.

Due prove precedenti sono inconcludenti per il blocco GPU: la seconda mostra
esplicitamente la minimizzazione al secondo 28, coincidente con l'arresto del
contatore hook. Explorer e poi Windows Terminal risultavano in primo piano.
Ripetendo senza lanciare altri comandi durante la registrazione, il blocco
non si è ripresentato. Non sono misure dell'impatto sugli FPS del gioco né una
garanzia per altri PC o driver.

Ogni registrazione conserva `capture-timings.jsonl`, con numero di chiamate,
media, massimo e conteggio oltre 16,667 ms per intervallo. Sono tempi CPU delle
API, non tempi GPU. Le statistiche vengono scritte da un thread separato.
`real_game.py` conserva inoltre video MP4, rapporto, eventi con tempi e stato
della finestra; una scena realmente statica può essere segnalata come blocco.
