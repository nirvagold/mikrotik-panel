// Isi tiap jenis blok di design spec. Layout (grid, lebar) diurus DesignPage dari modul agen.
import { useEffect, useState } from 'preact/hooks';
import type { Block } from './agent';
import { get } from './api';
import { dashboard, historyDays, loadError, lowQuota, refreshDashboard, routerStatus, type Customer } from './data';
import { ago, bytes, gib, shortDate } from './format';
import { on } from './events';

type Props<K extends Block['kind']> = { block: Extract<Block, { kind: K }> };

function Waiting() {
  if (loadError.value) return <p class="muted">Data gagal dimuat: {loadError.value}</p>;
  return <p class="muted">Memuat data…</p>;
}

const METRIC_LABELS: Record<string, string> = {
  customers_online: 'Pelanggan online', customers_total: 'Total pelanggan', low_quota: 'Kuota menipis',
  cpu: 'CPU router', memory: 'RAM router', uptime: 'Uptime router', traffic_today: 'Trafik hari ini', temperature: 'Suhu',
};

function metricValue(item: string): string {
  const d = dashboard.value!;
  const r = d.resources ?? {};
  const today = d.history.at(-1);
  switch (item) {
    case 'customers_online': return String(d.customers.filter(c => c.online).length);
    case 'customers_total': return String(d.customers.length);
    case 'low_quota': return String(lowQuota.value.length);
    case 'cpu': return r['cpu-load'] != null ? `${r['cpu-load']}%` : '—';
    case 'memory': {
      const total = Number(r['total-memory']), free = Number(r['free-memory']);
      return total ? `${Math.round(100 * (1 - free / total))}%` : '—';
    }
    case 'uptime': return r.uptime ?? '—';
    case 'traffic_today': return today && today.download != null ? bytes((today.download ?? 0) + (today.upload ?? 0)) : '—';
    case 'temperature': return r.temperature ? `${r.temperature}°C` : '—';
    default: return '—';
  }
}

export function Metrics({ block }: Props<'metrics'>) {
  if (!dashboard.value) return <Waiting />;
  return (
    <div class="metrics">
      {block.props.items.map(item => (
        <div class="metric" key={item}>
          <span class="metric-label">{METRIC_LABELS[item] ?? item}</span>
          <strong class="metric-value">{metricValue(item)}</strong>
        </div>
      ))}
      <p class="metric-foot muted">
        {routerStatus.value.ok ? `Diperbarui ${ago(dashboard.value.sampled_at)}` : `Router tidak terjangkau: ${routerStatus.value.error ?? ''}`}
      </p>
    </div>
  );
}

export function TrafficBars({ history }: { history: { date: string; download: number | null; upload: number | null; full: boolean }[] }) {
  const max = Math.max(1, ...history.map(d => (d.download ?? 0) + (d.upload ?? 0)));
  const w = 100 / history.length;
  return (
    <figure class="traffic">
      <svg viewBox="0 0 100 50" preserveAspectRatio="none" role="img" aria-label="Trafik harian">
        {history.map((d, i) => {
          const dl = ((d.download ?? 0) / max) * 46, ul = ((d.upload ?? 0) / max) * 46;
          return (
            <g key={d.date}>
              <title>{`${d.date}: unduh ${bytes(d.download)}, unggah ${bytes(d.upload)}${d.full ? '' : ' (data belum lengkap)'}`}</title>
              <rect x={i * w + w * 0.15} y={50 - dl - ul} width={w * 0.7} height={dl} class="bar-dl" />
              <rect x={i * w + w * 0.15} y={50 - ul} width={w * 0.7} height={ul} class="bar-ul" />
            </g>
          );
        })}
      </svg>
      <div class="traffic-axis" style={{ gridTemplateColumns: `repeat(${history.length}, 1fr)` }}>
        {history.map((d, i) => <span key={d.date}>{history.length <= 14 || i % 3 === 0 ? shortDate(d.date) : ''}</span>)}
      </div>
      <figcaption class="legend"><span class="dot dl" /> Unduh <span class="dot ul" /> Unggah</figcaption>
    </figure>
  );
}

export function TrafficChart({ block }: Props<'traffic_chart'>) {
  const days = block.props.days;
  const account = block.props.account;
  const [own, setOwn] = useState<{ history: any[] } | null>(null);
  useEffect(() => {
    if (account) {
      const load = () => get<{ history: any[] }>(`/api/customers/${encodeURIComponent(account)}?days=${days > 7 ? 30 : 7}`).then(setOwn).catch(() => {});
      load();
      return on('telemetry.update', load);
    }
    if (days > 7 && historyDays.value < 30) { historyDays.value = 30; void refreshDashboard(); }
    return undefined;
  }, [account, days]);
  const history = account ? own?.history : dashboard.value?.history;
  if (!history) return <Waiting />;
  const sliced = history.slice(-days);
  const total = sliced.reduce((s, d) => s + (d.download ?? 0) + (d.upload ?? 0), 0);
  return (
    <div>
      <h2 class="block-title">Trafik {days} hari{account ? ` · ${account}` : ''}</h2>
      {total > 0 ? <TrafficBars history={sliced} /> : <p class="muted empty">Belum ada data trafik. Grafik terisi setelah poller berjalan beberapa saat.</p>}
      {total > 0 && <p class="muted small">Total {bytes(total)}</p>}
    </div>
  );
}

const COLUMN_LABELS: Record<string, string> = {
  name: 'Pelanggan', status: 'Status', address: 'Alamat IP', profile: 'Profil', used: 'Terpakai',
  remaining: 'Sisa', limit: 'Batas', uptime: 'Uptime', actions: '',
};

function cell(c: Customer, col: string) {
  switch (col) {
    case 'name': return <a class="acct" href={`/pelanggan?akun=${encodeURIComponent(c.name)}`}>{c.name}</a>;
    case 'status': {
      const k = c.disabled ? 'off' : c.online ? 'ok' : 'idle';
      return <span class={`status ${k}`}><span class="status-dot" aria-hidden="true" />{c.disabled ? 'nonaktif' : c.online ? 'online' : 'offline'}</span>;
    }
    case 'address': return c.address ?? '—';
    case 'profile': return c.profile ?? '—';
    case 'used': {
      if (!c.limit) return gib(c.used);
      const pct = Math.min(100, Math.round((100 * (c.used ?? 0)) / c.limit));
      const tone = pct >= 90 ? 'bad' : pct >= 70 ? 'warn' : '';
      return <span class="usage"><span class={`usage-bar ${tone}`} role="meter" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label={`${pct}% terpakai`}><span style={{ width: `${pct}%` }} /></span>{gib(c.used)}</span>;
    }
    case 'remaining': return c.approximate ? 'perkiraan' : gib(c.remaining);
    case 'limit': return gib(c.limit);
    case 'uptime': return c.uptime ?? '—';
    case 'actions': return <a class="link-button" href={`/pelanggan?akun=${encodeURIComponent(c.name)}`}>Detail</a>;
    default: return '';
  }
}

export function CustomerTable({ block }: Props<'customer_table'>) {
  const [q, setQ] = useState('');
  if (!dashboard.value) return <Waiting />;
  const { filter, columns, row_limit, title } = block.props;
  const needle = q.trim().toLowerCase();
  const rows = dashboard.value.customers
    .filter(c => filter === 'online' ? c.online : filter === 'offline' ? !c.online : filter === 'low_quota' ? lowQuota.value.includes(c) : true)
    .filter(c => !needle || `${c.name} ${c.address ?? ''} ${c.profile ?? ''}`.toLowerCase().includes(needle));
  return (
    <div>
      <div class="block-head">
        <h2 class="block-title">{title}</h2>
        <input class="search" type="search" placeholder="Cari nama, IP, profil" value={q} onInput={e => setQ((e.target as HTMLInputElement).value)} aria-label="Cari pelanggan" />
      </div>
      <div class="table-wrap">
        <table>
          <thead><tr>{columns.map(col => <th key={col}>{COLUMN_LABELS[col] ?? col}</th>)}</tr></thead>
          <tbody>
            {rows.slice(0, row_limit).map(c => <tr key={c.name}>{columns.map(col => <td key={col} data-col={col}>{cell(c, col)}</td>)}</tr>)}
          </tbody>
        </table>
      </div>
      <p class="muted">{rows.length > row_limit ? `Menampilkan ${row_limit} dari ${rows.length}` : `${rows.length} pelanggan`}</p>
    </div>
  );
}

export function LowQuota({ block }: Props<'low_quota'>) {
  if (!dashboard.value) return <Waiting />;
  const rows = lowQuota.value.slice(0, block.props.limit);
  return (
    <div>
      <h2 class="block-title">Kuota menipis</h2>
      {rows.length === 0 ? <p class="muted">Tidak ada pelanggan dengan sisa kuota di bawah 20%.</p> : (
        <ul class="quota-list">
          {rows.map(c => {
            const pct = Math.round((100 * c.remaining!) / c.limit!);
            return (
              <li key={c.name}>
                <a href={`/pelanggan?akun=${encodeURIComponent(c.name)}`}>{c.name}</a>
                <span class="meter" role="meter" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}><span style={{ width: `${pct}%` }} /></span>
                <span class="muted">{gib(c.remaining)} ({pct}%)</span>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

export function RouterStatus(_: Props<'router_status'>) {
  if (!dashboard.value) return <Waiting />;
  const r = dashboard.value.resources ?? {};
  const rows: [string, string][] = [
    ['Model', r['board-name'] ?? '—'], ['RouterOS', r.version ?? '—'], ['CPU', r['cpu-load'] != null ? `${r['cpu-load']}%` : '—'],
    ['RAM bebas', r['free-memory'] ? `${bytes(Number(r['free-memory']))} dari ${bytes(Number(r['total-memory']))}` : '—'],
    ['Uptime', r.uptime ?? '—'], ['Sinkron terakhir', ago(dashboard.value.sampled_at)],
  ];
  return (
    <div>
      <h2 class="block-title">Status router</h2>
      {!routerStatus.value.ok && <p class="alert">Router tidak terjangkau: {routerStatus.value.error}</p>}
      <dl class="facts">{rows.map(([k, v]) => <><dt>{k}</dt><dd>{v}</dd></>)}</dl>
      <h3 class="sub-title">Interface ethernet</h3>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Nama</th><th>Status</th><th>Terima</th><th>Kirim</th></tr></thead>
          <tbody>
            {(dashboard.value.interfaces ?? []).map(i => (
              <tr key={i.name}><td>{i.name}</td><td>{i.running === 'true' ? 'aktif' : 'mati'}</td><td>{bytes(Number(i['rx-byte']))}</td><td>{bytes(Number(i['tx-byte']))}</td></tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function Logs({ block }: Props<'logs'>) {
  const [rows, setRows] = useState<Record<string, string>[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = () => {
    const params = new URLSearchParams({ limit: String(block.props.limit) });
    if (block.props.topics.length) params.set('topics', block.props.topics.join(','));
    get<{ logs: Record<string, string>[] }>(`/api/router/logs?${params}`).then(r => { setRows(r.logs); setError(null); }).catch(e => setError(e.message));
  };
  useEffect(load, [block.props.limit, block.props.topics.join(',')]);
  return (
    <div>
      <div class="block-head"><h2 class="block-title">Log router</h2><button type="button" class="ghost" onClick={load}>Muat ulang</button></div>
      {error ? <p class="alert">{error}</p> : !rows ? <p class="muted">Memuat log…</p> : rows.length === 0 ? <p class="muted">Tidak ada log.</p> : (
        <ol class="log-list">{rows.map((r, i) => <li key={i}><time>{r.time}</time> <span class="muted">{r.topics}</span> {r.message}</li>)}</ol>
      )}
    </div>
  );
}

type Recommendation = { id: string; kind: string; finding: string; evidence: string | null; status: string };

export function Recommendations({ block }: Props<'recommendations'>) {
  const [rows, setRows] = useState<Recommendation[] | null>(null);
  useEffect(() => { get<{ recommendations: Recommendation[] }>('/api/recommendations').then(r => setRows(r.recommendations)).catch(() => setRows([])); }, []);
  const open = (rows ?? []).filter(r => r.status === 'open').slice(0, block.props.limit);
  return (
    <div>
      <h2 class="block-title">Saran agen</h2>
      {open.length === 0 ? <p class="muted">Belum ada saran. Minta agen “audit router” untuk membuat saran.</p> : (
        <ul class="plain-list">{open.map(r => <li key={r.id}><span class="badge idle">{r.kind}</span> {r.finding}</li>)}</ul>
      )}
    </div>
  );
}

export function Topology(_: Props<'topology'>) {
  return (
    <div>
      <h2 class="block-title">Topologi</h2>
      <p class="muted">Diagram topologi dari neighbor, bridge, dan ARP akan tampil di sini setelah kolektor snapshot router aktif.</p>
    </div>
  );
}

export function ActionButton({ block }: Props<'action_button'>) {
  return <button type="button" disabled title="Tombol aksi custom akan aktif di tahap berikutnya">{block.props.label ?? 'Tombol aksi'}</button>;
}
