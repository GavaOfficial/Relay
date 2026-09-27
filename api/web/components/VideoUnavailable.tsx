"use client";

import { useEffect } from "react";

const MESSAGE = "Video non disponibile per ora: il server di archivio dove si trova è offline.";

export default function VideoUnavailable() {
  useEffect(() => {
    const onError = async (e: Event) => {
      const v = e.target;
      if (!(v instanceof HTMLVideoElement) || v.dataset.unavailable) return;
      const src = v.currentSrc || v.src;
      if (!src.includes("/api/")) return;
      const r = await fetch(src, { method: "HEAD" }).catch(() => null);
      if (r?.status !== 503) return;
      v.dataset.unavailable = "1";
      const box = document.createElement("div");
      box.className = "video-unavailable";
      box.textContent = MESSAGE;
      const parent = v.parentElement;
      if (parent && getComputedStyle(parent).position !== "static") {
        box.classList.add("over");
        parent.appendChild(box);
      } else {
        box.style.aspectRatio = `${v.clientWidth || 16} / ${v.clientHeight || 9}`;
        v.insertAdjacentElement("afterend", box);
        v.style.display = "none";
      }
    };
    document.addEventListener("error", onError, true);
    return () => document.removeEventListener("error", onError, true);
  }, []);
  return null;
}
