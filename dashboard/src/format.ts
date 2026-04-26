export function formatBytes(bytes: bigint): string {
  const n = Number(bytes);
  if (n >= 1_000_000_000_000) return `${(n / 1_000_000_000_000).toFixed(2)} TB`;
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)} GB`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)} MB`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(2)} KB`;
  return `${n} B`;
}
