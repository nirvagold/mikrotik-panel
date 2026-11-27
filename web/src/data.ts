// Data panel bersama: dimuat sekali, diperbarui otomatis lewat event realtime.
import { computed, signal } from '@preact/signals';
import { get } from './api';
import { on } from './events';

export type Customer = {
  name: string; disabled: boolean; profile: string | null; limitation: string | null; field: string | null;
  limit: number | null; reset: string | null; interval: string | null; online: boolean; address: string | null;
  uptime: string | null; editable: boolean; used: number | null; remaining: number | null; approximate: boolean;
  cycle_start: string | null;
};
export type Day = { date: string; download: number | null; upload: number | null; coverage: number; full: boolean };
export type Dashboard = {
  sampled_at: number | null; customers: Customer[]; history: Day[];
  resources: Record<string, string>; interfaces: Record<string, string>[];
};

export const dashboard = signal<Dashboard | null>(null);
export const historyDays = signal(7);
export const routerStatus = signal<{ ok: boolean; error?: string; hint?: string }>({ ok: true });
export const loadError = signal<string | null>(null);

export const lowQuota = computed(() =>
  (dashboard.value?.customers ?? [])
    .filter(c => c.remaining != null && c.limit && c.remaining <= c.limit * 0.2)
    .sort((a, b) => a.remaining! / a.limit! - b.remaining! / b.limit!));

let pending: Promise<void> | null = null;
let pendingDays = 0;

export function refreshDashboard(): Promise<void> {
  // Permintaan yang sedang jalan dipakai bersama, kecuali periode grafik sudah diganti
  // (7 ↔ 30 hari): saat itu muat ulang setelahnya supaya grafik sesuai pilihan.
  if (pending && pendingDays !== historyDays.value) return pending.then(() => refreshDashboard());
  if (pending) return pending;
  const days = (pendingDays = historyDays.value);
  pending = get<Dashboard>(`/api/dashboard?days=${days}`)
    .then(d => { if (days === historyDays.value) { dashboard.value = d; loadError.value = null; } })
    .catch(e => { loadError.value = e.message; })
    .finally(() => { pending = null; });
  return pending;
}

export function startData() {
  on('telemetry.update', () => void refreshDashboard());
  on('router.status', (s: { ok: boolean; error?: string; hint?: string }) => { routerStatus.value = s; });
  on('resync', () => void refreshDashboard());
  void refreshDashboard();
}
