import Link from "next/link";
import { logout } from "@/app/actions";
import { currentIdentity } from "@/lib/api";
import Avatar from "./Avatar";
import DownloadApp from "./DownloadApp";
import Nav from "./Nav";

export default async function Header() {
  const me = await currentIdentity();
  return (
    <div className="topbar-wrap">
      <header className="topbar">
        <div className="topbar-in">
          <Link href="/" className="brand">
            <span className="logo" aria-hidden="true" />
            Relay
          </Link>
          {me && <Nav />}
          <div className="topbar-right">
            <DownloadApp />
            {me && (
              <details className="acctmenu">
                <summary className="acctchip">
                  <Avatar name={me.name ?? me.user} size={26} />
                  <span className="who-name">{me.name ?? me.user}</span>
                </summary>
                <div className="acctpop">
                  <div className="acctpop-id">
                    <Avatar name={me.name ?? me.user} size={38} />
                    <div>
                      <div className="acctpop-idname">{me.name ?? me.user}</div>
                      <div className="acctpop-iduser">{me.user}</div>
                    </div>
                  </div>
                  <div className="acctpop-sep" />
                  <span className="acctpop-link acctpop-static">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                      <path d="M20 21a8 8 0 0 0-16 0" />
                      <circle cx="12" cy="7" r="4" />
                    </svg>
                    Account
                  </span>
                  <div className="acctpop-sep" />
                  <form action={logout}>
                    <button type="submit" className="acctpop-out">
                      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                        <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
                        <path d="M16 17l5-5-5-5" />
                        <path d="M21 12H9" />
                      </svg>
                      Esci
                    </button>
                  </form>
                </div>
              </details>
            )}
          </div>
        </div>
      </header>
    </div>
  );
}
