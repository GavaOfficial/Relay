"use client";

import { useEffect, type ReactNode } from "react";

export default function OpenApp({ deepLink, download }: { deepLink: string; download?: ReactNode }) {
  useEffect(() => {
    window.location.href = deepLink;
  }, [deepLink]);

  return (
    <section className="hero">
      <div className="bigdot" aria-hidden="true" />
      <h1>Apro Relay…</h1>
      <p className="lead">Il link di invito si apre nell&apos;app, non qui. Se non succede nulla, premi il pulsante.</p>
      <a href={deepLink} className="primary">
        Apri Relay
      </a>
      {download}
      <p className="muted">
        L&apos;app non si apre? Installa Relay e avviala una volta, poi riprova. In alternativa, in Relay scegli
        <strong> Entra con un link</strong> e incolla l&apos;indirizzo di questa pagina.
      </p>
    </section>
  );
}
