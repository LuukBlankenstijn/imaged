import type { Timestamp } from "@bufbuild/protobuf/wkt";

export function formatBytes(bytes: bigint): string {
  const n = Number(bytes);
  const KiB = 1024;
  const MiB = KiB * 1024;
  const GiB = MiB * 1024;
  const TiB = GiB * 1024;
  if (n >= TiB) return `${(n / TiB).toFixed(2)} TiB`;
  if (n >= GiB) return `${(n / GiB).toFixed(2)} GiB`;
  if (n >= MiB) return `${(n / MiB).toFixed(2)} MiB`;
  if (n >= KiB) return `${(n / KiB).toFixed(2)} KiB`;
  return `${n} B`;
}

export function formatRelative(ts?: Timestamp): string {
  if (!ts) return "—";
  const date = new Date(Number(ts.seconds) * 1000 + ts.nanos / 1e6);
  const diff = (date.getTime() - Date.now()) / 1000;
  const abs = Math.abs(diff);
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  if (abs < 60) return rtf.format(Math.round(diff), "second");
  if (abs < 3600) return rtf.format(Math.round(diff / 60), "minute");
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), "hour");
  if (abs < 86400 * 30) return rtf.format(Math.round(diff / 86400), "day");
  if (abs < 86400 * 365) return rtf.format(Math.round(diff / (86400 * 30)), "month");
  return rtf.format(Math.round(diff / (86400 * 365)), "year");
}

export function timestampToDate(ts?: Timestamp): Date | null {
  if (!ts) return null;
  return new Date(Number(ts.seconds) * 1000 + ts.nanos / 1e6);
}
