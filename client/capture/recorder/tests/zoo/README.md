# Zoo di cattura

Il metodo principale è l'hook OpenGL GPU, con DLL e iniettori x64/x86. Se non parte si usa WGC finestra, poi quando necessario WGC monitor. I processi riconosciuti dalla lista anti-cheat o non ispezionabili non vengono agganciati.

## Compilazione e prove

```powershell
rustup target add i686-pc-windows-msvc
./deploy/build-capture.ps1 -Zoo
```

Il runtime completo è in `dist/capture-runtime`, con `hooks/x64` e `hooks/x86`. Richiede MSVC, desktop Windows interattivo, interop OpenGL/D3D11 e ffmpeg. Usare `-BuildDir target/perf` per scegliere un'altra directory di compilazione.

Prove del runtime completo (due registrazioni nello stesso processo):

```powershell
python client/capture/recorder/tests/zoo/record.py --runtime dist/capture-runtime --zoo target/capture-build/release/examples/relay-zoo-opengl.exe --exclusive
python client/capture/recorder/tests/zoo/record.py --runtime dist/capture-runtime --zoo target/capture-build/i686-pc-windows-msvc/release/examples/relay-zoo-opengl.exe
```

Lo zoo disegna un contatore binario a 24 barre rosse/blu e un bordo verde. Il numero avanza a 10 Hz: rileva interruzioni di almeno 100 ms, non ogni presentazione saltata. Le prove controllano orientamento, contatori consecutivi, resize e riavvio. Il checker CPU controlla anche lo stato GL_PACK_ROW_LENGTH e i due metodi di iniezione.

La prova H.264 usa Source Hook, convertitore D3D11, encoder hardware e ffmpeg; aspetta un frame fresco dopo l'inizializzazione dell'encoder. Lo script CLI conserva video e log ed esclude i primi 15 frame dalla verifica della continuità a regime. Le prove GPU falliscono se manca il supporto richiesto. Non certificano 60 fps su ogni PC. Vedere [PERFORMANCE.md](PERFORMANCE.md) per le misure.

## Percorso GPU e limiti

- OpenGL → texture D3D11 privata tramite WGL_NV_DX_interop2 → texture condivisa NT con keyed mutex → texture del recorder → conversione GPU ed encoder hardware. Esistono copie GPU; i pixel non passano dalla RAM della CPU.
- La memoria condivisa GPU contiene solo 104 byte di metadati. Le texture occupano VRAM in proporzione alla risoluzione nativa.
- La cattura segue la frequenza richiesta. Il produttore non aspetta il consumer sul keyed mutex e può sostituire un frame non letto. Driver e sincronizzazione OpenGL/D3D11 hanno comunque un costo; non si garantisce assenza di stalli.
- Interop e recorder devono usare lo stesso adattatore. Se l'interop fallisce si usa WGC. La copia CPU rimane per prove e compatibilità, esclusa dalla selezione automatica.
- L'encoder automatico richiede hardware e input D3D11. L'encoder software richiede una scelta esplicita.
- Nessun DllMain personalizzato: bootstrap da RelayStart o callback Windows. La presentazione originale continua dopo errori Rust intercettabili; i fault nativi dei driver non sono intercettati da catch_unwind.
- Alla chiusura del recorder si interrompe la cattura. La distruzione delle risorse GL può attendere la prossima presentazione o eliminazione del contesto. Se un contesto in eliminazione non può essere reso corrente, le risorse vengono conservate fino alla fine del processo per evitare accessi non validi. DLL e trampolini rimangono residenti per sicurezza.
- La lista anti-cheat non è esaustiva. Finestre protette, architettura incompatibile e ispezione fallita impediscono l'iniezione.

## Roadmap

Implementati: protocollo CPU/GPU versionato, OpenGL GPU, NT handle e keyed mutex, hook principale con ripiego WGC, iniettori e DLL x64/x86, zoo e script di confezionamento.

DirectX 10/11/12, DirectX 9Ex e Vulkan sono implementati con i requisiti e i limiti descritti in [BACKENDS.md](BACKENDS.md).

Da completare: D3D9 classico e 9Ex non multithread; helper UAC; HDR; zoo delle altre API e finestra guida; collaudo FNF/Geometry Dash reali e PC economico; firma e distribuzione del runtime completo.

Per il collaudo reale verificare finestra/esclusivo, Alt-Tab, minimizzazione, cambio monitor e due registrazioni consecutive. Controllare il video oltre al caricamento della DLL.

Firma: `./deploy/build-capture.ps1 -CertificateThumbprint <impronta>`. Serve un certificato valido. Authenticode è distinto dalla firma degli aggiornamenti e non garantisce accettazione da Defender o anti-cheat. Pubblicare il solo relay-capture.exe non distribuisce gli hook.

Riferimenti: [Khronos interop](https://registry.khronos.org/OpenGL/extensions/NV/WGL_NV_DX_interop2.txt), [Microsoft AcquireSync](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgikeyedmutex-acquiresync), [NVIDIA NVENC](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/index.html).
