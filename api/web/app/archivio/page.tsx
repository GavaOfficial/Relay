import { notFound } from "next/navigation";
import OpsAdmin, { type OpsStatus } from "@/components/OpsAdmin";
import StorageAdmin, { type StorageStatus } from "@/components/StorageAdmin";
import { apiGet, apiGetOr } from "@/lib/api";

export const dynamic = "force-dynamic";

export default async function ArchivioPage() {
  const me = await apiGetOr<{ admin: boolean }>("/api/storage/admin", { admin: false });
  if (!me.admin) notFound();
  const [status, ops] = await Promise.all([
    apiGet<StorageStatus>("/api/storage/status"),
    apiGetOr<OpsStatus>("/api/ops/status", { nodes: [], queue: [] }),
  ]);
  return (
    <>
      <div className="pagehead">
        <h1>Archivio</h1>
        <p className="muted">
          I video si spostano cifrati sui server di archivio e da lì vengono trasmessi passando dal server centrale, senza copie locali.
        </p>
      </div>
      <StorageAdmin initial={status} />
      <OpsAdmin initial={ops} />
    </>
  );
}
