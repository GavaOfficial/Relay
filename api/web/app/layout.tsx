import type { Metadata } from "next";
import Header from "@/components/Header";
import "./globals.css";

export const metadata: Metadata = {
  title: "Relay",
  description: "Replay e diretta sincronizzati delle visuali di gioco",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="it">
      <body>
        <Header />
        <main className="page">{children}</main>
      </body>
    </html>
  );
}
