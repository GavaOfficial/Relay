import Link from "next/link";
import { logout } from "@/app/actions";
import { currentIdentity } from "@/lib/api";
import Avatar from "./Avatar";
import DownloadApp from "./DownloadApp";

export default async function Header() {
  const me = await currentIdentity();
  return (
    <header className="topbar">
      <div className="topbar-in">
        <Link href="/" className="brand">
          <span className="logo" aria-hidden="true" />
          Relay
        </Link>
        <div className="topbar-right">
          <DownloadApp />
          {me && (
          <form action={logout} className="who">
            <Avatar name={me.name ?? me.user} size={26} />
            <span className="who-name">{me.name ?? me.user}</span>
            <button type="submit" className="ghost small">
              Esci
            </button>
          </form>
          )}
        </div>
      </div>
    </header>
  );
}
