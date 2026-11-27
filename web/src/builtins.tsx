// Blok bawaan: halaman yang isinya tetap (detail pelanggan, riwayat, pengaturan AI, memori agen).
import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { Block } from './agent';
import { get, post, put } from './api';
import { ActionButton, TrafficBars } from './blocks';
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

const QUICK_GIB = [5, 10, 20, 50];

/** Kenapa top-up tidak bisa, dalam bahasa awam, plus jalan keluarnya. */
function whyNotEditable(c: Customer): string {
  if (!c.profile) return 'Pelanggan ini tidak punya tepat satu profil aktif di User Manager, jadi panel tidak tahu batas kuota mana yang harus ditambah.';
  if (!c.limitation) return `Profil ${c.profile} tidak punya batas kuota (limitation), jadi tidak ada kuota yang bisa ditambah.`;
  if (!c.field) return 'Batas kuota pelanggan ini memakai lebih dari satu jenis batas (unduh/unggah/total), jadi panel tidak tahu mana yang harus ditambah.';
  return `Batas kuota ${c.limitation} dipakai bersama profil atau pelanggan lain. Menambahnya akan ikut menambah kuota mereka juga, jadi panel tidak mengubahnya.`;
}

type Pending = { kind: 'disconnect' | 'disable' | 'enable' | 'profile'; text: string };

function CustomerDetail() {
  const account = query.value.get('akun');
  const [data, setData] = useState<{ customer: Customer; history: Day[]; sampled_at: number | null } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [amount, setAmount] = useState('10');
  const [op, setOp] = useState<Op | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [logs, setLogs] = useState<Record<string, string>[] | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [profiles, setProfiles] = useState<string[] | null>(null);
  const [newProfile, setNewProfile] = useState('');

  const load = () => {
    if (!account) return;
    const want = account;
    // Jawaban yang datang terlambat untuk pelanggan sebelumnya diabaikan, supaya tombol top-up
    // tidak pernah mengarah ke pelanggan yang salah.
    const still = () => query.value.get('akun') === want;
    get<typeof data>(`/api/customers/${encodeURIComponent(want)}?days=30`)
      .then(d => { if (still()) { setData(d); setError(null); } })
      .catch(e => { if (still()) setError(e.message); });
  };
  useEffect(() => {
    setData(null); setOp(null); setNotice(null); setLogs(null); setPending(null); setNewProfile('');
    load();
    return account ? on('telemetry.update', load) : undefined;
  }, [account]);
  // Esc menutup panel detail.
  useEffect(() => {
    if (!account) return;
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape' && !busy) close(); };
    document.addEventListener('keydown', esc);
    return () => document.removeEventListener('keydown', esc);
  }, [account, busy]);

  const close = () => navigate(location.pathname);
  if (!account) return <p class="muted small">Ketuk nama pelanggan untuk melihat detail, menambah kuota, atau memutus sesi.</p>;

  const frame = (body: ComponentChildren) => (
    <div class="drawer-backdrop" onClick={() => { if (!busy) close(); }}>
      <div class="drawer" role="dialog" aria-modal="true" aria-label={`Detail ${account}`} onClick={e => e.stopPropagation()}>
        <span class="sheet-grip" aria-hidden="true" />
        {body}
      </div>
    </div>
  );
  if (error) return frame(<><div class="block-head"><h2 class="block-title">{account}</h2><button type="button" class="ghost" onClick={close}>Tutup</button></div><p class="alert">{error}</p></>);
  if (!data || data.customer.name !== account) return frame(<p class="muted">Memuat {account}…</p>);
  const c = data.customer;
  const url = (tail: string) => `/api/customers/${encodeURIComponent(c.name)}/${tail}`;

  const preview = async (gibs: number) => {
    setBusy(true); setNotice(null);
    try { setOp(await post<Op>(url('quota/preview'), { amount_gib: gibs })); }
    catch (err) { setNotice({ ok: false, text: (err as Error).message }); }
    finally { setBusy(false); }
  };
  const apply = async () => {
    if (!op) return;
    setBusy(true);
    try {
      const r = await post<{ status: string }>(url('quota/apply'), { id: op.id });
      setNotice(r.status === 'Terverifikasi'
        ? { ok: true, text: `Kuota ${c.name} sekarang ${gib(op.after)} (sudah dicek di User Manager).` }
        : { ok: false, text: `${r.status}. Buka ulang detail ini lalu coba lagi.` });
      setOp(null);
      load();
      void refreshDashboard();
    } catch (err) { setNotice({ ok: false, text: (err as Error).message }); }
    finally { setBusy(false); }
  };
  const ask = (p: Pending) => { setNotice(null); setPending(p); };
  const confirm = async () => {
    if (!pending) return;
    setBusy(true);
    try {
      const r = pending.kind === 'disconnect' ? await post<{ message: string }>(url('disconnect'))
        : pending.kind === 'profile' ? await post<{ message: string }>(url('profile'), { profile: newProfile })
        : await post<{ message: string }>(url('enabled'), { enabled: pending.kind === 'enable' });
      setNotice({ ok: true, text: r.message });
      setPending(null); setNewProfile('');
      void refreshDashboard();
      setTimeout(load, 1500);
    } catch (err) { setNotice({ ok: false, text: (err as Error).message }); }
    finally { setBusy(false); }
  };
  const openProfiles = () => {
    if (profiles) return;
    get<{ profiles: string[] }>('/api/profiles').then(r => setProfiles(r.profiles)).catch(e => setNotice({ ok: false, text: e.message }));
  };
  const askAgent = (text: string) => window.dispatchEvent(new CustomEvent('agent:prefill', { detail: text }));

  return frame(
    <div class="detail">
      <div class="block-head">
        <h2 class="block-title">{c.name}</h2>
        <button type="button" class="ghost" onClick={close} disabled={busy}>Tutup</button>
      </div>
      <dl class="facts">
        <dt>Status</dt><dd>{c.disabled ? 'nonaktif' : c.online ? `online, ${c.address ?? ''} (${c.uptime ?? ''})` : 'offline'}</dd>
        <dt>Profil</dt><dd>{c.profile ?? '—'}</dd>
        <dt>Batas kuota</dt><dd>{gib(c.limit)}</dd>
        <dt>Terpakai</dt><dd>{gib(c.used)}{c.cycle_start ? ` sejak ${c.cycle_start}` : ''}</dd>
        <dt>Sisa</dt><dd>{c.approximate ? 'perkiraan (sesi melintasi awal siklus)' : gib(c.remaining)}</dd>
      </dl>

      <h3 class="sub-title">Tindakan</h3>
      {pending ? (
        <div class="confirm">
          <p>{pending.text}</p>
          <div class="row"><button type="button" class="danger" onClick={confirm} disabled={busy}>{busy ? 'Memproses…' : 'Ya, lanjutkan'}</button><button type="button" class="ghost" onClick={() => setPending(null)} disabled={busy}>Batal</button></div>
        </div>
      ) : (
        <div class="row">
          <button type="button" class="ghost" disabled={busy || !c.online} title={c.online ? undefined : 'Pelanggan sedang offline'} onClick={() => ask({ kind: 'disconnect', text: `Putus sesi ${c.name} sekarang? Pelanggan bisa langsung tersambung lagi.` })}>Putus sesi</button>
          {c.disabled
            ? <button type="button" class="ghost" disabled={busy} onClick={() => ask({ kind: 'enable', text: `Aktifkan lagi ${c.name}?` })}>Aktifkan</button>
            : <button type="button" class="ghost" disabled={busy} onClick={() => ask({ kind: 'disable', text: `Nonaktifkan ${c.name}? Sesinya ikut diputus dan ia tidak bisa tersambung sampai diaktifkan lagi.` })}>Nonaktifkan</button>}
          <select aria-label="Ganti profil" value={newProfile} onFocus={openProfiles} onPointerDown={openProfiles}
            onChange={e => { const v = (e.target as HTMLSelectElement).value; setNewProfile(v); if (v) ask({ kind: 'profile', text: `Ganti profil ${c.name} dari ${c.profile ?? 'tanpa profil'} ke ${v}? Berlaku saat pelanggan tersambung ulang.` }); }}>
            <option value="">{profiles ? 'Ganti profil…' : 'Ganti profil… (memuat)'}</option>
            {(profiles ?? []).filter(p => p !== c.profile).map(p => <option key={p} value={p}>{p}</option>)}
          </select>
        </div>
      )}

      <h3 class="sub-title">Tambah kuota</h3>
      {!c.editable ? (
        <div class="muted small stack">
          <p>{whyNotEditable(c)}</p>
          <p><button type="button" class="link-btn" onClick={() => askAgent(`Buatkan batas kuota khusus untuk pelanggan ${c.name} supaya bisa di-top-up dari panel.`)}>Minta agen membuatkan batas kuota khusus</button></p>
        </div>
      ) : op ? (
        <div class="confirm">
          <p>Kuota <strong>{op.account}</strong> berubah dari <strong>{gib(op.before)}</strong> menjadi <strong>{gib(op.after)}</strong>.</p>
          <div class="row"><button type="button" onClick={apply} disabled={busy}>{busy ? 'Menerapkan…' : 'Terapkan'}</button><button type="button" class="ghost" onClick={() => setOp(null)} disabled={busy}>Batal</button></div>
        </div>
      ) : (
        <div class="stack">
          <div class="row" role="group" aria-label="Tambah cepat">
            {QUICK_GIB.map(n => <button key={n} type="button" class="ghost" disabled={busy} onClick={() => void preview(n)}>+{n} GiB</button>)}
          </div>
          <form class="row" onSubmit={e => { e.preventDefault(); void preview(Number(amount)); }}>
            <label>Jumlah lain <input type="number" min={1} max={1000} value={amount} onInput={e => setAmount((e.target as HTMLInputElement).value)} required /> GiB</label>
            <button type="submit" disabled={busy}>{busy ? 'Memeriksa…' : 'Pratinjau'}</button>
          </form>
        </div>
      )}
      {notice && <p class={notice.ok ? 'notice' : 'alert'} role="status">{notice.text}</p>}

      <h3 class="sub-title">Trafik 30 hari</h3>
      <TrafficBars history={data.history} />
      <h3 class="sub-title">Log PPPoE</h3>
      {logs ? (logs.length ? <ol class="log-list">{logs.map((l, i) => <li key={i}><time>{l.time}</time> {l.message}</li>)}</ol> : <p class="muted">Tidak ada log untuk akun ini.</p>)
        : <button type="button" class="ghost" onClick={() => get<{ logs: Record<string, string>[] }>(url('logs')).then(r => setLogs(r.logs)).catch(e => setNotice({ ok: false, text: e.message }))}>Muat log</button>}
    </div>
  );
}

type AuditRow = { id: string; at: number; actor: string; account: string | null; action: string; before: string | null; after: string | null; status: string };

const ACTION_LABELS: Record<string, string> = {
  'quota.increase': 'Top-up kuota', 'customer.disconnect': 'Putus sesi', 'customer.enabled': 'Aktif/nonaktif pelanggan',
  'customer.profile': 'Ganti profil', 'recommendation.accept': 'Terapkan saran', 'action.revert': 'Kembalikan tombol aksi',
  'agent.topup_quota': 'Top-up kuota (agen)', 'agent.disconnect_session': 'Putus sesi (agen)', 'agent.set_customer_enabled': 'Aktif/nonaktif (agen)',
  'agent.set_customer_profile': 'Ganti profil (agen)', 'agent.set_radius_user': 'Atribut RADIUS (agen)', 'agent.reboot_router': 'Reboot router (agen)',
  'agent.create_action_button': 'Buat tombol aksi (agen)', 'agent.update_action_button': 'Ubah tombol aksi (agen)',
  'agent.delete_action_button': 'Hapus tombol aksi (agen)', 'agent.run_action_button': 'Jalankan tombol (agen)',
  'agent.revert_action_run': 'Kembalikan tombol (agen)', 'agent.install_update': 'Pasang update (agen)', 'agent.revert_service': 'Kembalikan IP Services',
  'agent.run_command': 'Perintah router (agen)',
};
const STATUS_LABELS: Record<string, string> = { done: 'berhasil', failed: 'gagal', partial: 'gagal sebagian', Terverifikasi: 'berhasil' };
const ACTOR_LABELS: Record<string, string> = { admin: 'Admin', agent: 'Agen AI' };

function actionLabel(a: string): string {
  if (ACTION_LABELS[a]) return ACTION_LABELS[a];
  if (a.startsWith('action.')) return `Tombol: ${a.slice(7)}`;
  if (a.startsWith('agent.')) return `${a.slice(6).replace(/_/g, ' ')} (agen)`;
  return a;
}

function Audit() {
  const [rows, setRows] = useState<AuditRow[] | null>(null);
  const [account, setAccount] = useState('');
  const [more, setMore] = useState(true);
  const fetchRows = (before?: number) => {
    const q = new URLSearchParams();
    if (account.trim()) q.set('account', account.trim());
    if (before) q.set('before', String(before));
    return get<{ audit: AuditRow[] }>(`/api/audit?${q}`).then(r => { setMore(r.audit.length === 100); return r.audit; });
  };
  useEffect(() => {
    const t = setTimeout(() => { fetchRows().then(setRows).catch(() => setRows([])); }, 250);
    return () => clearTimeout(t);
  }, [account]);
  const show = (r: AuditRow, v: string | null) => v && r.action === 'quota.increase' ? gib(Number(v)) : (v ?? '');
  const change = (r: AuditRow) => {
    const b = show(r, r.before), a = show(r, r.after);
    return b && a ? `${b} → ${a}` : a || b || '—';
  };
  return (
    <div>
      <div class="block-head">
        <h2 class="block-title">Riwayat perubahan</h2>
        <input class="search" type="search" placeholder="Saring nama pelanggan" value={account} onInput={e => setAccount((e.target as HTMLInputElement).value)} aria-label="Saring riwayat per pelanggan" />
      </div>
      {!rows ? <p class="muted">Memuat riwayat…</p> : rows.length === 0 ? <p class="muted">Belum ada perubahan{account.trim() ? ` untuk ${account.trim()}` : ''}.</p> : (
        <>
          <div class="table-wrap"><table>
            <thead><tr><th>Waktu</th><th>Oleh</th><th>Tindakan</th><th>Pelanggan</th><th>Perubahan</th><th>Hasil</th></tr></thead>
            <tbody>{rows.map(r => <tr key={r.id}><td>{dateTime(r.at)}</td><td>{ACTOR_LABELS[r.actor] ?? r.actor}</td><td>{actionLabel(r.action)}</td><td>{r.account ?? '—'}</td><td class="audit-change">{change(r)}</td><td>{STATUS_LABELS[r.status] ?? r.status}</td></tr>)}</tbody>
          </table></div>
          {more && <p><button type="button" class="ghost" onClick={() => fetchRows(rows[rows.length - 1].at).then(next => setRows([...rows, ...next])).catch(() => setMore(false))}>Muat lebih banyak</button></p>}
        </>
      )}
    </div>
  );
}

function AiSettings() {
  const [form, setForm] = useState({ url: '', protocol: 'anthropic', model: '', api_key: '', thinking: 'high' });
  const [hasKey, setHasKey] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [models, setModels] = useState<{ id: string; name: string }[] | null>(null);
  const [fetching, setFetching] = useState(false);
  const fetchModels = async () => {
    setFetching(true); setNotice(null);
    try {
      const r = await post<{ models: { id: string; name: string }[] }>('/api/settings/ai/models', { url: form.url, protocol: form.protocol, api_key: form.api_key || null });
      setModels(r.models);
      setNotice(`${r.models.length} model ditemukan. Pilih salah satu, lalu Simpan.`);
    } catch (err) { setModels(null); setNotice((err as Error).message); }
    finally { setFetching(false); }
  };
  useEffect(() => {
    get<{ url: string | null; protocol: string | null; model: string | null; has_key: boolean; thinking?: string }>('/api/settings/ai').then(s => {
      setForm(f => ({ ...f, url: s.url ?? 'https://api.anthropic.com/v1', protocol: s.protocol ?? 'anthropic', model: s.model ?? '', thinking: s.thinking ?? 'high' }));
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
      <label>API key<input type="password" autocomplete="off" value={form.api_key} onInput={set('api_key')} placeholder={hasKey ? 'Tersimpan; kosongkan untuk tetap memakai key lama' : ''} /></label>
      <label>Model<span class="row">
        <input style={{ flex: 1, minWidth: 0 }} value={form.model} onInput={set('model')} required maxLength={150} placeholder="Ketik, atau tekan Ambil model" />
        <button type="button" class="ghost" onClick={() => void fetchModels()} disabled={fetching || !form.url}>{fetching ? 'Mengambil…' : 'Ambil model'}</button>
      </span></label>
      {models && models.length > 0 && <label>Model dari endpoint<select value={models.some(m => m.id === form.model) ? form.model : ''} onChange={e => setForm({ ...form, model: (e.target as HTMLSelectElement).value })}>
        <option value="" disabled>Pilih model…</option>
        {models.map(m => <option key={m.id} value={m.id}>{m.name === m.id ? m.id : `${m.name} (${m.id})`}</option>)}
      </select></label>}
      <label>Kedalaman berpikir<select value={form.thinking} onChange={set('thinking')}>
        <option value="high">Tinggi (bawaan)</option><option value="max">Maksimal</option><option value="medium">Sedang</option><option value="low">Rendah</option><option value="off">Mati</option>
      </select><span class="muted">Lebih tinggi = jawaban lebih matang, tapi lebih lambat dan lebih banyak token. Model yang tidak mendukung otomatis dipakai tanpa mode ini.</span></label>
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
  useEffect(() => {
    const load = () => get<{ actions: Action[] }>('/api/actions').then(r => setRows(r.actions)).catch(() => setRows([]));
    void load();
    return on('actions.updated', () => void load());
  }, []);
  if (!rows) return <p class="muted">Memuat tombol aksi…</p>;
  return (
    <div>
      <h2 class="block-title">Tombol aksi</h2>
      {rows.length === 0 ? <p class="muted">Belum ada tombol aksi. Minta agen membuatnya, misalnya “buat tombol putus sesi PPPoE pelanggan”.</p> : (
        <ul class="plain-list action-list">{rows.map(a => <li key={a.id}><ActionButton block={{ id: a.id, span: 1, kind: 'action_button', props: { action_id: a.id, label: a.label } } as never} /> <span class={`badge ${a.risk === 'critical' ? 'off' : 'idle'}`}>{({ read: 'baca', write: 'ubah', critical: 'kritis' } as Record<string, string>)[a.risk] ?? a.risk}</span></li>)}</ul>
      )}
    </div>
  );
}
