"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

const LINKS = [{ href: "/", label: "Partite" }];

export default function Nav() {
  const pathname = usePathname();
  return (
    <nav className="navlinks" aria-label="Navigazione">
      {LINKS.map((l) => {
        const active = l.href === "/" ? pathname === "/" : pathname.startsWith(l.href);
        return (
          <Link key={l.href} href={l.href} className={`navlink${active ? " on" : ""}`}>
            {l.label}
          </Link>
        );
      })}
    </nav>
  );
}
