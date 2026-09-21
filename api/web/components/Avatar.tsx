function hue(s: string): number {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

export function initials(name: string): string {
  const parts = name.trim().split(/[\s._-]+/).filter(Boolean);
  const a = parts[0]?.[0] ?? "?";
  const b = parts.length > 1 ? parts[parts.length - 1][0] : "";
  return (a + b).toUpperCase();
}

export default function Avatar({ name, size = 28 }: { name: string; size?: number }) {
  const h = hue(name);
  return (
    <span
      className="avatar"
      aria-hidden="true"
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.4),
        background: `hsl(${h} 45% 34%)`,
        color: `hsl(${h} 80% 92%)`,
      }}
    >
      {initials(name)}
    </span>
  );
}

export function AvatarStack({ names, max = 4 }: { names: string[]; max?: number }) {
  const shown = names.slice(0, max);
  const extra = names.length - shown.length;
  return (
    <span className="avatars" aria-hidden="true">
      {shown.map((n, i) => (
        <Avatar key={`${n}-${i}`} name={n} />
      ))}
      {extra > 0 && <span className="avatar more">+{extra}</span>}
    </span>
  );
}
