# Misura preliminare OpenGL — 28 settembre 2026

Zoo sintetico in esclusivo a 2560×1440, richiesta 60 acquisizioni/s. Tre secondi di riscaldamento e circa quattro misurati per modalità, una singola esecuzione. Esclude l'encoder: misura il trasferimento al recorder. Non è una misura degli FPS di un gioco reale o di un PC economico.

| Misura | Nessuna cattura | Copia CPU | Texture GPU |
|---|---:|---:|---:|
| Durata, s | 4,007 | 4,019 | 4,013 |
| Fotogrammi acquisiti | 0 | 208 | 237 |
| Acquisizioni/s | — | 51,8 | 59,1 |
| Tempo CPU recorder, ms | 0,0 | 1203,1 | 46,9 |
| Tempo CPU zoo, ms | 78,1 | 1125,0 | 359,4 |
| SwapBuffers media, ms | 0,068 | 1,573 | 0,282 |
| SwapBuffers p95, ms | 0,101 | 2,732 | 0,614 |
| SwapBuffers p99, ms | 1,809 | 3,888 | 1,369 |

Tempo CPU recorder ridotto di circa il 96% e durata media SwapBuffers di circa l'82% rispetto alla copia CPU. Non sono percentuali di perdita FPS rispetto al gioco senza registrazione. I tempi CPU sono tempi cumulativi di processo, non percentuali di utilizzo dell'intero PC. Il p99 del baseline evidenzia rumore nella misura breve.

## Riproduzione

```powershell
cargo build --release --target-dir target/perf -p relay-recorder --examples
cargo build --release --target-dir target/perf -p relay-hook -p relay-inject
$env:RELAY_ZOO_RUNTIME = (Resolve-Path target/perf/release).Path
$env:RELAY_ZOO_EXCLUSIVE = '1'
cargo test --release -p relay-recorder --lib capture_performance_comparison -- --ignored --nocapture
Remove-Item Env:RELAY_ZOO_RUNTIME, Env:RELAY_ZOO_EXCLUSIVE
```

Report grezzo: `RELAY_ZOO_RUNTIME/capture-performance.txt`. Le prove H.264 e CLI verificano separatamente la registrazione completa, senza attribuirle i numeri della sola cattura.

Prima di promettere un impatto quasi impercettibile servono prove prolungate e ripetute su hardware economico, annotando CPU, GPU e driver: gioco senza registrazione contro registrazione completa, FPS e tempi p95/p99, CPU/GPU, memoria e frame persi. Includere GPU prossima al 100%, portatili con due GPU e risoluzioni diverse. Le copie GPU attuali sono alla risoluzione nativa: banda e VRAM continuano a contare.
