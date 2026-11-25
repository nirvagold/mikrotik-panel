// Blok bawaan: halaman yang isinya tetap (detail pelanggan, riwayat, pengaturan AI, memori agen).
import { useEffect, useState } from 'preact/hooks';
import type { Block } from './agent';
import { get, post, put } from './api';
import { TrafficBars } from './blocks';
import { refreshDashboard, type Customer, type Day } from './data';
import { on } from './events';
import { ago, dateTime, gib } from './format';
import { navigate, query } from './router';

type Props = { block: Extract<Block, { kind: 'builtin' }> };

export function Builtin({ block }: Props) {
  switch (block.props.name) {
    case 'customer_detail': return <CustomerDetail />;
    case 'audit': return <Audit />;
    case 'ai_settings': return <AiSettings />;
    case 'agent_memory': return <AgentMemory />;
    case 'agent': return <p class="muted">Panel agen ada di sisi kanan layar. Tulis permintaan di sana, misalnya “buat tema gelap” atau “siapa yang kuotanya hampir habis?”.</p>;
    case 'actions': return <Actions />;
    default: return <p class="muted">Blok bawaan “{block.props.name}” belum tersedia.</p>;
  }
}

type Op = { id: string; account: string; profile: string; limitation: string; field: string; before: number; after: number };

function CustomerDetail() {
  const account = query.value.get('akun');
  const [data, setData] = useState<{ customer: Customer; history: Day[]; sampled_at: number | null } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [amount, setAmount] = useState('10');
  const [op, setOp] = useState<Op | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [logs, setLogs] = useState<Record<string, string>[] | null>(null);

  const load = () => {
    if (!account) return;
    get<typeof data>(`/api/customers/${encodeURIComponent(account)}?days=30`).then(d => { setData(d); setError(null); }).catch(e => setError(e.message));
  };
  useEffect(() => { setData(null); setOp(null); setNotice(null); setLogs(null); load(); return account ? on('telemetry.update', load) : undefined; }, [account]);

  if (!account) return <p class="muted">Pilih pelanggan di tabel untuk melihat detail dan menambah kuota.</p>;
  if (error) return <p class="alert">{error}</p>;
  if (!data) return <p class="muted">Memuat {account}…</p>;
  const c = data.customer;

  const preview = async (e: Event) => {
    e.preventDefault();
    setBusy(true); setNotice(null);
    try { setOp(await post<Op>(`/api/customers/${encodeURIComponent(c.name)}/quota/preview`, { amount_gib: Number(amount) })); }
    catch (err) { setNotice((err as Error).message); }
    finally { setBusy(false); }
  };
  const apply = async () => {
    if (!op) return;
    setBusy(true);
    try {
      const r = await post<{ status: string }>(`/api/customers/${encodeURIComponent(c.name)}/quota/apply`, { id: op.id });
      setNotice(r.status === 'Terverifikasi' ? 'Kuota baru sudah dikonfirmasi oleh User Manager.' : `${r.status}. Baca ulang profil sebelum mencoba lagi.`);
      setOp(null);
      load();
      void refreshDashboard();
    } catch (err) { setNotice((err as Error).message); }
    finally { setBusy(false); }
  };

  return (
    <div class="detail">
      <div class="block-head">
        <h2 class="block-title">{c.name}</h2>
        <button type="button" class="ghost" onClick={() => navigate(location.pathname)}>Tutup</button>
      </div>
      <dl class="facts">
        <dt>Status</dt><dd>{c.disabled ? 'nonaktif' : c.online ? `online, ${c.address ?? ''} (${c.uptime ?? ''})` : 'offline'}</dd>
        <dt>Profil</dt><dd>{c.profile ?? '—'}</dd>
        <dt>Batas kuota</dt><dd>{gib(c.limit)} {c.field ? `(${c.field})` : ''}</dd>
        <dt>Terpakai siklus ini</dt><dd>{gib(c.used)}{c.cycle_start ? ` sejak ${c.cycle_start}` : ''}</dd>
        <dt>Sisa</dt><dd>{c.approximate ? 'perkiraan (sesi melintasi awal siklus)' : gib(c.remaining)}</dd>
      </dl>
      <h3 class="sub-title">Trafik 30 hari</h3>
      <TrafficBars history={data.history} />
      <h3 class="sub-title">Tambah kuota</h3>
      {!c.editable ? <p class="muted">Top-up hanya untuk profil dengan satu batas kuota khusus pelanggan ini.</p> : op ? (
        <div class="confirm">
          <p>Batas {op.field} untuk <strong>{op.account}</strong> berubah dari <strong>{gib(op.before)}</strong> menjadi <strong>{gib(op.after)}</strong>.</p>
          <div class="row"><button type="button" onClick={apply} disabled={busy}>Terapkan</button><button type="button" class="ghost" onClick={() => setOp(null)} disabled={busy}>Batal</button></div>
        </div>
      ) : (
        <form class="row" onSubmit={preview}>
          <label>Tambah <input type="number" min={1} max={1000} value={amount} onInput={e => setAmount((e.target as HTMLInputElement).value)} required /> GiB</label>
          <button type="submit" disabled={busy}>Pratinjau</button>
        </form>
      )}
      {notice && <p class="notice">{notice}</p>}
      <h3 class="sub-title">Log PPPoE</h3>
      {logs ? (logs.length ? <ol class="log-list">{logs.map((l, i) => <li key={i}><time>{l.time}</time> {l.message}</li>)}</ol> : <p class="muted">Tidak ada log untuk akun ini.</p>)
        : <button type="button" class="ghost" onClick={() => get<{ logs: Record<string, string>[] }>(`/api/customers/${encodeURIComponent(c.name)}/logs`).then(r => setLogs(r.logs)).catch(e => setNotice(e.message))}>Muat log</button>}
    </div>
  );
}

type AuditRow = { id: string; at: number; actor: string; account: string | null; action: string; before: string | null; after: string | null; status: string };

function Audit() {
  const [rows, setRows] = useState<AuditRow[] | null>(null);
  useEffect(() => { get<{ audit: AuditRow[] }>('/api/audit').then(r => setRows(r.audit)).catch(() => setRows([])); }, []);
  if (!rows) return <p class="muted">Memuat riwayat…</p>;
  const show = (r: AuditRow, v: string | null) => v && r.action === 'quota.increase' ? gib(Number(v)) : (v ?? '—');
  return (
    <div>
      <h2 class="block-title">Riwayat perubahan</h2>
      {rows.length === 0 ? <p class="muted">Belum ada perubahan.</p> : (
        <div class="table-wrap"><table>
          <thead><tr><th>Waktu</th><th>Oleh</th><th>Tindakan</th><th>Pelanggan</th><th>Sebelum</th><th>Sesudah</th><th>Status</th></tr></thead>
          <tbody>{rows.map(r => <tr key={r.id}><td>{dateTime(r.at)}</td><td>{r.actor}</td><td>{r.action}</td><td>{r.account ?? '—'}</td><td>{show(r, r.before)}</td><td>{show(r, r.after)}</td><td>{r.status}</td></tr>)}</tbody>
        </table></div>
      )}
    </div>
  );
}

function AiSettings() {
  const [form, setForm] = useState({ url: '', protocol: 'anthropic', model: '', api_key: '' });
  const [hasKey, setHasKey] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    get<{ url: string | null; protocol: string | null; model: string | null; has_key: boolean }>('/api/settings/ai').then(s => {
      setForm(f => ({ ...f, url: s.url ?? 'https://api.anthropic.com/v1', protocol: s.protocol ?? 'anthropic', model: s.model ?? '' }));
      setHasKey(s.has_key);
    }).catch(e => setNotice(e.message));
  }, []);
  const set = (k: keyof typeof form) => (e: Event) => setForm({ ...form, [k]: (e.target as HTMLInputElement).value });
  const save = async (e: Event) => {
    e.preventDefault();
    try {
      await put('/api/settings/ai', { ...form, api_key: form.api_key || null });
      setNotice('Pengaturan AI disimpan.'); setHasKey(hasKey || !!form.api_key); setForm({ ...form, api_key: '' });
    } catch (err) { setNotice((err as Error).message); }
  };
  return (
    <form class="stack" onSubmit={save}>
      <h2 class="block-title">Pengaturan AI</h2>
      <label>Protokol<select value={form.protocol} onChange={set('protocol')}><option value="anthropic">Anthropic (Claude)</option><option value="openai">OpenAI-compatible</option></select></label>
      <label>Endpoint (https)<input type="url" value={form.url} onInput={set('url')} required /></label>
      <label>Model<input value={form.model} onInput={set('model')} required maxLength={150} /></label>
      <label>API key<input type="password" autocomplete="off" value={form.api_key} onInput={set('api_key')} placeholder={hasKey ? 'Tersimpan; kosongkan untuk tetap memakai key lama' : ''} /></label>
      <div class="row"><button type="submit">Simpan</button></div>
      {notice && <p class="notice">{notice}</p>}
    </form>
  );
}

type Memory = { id: string; kind: string; text: string; updated_at: number };

function AgentMemory() {
  const [rows, setRows] = useState<Memory[] | null>(null);
  const load = () => get<{ memories: Memory[] }>('/api/agent/memories').then(r => setRows(r.memories)).catch(() => setRows([]));
  useEffect(() => { void load(); }, []);
  const forget = async (id: string) => { await post(`/api/agent/memories/${id}/delete`); void load(); };
  if (!rows) return <p class="muted">Memuat memori…</p>;
  return (
    <div>
      <h2 class="block-title">Memori agen</h2>
      <p class="muted">Hal yang diingat agen tentang kamu dan jaringanmu. Hapus yang tidak benar.</p>
      {rows.length === 0 ? <p class="muted">Belum ada memori.</p> : (
        <ul class="plain-list">{rows.map(m => <li key={m.id}><span class="badge idle">{m.kind}</span> {m.text} <span class="muted">· {ago(m.updated_at)}</span> <button type="button" class="ghost small" onClick={() => forget(m.id)}>Hapus</button></li>)}</ul>
      )}
    </div>
  );
}

type Action = { id: string; label: string; risk: string; approved_at: number | null };

function Actions() {
  const [rows, setRows] = useState<Action[] | null>(null);
  useEffect(() => { get<{ actions: Action[] }>('/api/actions').then(r => setRows(r.actions)).catch(() => setRows([])); }, []);
  if (!rows) return <p class="muted">Memuat tombol aksi…</p>;
  return (
    <div>
      <h2 class="block-title">Tombol aksi</h2>
      {rows.length === 0 ? <p class="muted">Belum ada tombol aksi. Minta agen membuatnya, misalnya “buat tombol putus sesi PPPoE pelanggan”.</p> : (
        <ul class="plain-list">{rows.map(a => <li key={a.id}><span class={`badge ${a.risk === 'critical' ? 'off' : 'idle'}`}>{a.risk}</span> {a.label} {a.approved_at ? '' : <span class="muted">(menunggu persetujuan)</span>}</li>)}</ul>
      )}
    </div>
  );
}
