"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

const LINKS = [
  { href: "/", label: "Partite" },
  { href: "/giochi", label: "Giochi" },
];

export default function Nav({ admin = false }: { admin?: boolean }) {
  const pathname = usePathname();
  const links = admin ? [...LINKS, { href: "/archivio", label: "Archivio" }] : LINKS;
  return (
    <nav className="navlinks" aria-label="Navigazione">
      {links.map((l) => {
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
