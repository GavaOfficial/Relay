import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { createServer } from "node:http";

const PORT = Number(process.env.MOCK_PORT ?? 4000);
const ISS = process.env.MOCK_ISS ?? `http://127.0.0.1:${PORT}`;
const DEFAULT_USER = process.env.MOCK_USER ?? "cuid_mock_1";
const DEFAULT_NAME = process.env.MOCK_NAME ?? "Mock User";

const { publicKey, privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk = publicKey.export({ format: "jwk" });
const JWKS = { keys: [{ kty: "RSA", use: "sig", alg: "RS256", kid: "mock1", n: jwk.n, e: jwk.e }] };

const codes = new Map();

const b64u = (x) => Buffer.from(x).toString("base64url");

function accessToken(user, name) {
  const now = Math.floor(Date.now() / 1000);
  const head = b64u(JSON.stringify({ alg: "RS256", typ: "JWT", kid: "mock1" }));
  const body = b64u(
    JSON.stringify({
      iss: ISS,
      aud: "gavaauth-clients",
      sub: user,
      name,
      sid: randomBytes(8).toString("hex"),
      iat: now,
      exp: now + 900,
    }),
  );
  const sig = sign("RSA-SHA256", Buffer.from(`${head}.${body}`), privateKey).toString("base64url");
  return `${head}.${body}.${sig}`;
}

const refreshTokens = new Map();

function issue(user, name) {
  const refresh = randomBytes(32).toString("base64url");
  refreshTokens.set(refresh, { user, name });
  return { access_token: accessToken(user, name), refresh_token: refresh, token_type: "Bearer", expires_in: 900 };
}

function send(res, status, body, headers = {}) {
  const isObj = typeof body === "object";
  res.writeHead(status, { "content-type": isObj ? "application/json" : "text/plain", ...headers });
  res.end(isObj ? JSON.stringify(body) : body);
}

async function readJson(req) {
  let s = "";
  for await (const c of req) s += c;
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}

createServer(async (req, res) => {
  const url = new URL(req.url, ISS);
  try {
    if (req.method === "GET" && url.pathname === "/jwks.json") return send(res, 200, JWKS);

    if (req.method === "GET" && url.pathname === "/login") {
      const q = url.searchParams;
      const client_id = q.get("client_id");
      const redirect_uri = q.get("redirect_uri");
      const challenge = q.get("code_challenge");
      if (!client_id || !redirect_uri) return send(res, 400, "client_id/redirect_uri mancanti");
      if (q.get("response_type") !== "code") return send(res, 400, "response_type non valido");
      if (q.get("code_challenge_method") !== "S256" || !/^[A-Za-z0-9_-]{43}$/.test(challenge ?? "")) {
        return send(res, 400, "code_challenge non valido");
      }
      const hint = q.get("login_hint");
      const [user, name] = hint ? hint.split(":") : [DEFAULT_USER, DEFAULT_NAME];
      const code = randomBytes(24).toString("base64url");
      codes.set(code, {
        user,
        name: name ?? user,
        client_id,
        redirect_uri,
        challenge,
        exp: Date.now() + 60_000,
      });
      const back = new URL(redirect_uri);
      back.searchParams.set("code", code);
      if (q.get("state")) back.searchParams.set("state", q.get("state"));
      return send(res, 302, "", { location: back.toString() });
    }

    if (req.method === "POST" && url.pathname === "/token") {
      const b = await readJson(req);

      if (b && b.grant_type === "refresh_token") {
        const r = refreshTokens.get(String(b.refresh_token ?? ""));
        refreshTokens.delete(String(b.refresh_token ?? ""));
        if (!r) return send(res, 400, { error: "invalid_grant" });
        return send(res, 200, issue(r.user, r.name));
      }
      if (!b || b.grant_type !== "authorization_code") return send(res, 400, { error: "unsupported_grant_type" });
      const c = codes.get(b.code);
      codes.delete(b.code);
      if (!c || c.exp < Date.now()) return send(res, 400, { error: "invalid_grant" });
      if (c.client_id !== b.client_id || c.redirect_uri !== b.redirect_uri) {
        return send(res, 400, { error: "invalid_grant", error_description: "redirect_uri/client_id" });
      }
      const chal = createHash("sha256").update(String(b.code_verifier ?? "")).digest("base64url");
      if (chal !== c.challenge) return send(res, 400, { error: "invalid_grant", error_description: "PKCE" });
      return send(res, 200, issue(c.user, c.name));
    }

    if (req.method === "POST" && url.pathname === "/logout") {
      const b = await readJson(req);
      if (b?.refresh_token) refreshTokens.delete(String(b.refresh_token));
      return send(res, 204, "");
    }

    if (req.method === "GET" && url.pathname === "/logout") {
      const to = url.searchParams.get("post_logout_redirect_uri");
      return to ? send(res, 302, "", { location: to }) : send(res, 200, "logout");
    }

    send(res, 404, "not found");
  } catch (e) {
    send(res, 500, String(e));
  }
}).listen(PORT, "127.0.0.1", () => console.log(`mock GavaAuth su ${ISS} (utente ${DEFAULT_USER})`));
