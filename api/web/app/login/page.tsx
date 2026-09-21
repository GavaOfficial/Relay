import { login } from "@/app/actions";
import DownloadApp from "@/components/DownloadApp";
import { DEV_LOGIN, GAVAAUTH_URL } from "@/lib/config";
import { safeNext } from "@/lib/redirect";

export const dynamic = "force-dynamic";

const ERRORS: Record<string, string> = {
  "1": "Token non valido.",
  server: "Il server di Relay non è raggiungibile. Riprova tra poco.",
  state: "Richiesta di accesso non valida o scaduta. Riprova ad accedere.",
  exchange: "Accesso non riuscito: lo scambio con GavaAuth è fallito (il codice potrebbe essere scaduto). Riprova.",
  session: "Relay non ha accettato l'accesso di GavaAuth. Riprova.",
  denied: "Accesso annullato o rifiutato da GavaAuth.",
  config: "Il login non è configurato.",
};

export default async function LoginPage({
  searchParams,
}: {
  searchParams: Promise<{ error?: string; next?: string }>;
}) {
  const { error, next: rawNext } = await searchParams;
  const next = safeNext(rawNext);
  const startHref = `/auth/start${next !== "/" ? `?next=${encodeURIComponent(next)}` : ""}`;
  const configured = Boolean(GAVAAUTH_URL) || DEV_LOGIN;

  return (
    <section className="hero">
      <span className="bigdot" aria-hidden="true" />
      <h1>Relay</h1>
      <p className="lead">Rivedi le visuali di gioco della tua partita, tutte insieme e in sincronia.</p>
      {error && <p className="error">{ERRORS[error] ?? "Errore."}</p>}

      {!configured && (
        <p className="muted">
          Il login non è configurato: imposta <code>GAVAAUTH_URL</code> (oppure <code>DEV_LOGIN=1</code> per lo
          sviluppo) nell&apos;ambiente del sito.
        </p>
      )}

      {GAVAAUTH_URL && (
        <a href={startHref} className="primary lg" role="button">
          Accedi con GavaAuth
        </a>
      )}

      <DownloadApp variant="full" />

      {DEV_LOGIN && (
        <form action={login} className="stack">
          <p className="muted">Accesso di sviluppo con token.</p>
          <input type="hidden" name="next" value={next} />
          <label>
            Token
            <input name="token" type="password" autoComplete="off" required />
          </label>
          <button type="submit" className="primary">
            Entra
          </button>
        </form>
      )}
    </section>
  );
}
