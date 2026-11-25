export const GIB = 1024 ** 3;

export function gib(bytes: number | null | undefined): string {
  if (bytes == null) return '—';
  return (bytes / GIB).toLocaleString('id-ID', { maximumFractionDigits: 2, minimumFractionDigits: 2 }) + ' GiB';
}

export function bytes(n: number | null | undefined): string {
  if (n == null) return '—';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let v = n, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return v.toLocaleString('id-ID', { maximumFractionDigits: i ? 1 : 0 }) + ' ' + units[i];
}

export function ago(epoch: number | null | undefined): string {
  if (!epoch) return 'belum pernah';
  const s = Math.max(0, Math.round(Date.now() / 1000 - epoch));
  if (s < 60) return `${s} detik lalu`;
  if (s < 3600) return `${Math.round(s / 60)} menit lalu`;
  return `${Math.round(s / 3600)} jam lalu`;
}

export function dateTime(epoch: number): string {
  return new Date(epoch * 1000).toLocaleString('id-ID', { dateStyle: 'medium', timeStyle: 'short' });
}

export function shortDate(iso: string): string {
  const [, m, d] = iso.split('-').map(Number);
  return `${d}/${m}`;
}
