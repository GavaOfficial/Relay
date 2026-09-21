"use client";

import { useSyncExternalStore } from "react";

const noop = () => () => {};

export default function When({ seconds }: { seconds: number }) {
  const text = useSyncExternalStore(
    noop,
    () => {
      const d = new Date(seconds * 1000);
      const day = d.toLocaleDateString("it-IT", { day: "numeric", month: "short" });
      const time = d.toLocaleTimeString("it-IT", { hour: "2-digit", minute: "2-digit" });
      const today = new Date();
      const same = d.toDateString() === today.toDateString();
      return same ? `Oggi · ${time}` : `${day} · ${time}`;
    },
    () => "",
  );
  return <span className="when">{text}</span>;
}
