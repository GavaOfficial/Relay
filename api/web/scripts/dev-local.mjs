import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WEB = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ROOT = path.resolve(WEB, "..", "..");
const DATA = path.join(WEB, ".dev-data");
const API = "http://127.0.0.1:8080";
const isWin = process.platform === "win32";
const exe = (n) => (isWin ? `${n}.exe` : n);

const children = [];
const stopAll = () => children.forEach((c) => c.kill());
process.on("SIGINT", () => (stopAll(), process.exit(0)));
process.on("exit", stopAll);

const say = (t) => console.log(`\x1b[36m[relay]\x1b[0m ${t}`);

function findFfmpeg() {
  const candidates = [process.env.RELAY_FFMPEG, "ffmpeg"];
  if (isWin && process.env.LOCALAPPDATA) {
    const pk = path.join(process.env.LOCALAPPDATA, "Microsoft", "WinGet", "Packages");
    if (existsSync(pk)) {
      for (const d of readdirSync(pk).filter((n) => n.startsWith("Gyan.FFmpeg"))) {
        const inner = path.join(pk, d);
        for (const v of readdirSync(inner).filter((n) => n.startsWith("ffmpeg-"))) candidates.push(path.join(inner, v, "bin", "ffmpeg.exe"));
      }
    }
  }
  for (const c of candidates.filter(Boolean)) {
    const r = spawnSync(c, ["-version"], { stdio: "ignore" });
    if (r.status === 0) return c;
  }
  return null;
}
const FFMPEG = findFfmpeg();
say(FFMPEG ? `ffmpeg trovato: ${FFMPEG}` : "ffmpeg non trovato: le partite di prova non avranno video (installalo per vedere il lettore)");

function cargo() {
  const home = process.env.CARGO_HOME || path.join(homedir(), ".cargo");
  const p = path.join(home, "bin", exe("cargo"));
  return existsSync(p) ? p : "cargo";
}
say("compilo il server Relay (la prima volta ci vuole un po')...");
const build = spawnSync(cargo(), ["build", "-p", "relay-server"], { cwd: ROOT, stdio: "inherit" });
if (build.status !== 0) {
  console.error("Compilazione del server non riuscita. Serve Rust (https://rustup.rs).");
  process.exit(1);
}

const TOKENS = "dev=dev,marco=marco,luca=luca,anna=anna";
const fresh = process.argv.includes("--reset");
if (fresh && existsSync(DATA)) rmSync(DATA, { recursive: true, force: true });
const seeded = existsSync(path.join(DATA, "matches")) && readdirSync(path.join(DATA, "matches")).length > 0;
mkdirSync(DATA, { recursive: true });

const server = spawn(path.join(ROOT, "target", "debug", exe("relay-server")), [], {
  cwd: ROOT,
  env: {
    ...process.env,
    RELAY_DEV_TOKENS: TOKENS,
    RELAY_BIND: "127.0.0.1:8080",
    RELAY_DATA: DATA,
    ...(FFMPEG ? { RELAY_FFMPEG: FFMPEG } : {}),
  },
  stdio: ["ignore", "inherit", "inherit"],
});
children.push(server);
server.on("exit", (c) => {
  if (c) console.error(`Il server Relay si e' fermato (codice ${c}). La porta 8080 e' gia' usata?`);
  process.exit(c ?? 0);
});

async function waitHealthy() {
  for (let i = 0; i < 60; i++) {
    try {
      if ((await fetch(`${API}/api/healthz`)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error("il server non risponde");
}

const call = (method, url, token, body, headers = {}) =>
  fetch(`${API}${url}`, {
    method,
    headers: { authorization: `Bearer ${token}`, ...(body && !Buffer.isBuffer(body) ? { "content-type": "application/json" } : {}), ...headers },
    body: body ? (Buffer.isBuffer(body) ? body : JSON.stringify(body)) : undefined,
  });

function makeSegments(src, freq, size, secs, dir) {
  mkdirSync(dir, { recursive: true });
  const args = [
    "-hide_banner", "-loglevel", "error", "-y",
    "-f", "lavfi", "-i", `${src}=size=${size}:rate=30`,
    "-f", "lavfi", "-i", `sine=frequency=${freq}:sample_rate=48000,volume=0.6`,
    "-t", String(secs),
    "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-g", "120", "-keyint_min", "120", "-sc_threshold", "0",
    "-c:a", "aac", "-b:a", "128k",
    "-f", "hls", "-hls_time", "4", "-hls_list_size", "0",
    "-hls_segment_filename", path.join(dir, "s_%d.ts"), path.join(dir, "p.m3u8"),
  ];
  const r = spawnSync(FFMPEG, args, { stdio: "ignore" });
  if (r.status !== 0) throw new Error("ffmpeg non ha creato i video di prova");
  const out = [];
  for (let n = 0; existsSync(path.join(dir, `s_${n}.ts`)); n++) out.push(readFileSync(path.join(dir, `s_${n}.ts`)));
  return out;
}

async function seed() {
  say("creo le partite di prova...");
  const mk = async (players) => (await (await call("POST", "/api/matches", "dev", { players, play: false })).json()).id;

  if (FFMPEG) {
    const id = await mk(["marco", "luca", "anna"]);
    await call("POST", `/api/matches/${id}/start?force=true`, "dev");
    const work = path.join(tmpdir(), `relay-seed-${process.pid}`);
    const sets = [
      ["marco", "testsrc2", 440, "1280x720"],
      ["luca", "smptebars", 523, "1280x720"],
      ["anna", "rgbtestsrc", 659, "1280x720"],
    ];
    for (const [who, src, freq, size] of sets) {
      const segs = makeSegments(src, freq, size, 24, path.join(work, who));
      for (let n = 0; n < segs.length; n++) {
        await call("PUT", `/api/matches/${id}/players/${who}/segments/${String(n).padStart(8, "0")}.ts`, who, segs[n], { "x-segment-duration-ms": "4000" });
      }
    }
    await call("POST", `/api/matches/${id}/stop`, "dev");
    for (const [who] of sets) await call("POST", `/api/matches/${id}/players/${who}/finish`, who);
    rmSync(work, { recursive: true, force: true });
    say("partita finita creata (il server sta preparando gli MP4: pochi secondi)");
  }

  await mk(["marco", "luca"]);
  await mk(["anna"]);
}

async function main() {
  await waitHealthy();
  if (!seeded) await seed();
  else say("uso i dati gia' presenti (`npm run dev:local -- --reset` per ricominciare)");

  say("avvio il sito...");
  const next = spawn(isWin ? "npx.cmd" : "npx", ["next", "dev", "-p", "3000"], {
    cwd: WEB,
    shell: isWin,
    env: { ...process.env, API_ORIGIN: API, DEV_LOGIN: "1", SITE_URL: "http://localhost:3000" },
    stdio: "inherit",
  });
  children.push(next);
  next.on("exit", (c) => process.exit(c ?? 0));

  console.log(`
  \x1b[1mPronto.\x1b[0m
    Sito:      http://localhost:3000
    Accesso:   scrivi il token  \x1b[1mdev\x1b[0m   (e' l'organizzatore delle partite di prova)
    Altri:     marco, luca, anna (giocatori)
    Server:    ${API}
    Fermare:   Ctrl+C
`);
}
main().catch((e) => {
  console.error(e);
  process.exit(1);
});
