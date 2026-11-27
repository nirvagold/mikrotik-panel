// Isi tiap jenis blok di design spec. Layout (grid, lebar) diurus DesignPage dari modul agen.
import { useEffect, useState } from 'preact/hooks';
import type { Block } from './agent';
import { get, post } from './api';
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
type RecEvidence = { title?: string; evidence?: unknown; impact?: string; fix?: { command?: unknown } | null };

const KIND_LABEL: Record<string, string> = { performance: 'performa', security: 'keamanan', topology: 'topologi', layout: 'tampilan' };

export function Recommendations({ block }: Props<'recommendations'>) {
  const [rows, setRows] = useState<Recommendation[] | null>(null);
  const [msg, setMsg] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState('');
  const load = () => get<{ recommendations: Recommendation[] }>('/api/recommendations').then(r => setRows(r.recommendations)).catch(() => setRows([]));
  useEffect(() => { void load(); return on('recommendations.updated', () => void load()); }, []);
  const open = (rows ?? []).filter(r => r.status === 'open').slice(0, block.props.limit);
  const [need, setNeed] = useState<Record<string, { phrase: string; typed: string }>>({});
  const act = async (r: Recommendation, kind: 'accept' | 'dismiss', typed?: string) => {
    setBusy(r.id);
    try {
      const res = await post<{ message?: string }>(`/api/recommendations/${r.id}/${kind}`, typed ? { typed } : {});
      if (res.message) setMsg(m => ({ ...m, [r.id]: res.message! }));
      await load();
    } catch (e) {
      const text = (e as Error).message;
      const m = /Ketik "([^"]+)"/.exec(text);
      if (m && kind === 'accept') setNeed(n => ({ ...n, [r.id]: { phrase: m[1], typed: '' } }));
      else setMsg(x => ({ ...x, [r.id]: text }));
    } finally { setBusy(''); }
  };
  const done = Object.entries(msg).filter(([id]) => !open.some(r => r.id === id));
  return (
    <div>
      <h2 class="block-title">Saran agen</h2>
      {open.length === 0 ? <p class="muted">Belum ada saran. Minta agen “audit router” untuk membuat saran.</p> : (
        <ul class="rec-list">{open.map(r => {
          const ev: RecEvidence = (() => { try { return JSON.parse(r.evidence ?? '{}'); } catch { return {}; } })();
          const [title, ...rest] = r.finding.split(' — ');
          return (
            <li key={r.id}>
              <div class="rec-head"><span class="badge idle">{KIND_LABEL[r.kind] ?? r.kind}</span><b>{title}</b></div>
              {rest.length > 0 && <p>{rest.join(' — ')}</p>}
              {ev.impact && <p class="muted small">Dampak: {ev.impact}</p>}
              {msg[r.id] && <p class="alert small">{msg[r.id]}</p>}
              {need[r.id] && <label class="small">Perintah kritis. Ketik <b>{need[r.id].phrase}</b> lalu tekan Terapkan lagi:
                <input type="text" value={need[r.id].typed} autocomplete="off" onInput={e => { const v = (e.target as HTMLInputElement).value; setNeed(n => ({ ...n, [r.id]: { ...n[r.id], typed: v } })); }} /></label>}
              <div class="rec-actions">
                <button type="button" class="pill-btn primary" disabled={busy === r.id || (!!need[r.id] && need[r.id].typed.trim().toLowerCase() !== need[r.id].phrase)} onClick={() => void act(r, 'accept', need[r.id]?.typed)}>{ev.fix?.command ? 'Terapkan perbaikan' : 'Tandai selesai'}</button>
                <button type="button" class="pill-btn" disabled={busy === r.id} onClick={() => void act(r, 'dismiss')}>Abaikan</button>
              </div>
            </li>
          );
        })}</ul>
      )}
      {done.map(([id, m]) => <p key={id} class="muted small">{m}</p>)}
    </div>
  );
}

type Topo = {
  router: string;
  interfaces: { name: string; type: string; running: boolean; disabled: boolean; comment: string; arp_hosts: number }[];
  bridges: Record<string, { interface: string; pvid: string; disabled: boolean }[]>;
  vlans: Record<string, string>[];
  addresses: Record<string, string>[];
  neighbors: Record<string, string>[];
};

export function Topology(_: Props<'topology'>) {
  const [t, setT] = useState<Topo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => { get<Topo>('/api/topology').then(setT).catch(e => setErr((e as Error).message)); }, []);
  if (err) return <div><h2 class="block-title">Topologi</h2><p class="muted">Topologi gagal dibaca: {err}</p></div>;
  if (!t) return <div><h2 class="block-title">Topologi</h2><p class="muted">Membaca topologi router…</p></div>;
  const inBridge = new Set(Object.values(t.bridges).flat().map(p => p.interface));
  const addr = (name: string) => t.addresses.filter(a => a.interface === name).map(a => a.address).join(', ');
  const neigh = (name: string) => t.neighbors.filter(n => n.interface === name || n.interface?.split(',').includes(name));
  const port = (name: string) => {
    const i = t.interfaces.find(x => x.name === name);
    const ns = neigh(name);
    return (
      <li key={name} class={i?.disabled ? 'off' : i?.running ? 'up' : 'down'}>
        <span class="dot" /> <b>{name}</b>{i?.comment ? <span class="muted"> · {i.comment}</span> : null}
        {addr(name) && <span class="mono muted"> {addr(name)}</span>}
        {i && i.arp_hosts > 0 && <span class="muted"> · {i.arp_hosts} host</span>}
        {ns.map(n => <div key={n['mac-address']} class="topo-neigh">↳ {n.identity || n.address || n['mac-address']} <span class="muted">{[n.platform, n.board, n.address].filter(Boolean).join(' · ')}</span></div>)}
      </li>
    );
  };
  const loose = t.interfaces.filter(i => !inBridge.has(i.name) && !(i.name in t.bridges) && ['ether', 'wlan', 'sfp', 'vlan', 'pppoe-out', 'wg', 'lte'].some(k => i.type.startsWith(k)));
  return (
    <div class="topo">
      <h2 class="block-title">Topologi · {t.router}</h2>
      {Object.entries(t.bridges).map(([b, ports]) => (
        <div key={b} class="topo-group"><p class="topo-title"><b>{b}</b> <span class="muted">bridge{addr(b) ? ` · ${addr(b)}` : ''}</span></p><ul>{ports.map(p => port(p.interface))}</ul></div>
      ))}
      {loose.length > 0 && <div class="topo-group"><p class="topo-title"><b>Port lain</b></p><ul>{loose.map(i => port(i.name))}</ul></div>}
    </div>
  );
}

type Param = { name: string; label: string; type: 'text' | 'integer' | 'choice' | 'customer' | 'profile'; max_len?: number; min?: number; max?: number; values?: string[] };
type Run = { id: string; command: string; result: string | null; at: number; revertible: boolean };
type ActionInfo = { id: string; label: string; description: string; risk: 'read' | 'write' | 'critical'; params: Param[]; preview: string | null; confirm: string | null; customers?: string[]; profiles?: string[]; runs: Run[] };

const RISK_LABEL = { read: 'baca', write: 'ubah', critical: 'kritis' } as const;

export function ActionButton({ block }: Props<'action_button'>) {
  const id = block.props.action_id;
  const [info, setInfo] = useState<ActionInfo | null>(null);
  const [missing, setMissing] = useState(false);
  const [open, setOpen] = useState(false);
  const [values, setValues] = useState<Record<string, string>>({});
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const load = () => get<ActionInfo>(`/api/actions/${encodeURIComponent(id)}`).then(a => { setInfo(a); setMissing(false); }).catch(() => setMissing(true));
  useEffect(() => { void load(); return on('actions.updated', () => void load()); }, [id]);
  const label = block.props.label || info?.label || 'Tombol aksi';
  if (missing) return <button type="button" disabled title="Tombol ini sudah dihapus atau belum disetujui">{label}</button>;
  if (!info) return <button type="button" disabled>{label}</button>;

  const value = (p: Param): string | number => p.type === 'integer' ? Number(values[p.name]) : (values[p.name] ?? '');
  const ready = info.params.every(p => (values[p.name] ?? '') !== '') && (!info.confirm || typed.trim().toLowerCase() === info.confirm);
  const submit = async (e: Event) => {
    e.preventDefault();
    setBusy(true); setMsg(null);
    try {
      const body = { values: Object.fromEntries(info.params.map(p => [p.name, value(p)])), confirmed: true, typed: info.confirm ? typed : undefined };
      const r = await post<{ message: string }>(`/api/actions/${encodeURIComponent(id)}/run`, body);
      setMsg({ ok: true, text: r.message }); setTyped(''); await load();
    } catch (err) { setMsg({ ok: false, text: (err as Error).message }); } finally { setBusy(false); }
  };
  const revert = async (run: Run) => {
    setBusy(true);
    try { const r = await post<{ message: string }>(`/api/action-runs/${run.id}/revert`); setMsg({ ok: true, text: r.message }); await load(); }
    catch (err) { setMsg({ ok: false, text: (err as Error).message }); } finally { setBusy(false); }
  };
  const field = (p: Param) => {
    const set = (v: string) => setValues(x => ({ ...x, [p.name]: v }));
    const list = p.type === 'choice' ? p.values : p.type === 'customer' ? info.customers : p.type === 'profile' ? info.profiles : null;
    if (list) return (
      <select value={values[p.name] ?? ''} onChange={e => set((e.target as HTMLSelectElement).value)} required>
        <option value="">Pilih…</option>{list.map(v => <option key={v} value={v}>{v}</option>)}
      </select>
    );
    if (p.type === 'integer') return <input type="number" min={p.min} max={p.max} value={values[p.name] ?? ''} onInput={e => set((e.target as HTMLInputElement).value)} required />;
    return <input type="text" maxLength={p.max_len} value={values[p.name] ?? ''} onInput={e => set((e.target as HTMLInputElement).value)} required />;
  };

  return (
    <div class="action-btn">
      <button type="button" class={info.risk === 'critical' ? 'danger' : ''} onClick={() => { setOpen(o => !o); setMsg(null); }} aria-expanded={open}>{label}</button>
      {open && (
        <form class="action-form" onSubmit={submit}>
          <p class="action-head"><b>{info.label}</b><span class={`badge ${info.risk === 'critical' ? 'off' : 'idle'}`}>{RISK_LABEL[info.risk]}</span></p>
          {info.description && <p class="muted small">{info.description}</p>}
          {info.preview && <code class="action-preview">{info.preview}</code>}
          {info.params.map(p => <label key={p.name}><span>{p.label}</span>{field(p)}</label>)}
          {info.confirm && <label><span>Ketik <b>{info.confirm}</b> untuk konfirmasi</span><input type="text" value={typed} onInput={e => setTyped((e.target as HTMLInputElement).value)} autocomplete="off" /></label>}
          {info.risk === 'write' && <p class="muted small">Perintah ini mengubah router. Menekan Jalankan berarti kamu menyetujuinya.</p>}
          {msg && <p class={msg.ok ? 'update-msg' : 'alert'} role="status">{msg.text}</p>}
          <div class="update-actions">
            <button type="button" class="pill-btn" onClick={() => setOpen(false)}>Tutup</button>
            <button type="submit" class="pill-btn primary" disabled={busy || !ready}>{busy ? 'Menjalankan…' : 'Jalankan'}</button>
          </div>
          {info.runs.length > 0 && (
            <ul class="action-runs">{info.runs.map(r => (
              <li key={r.id}><span class="muted">{new Date(r.at * 1000).toLocaleString('id-ID', { dateStyle: 'short', timeStyle: 'short' })}</span> <code>{r.command}</code> <span class={r.result === 'done' ? '' : 'muted'}>{r.result === 'done' ? 'berhasil' : r.result === 'reverted' ? 'dikembalikan' : r.result === 'partial' ? 'gagal sebagian' : 'gagal'}</span>
                {(r.result === 'done' || r.result === 'partial') && r.revertible && <button type="button" class="link-btn" disabled={busy} onClick={() => void revert(r)}>Kembalikan</button>}
              </li>
            ))}</ul>
          )}
        </form>
      )}
    </div>
  );
}
