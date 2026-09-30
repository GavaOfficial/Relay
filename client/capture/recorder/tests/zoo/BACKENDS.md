# Backend DirectX e Vulkan

## Implementazione

- **DirectX 11:** Present, Present1, ResizeBuffers e ResizeBuffers1; copia/risoluzione MSAA su texture GPU condivisa NT con keyed mutex. Il backbuffer del gioco non viene conservato fra le presentazioni.
- **DirectX 10:** Present DXGI; tre texture condivise e query di completamento per sincronizzare D3D10 e D3D11 senza attendere la GPU sulla CPU. Il trasferimento al recorder usa NT handle e keyed mutex.
- **DirectX 12:** Present DXGI e ExecuteCommandLists; D3D11On12 usa esclusivamente una coda DIRECT osservata durante la presentazione della stessa swapchain. Se la coda non è identificabile si usa WGC. Gestisce sostituzione coda e resize.
- **DirectX 9Ex:** Present, PresentEx, Present della swapchain, Reset e ResetEx. Tre texture condivise e query GPU. Per ora richiede un dispositivo creato con MULTITHREADED; D3D9 classico e dispositivi 9Ex non multithread usano WGC. Non vengono applicate patch ai componenti interni del runtime D3D9.
- **Vulkan:** layer del loader, senza iniettore. Intercetta creazione/distruzione di instance, device, superficie Win32 e swapchain, acquisizione code e QueuePresentKHR. Importa una texture D3D11 nella GPU Vulkan e concatena il semaforo della copia alla presentazione originale. Il keyed mutex usa una chiave riservata alla coda Vulkan; nessuna lettura dei pixel sulla CPU.

Tutti i percorsi limitano le acquisizioni alla frequenza richiesta. Nessun busy loop su fence/query. Se il consumer è occupato, il produttore salta una copia. Le chiamate ai driver possono comunque avere costi o attese interne; non è una garanzia di impatto nullo.

Il protocollo è ora versione **3**, mantiene il layout di 104 byte e distingue BGRA, RGBA e BGRX. Distribuire insieme recorder, DLL e iniettori della stessa compilazione.

## Requisiti e limiti Vulkan

- Layer caricato **prima** che il gioco crei l'istanza Vulkan. I giochi già aperti richiedono un riavvio dopo l'installazione.
- Vulkan 1.1 o successivo, VK_KHR_external_memory_win32 e VK_KHR_win32_keyed_mutex; stesso adattatore del recorder, formato BGRA8/RGBA8 e immagini copiabili come TRANSFER_SRC.
- Una swapchain per QueuePresent, senza catena pNext della presentazione; niente risorse protette. La coda di presentazione deve supportare grafica. Per immagini con ownership esclusiva serve una sola famiglia grafica richiesta dal dispositivo. I casi non supportati passano a WGC.
- Per consentire l'avvio della registrazione su un gioco già aperto, il layer prepara il supporto all'importazione e richiede TRANSFER_SRC quando crea la swapchain. Le copie e il dispositivo D3D11 di cattura partono solo quando esiste una sessione Relay autorizzata.
- Alla fine della registrazione il canale viene chiuso. Texture importata e semafori rimangono fino alla distruzione della swapchain e vengono riutilizzati alla registrazione successiva: distruggerli mentre il motore di presentazione li usa non sarebbe sicuro. Nessuna attesa di inattività del device durante la cattura; l'attesa di rilascio è limitata alla distruzione delle risorse.
- HKCU non abilita il layer nei giochi eseguiti come amministratore. La lista anti-cheat impedisce l'attivazione della cattura per i nomi conosciuti; non certifica protezioni sconosciute.

## Pacchetto e installazione Vulkan

```powershell
./deploy/build-capture.ps1 -BuildDir target/dxvk
```

Il pacchetto è in `dist/capture-runtime`, con DLL/iniettori/manifest in `hooks/x64` e `hooks/x86`. La registrazione del layer nelle chiavi HKCU Khronos (DWORD 0) e la sua rimozione le farà l'app all'installazione e alla disinstallazione; fino ad allora si prova senza toccare il registro, come qui sotto.

Per una prova senza modificare il registro:

```powershell
$env:VK_LAYER_PATH = (Resolve-Path dist/capture-runtime/hooks/x64).Path
$env:VK_INSTANCE_LAYERS = 'VK_LAYER_RELAY_capture'
python client/capture/recorder/tests/zoo/record.py --runtime dist/capture-runtime --zoo target/dxvk/release/examples/relay-zoo-vulkan.exe --api vulkan
Remove-Item Env:VK_LAYER_PATH, Env:VK_INSTANCE_LAYERS
```

Per un gioco a 32 bit scegliere `hooks/x86` e l'esempio compilato con `--target i686-pc-windows-msvc`. Il recorder rimane a 64 bit. `RELAY_VK_DISABLE=1` disabilita il caricamento implicito del layer nei nuovi processi.

## Verifica

Gli esempi `relay-zoo-dxgi`, `relay-zoo-dx10`, `relay-zoo-dx12`, `relay-zoo-d3d9` e `relay-zoo-vulkan` disegnano il contatore a colori. `record.py --api dxgi|d3d9|vulkan` verifica sorgente Hook, API pubblicata, assenza di buffer CPU, encoder hardware, due registrazioni e contatori consecutivi nel video HLS decodificato. Il controllo a regime esclude i primi 15 fotogrammi; il contatore a 10 Hz non distingue ogni presentazione saltata.

Non sono ancora misure di prestazioni su giochi reali, test HDR o certificazioni di compatibilità con ogni driver. I risultati del benchmark OpenGL in PERFORMANCE.md non vanno estesi a questi backend.

## Riferimenti

- [Microsoft: condivisione fra API grafiche](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/surface-sharing-between-windows-graphics-apis)
- [Microsoft: D3D11On12](https://learn.microsoft.com/en-us/windows/win32/direct3d12/direct3d-11-on-12)
- [Khronos: contratto del loader e registrazione dei layer](https://github.com/KhronosGroup/Vulkan-Loader/blob/main/docs/LoaderLayerInterface.md)
- [Khronos: sincronizzazione keyed mutex](https://registry.khronos.org/vulkan/specs/latest/man/html/VkWin32KeyedMutexAcquireReleaseInfoKHR.html)
