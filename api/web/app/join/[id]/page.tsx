import Link from "next/link";
import DownloadApp from "@/components/DownloadApp";
import OpenApp from "@/components/OpenApp";

export const dynamic = "force-dynamic";

const SAFE = /^[A-Za-z0-9_-]{1,64}$/;

export default async function JoinPage({
  params,
  searchParams,
}: {
  params: Promise<{ id: string }>;
  searchParams: Promise<{ code?: string | string[] }>;
}) {
  const { id } = await params;
  const sp = await searchParams;
  const code = Array.isArray(sp.code) ? sp.code[0] : sp.code;

  if (!SAFE.test(id) || !code || !SAFE.test(code)) {
    return (
      <section className="hero">
        <div className="bigdot" aria-hidden="true" />
        <h1>Link non valido</h1>
        <p className="lead">Al link di invito manca qualcosa. Chiedi all&apos;host di inviartelo di nuovo.</p>
        <Link href="/" className="back">
          Torna alle partite
        </Link>
      </section>
    );
  }

  const deepLink = `relay://join/${id}?code=${code}`;
  return <OpenApp deepLink={deepLink} download={<DownloadApp variant="full" />} />;
}
