import { render } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import './agent/design.css';
import './app.css';
import { DesignPage, liveSpec, mountAgentPanel, startDesign, type BlockRenderers } from './agent';
import { applyDesign } from './agent/design-runtime';
import type { DesignSpec } from './agent/types';
import { authed, checkSession, get, login, logout, post } from './api';
import { ActionButton, CustomerTable, Logs, LowQuota, Metrics, Recommendations, RouterStatus, Topology, TrafficChart } from './blocks';
import { Builtin } from './builtins';
import { dashboard, refreshDashboard, routerStatus, startData } from './data';
import { connect } from './events';
import { ago } from './format';
import { navigate, path } from './router';

const renderers: BlockRenderers = {
  metrics: Metrics, traffic_chart: TrafficChart, customer_table: CustomerTable, low_quota: LowQuota,
  router_status: RouterStatus, topology: Topology, logs: Logs, recommendations: Recommendations,
  action_button: ActionButton, builtin: Builtin,
} as BlockRenderers;

/** ?design=default: mode darurat, tampilkan desain bawaan tanpa mengubah apa pun di server. */
const emergency = new URLSearchParams(location.search).get('design') === 'default';

/** Menu bawah ala Instagram untuk HP (permintaan Usep 2026-11-26): 3 halaman utama, tombol
 *  agen di tengah, dan "Lainnya" berisi sisa menu serta Keluar. Di layar lebar disembunyikan CSS. */
function TabBar({ items, current }: { items: DesignSpec['nav']['items']; current?: string }) {
  const [more, setMore] = useState(false);
  const pages = items.filter(i => i.page !== 'agen');
  const main = pages.slice(0, 3);
  const rest = pages.slice(3);
  const restActive = rest.some(i => i.page === current);
  const link = (item: DesignSpec['nav']['items'][number]) => (
    <a key={item.page} href={`/${item.page}`} class="tab" aria-current={current === item.page ? 'page' : undefined}>
      <span class="tab-icon" aria-hidden="true">{item.icon}</span><span class="tab-label">{item.label}</span>
    </a>
  );
  return (
    <>
      <nav class="tabbar" aria-label="Menu bawah">
        {main.slice(0, 2).map(link)}
        <button type="button" class="tab tab-agent" aria-label="Buka agen AI" onClick={() => window.dispatchEvent(new Event('agent:toggle'))}>
          <span class="tab-agent-mark" aria-hidden="true">✦</span>
        </button>
        {main.slice(2).map(link)}
        <button type="button" class="tab" aria-expanded={more} aria-current={restActive ? 'page' : undefined} onClick={() => setMore(m => !m)}>
          <span class="tab-icon" aria-hidden="true">☰</span><span class="tab-label">Lainnya</span>
        </button>
      </nav>
      {more && (
        <div class="sheet-backdrop" onClick={() => setMore(false)}>
          <div class="sheet" role="dialog" aria-label="Menu lainnya" onClick={e => e.stopPropagation()}>
            <span class="sheet-grip" aria-hidden="true" />
            <ul>
              {rest.map(item => <li key={item.page}><a href={`/${item.page}`} onClick={() => setMore(false)} aria-current={current === item.page ? 'page' : undefined}><span class="tab-icon" aria-hidden="true">{item.icon}</span>{item.label}</a></li>)}
              <li><button type="button" onClick={() => void logout()}><span class="tab-icon" aria-hidden="true">⏻</span>Keluar</button></li>
            </ul>
          </div>
        </div>
      )}
    </>
  );
}

function Login() {
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async (e: Event) => {
    e.preventDefault();
    setBusy(true); setError(null);
    try { await login(password); } catch (err) { setError((err as Error).message); } finally { setBusy(false); }
  };
  return (
    <main class="login">
      <form class="login-card stack" onSubmit={submit}>
        <div class="login-brand"><span class="brand-mark" aria-hidden="true">M</span><h1>Panel MikroTik</h1></div>
        <label>Kata sandi admin<input type="password" autocomplete="current-password" value={password} onInput={e => setPassword((e.target as HTMLInputElement).value)} required autofocus /></label>
        <button type="submit" disabled={busy}>{busy ? 'Memeriksa…' : 'Masuk'}</button>
        {error && <p class="alert" role="alert">{error}</p>}
      </form>
    </main>
  );
}

type UpdateStatus = { installed: string; latest: string; checked_at: string; check: string; last_tag: string; last_at: string; last_result: string; auto: boolean };

function when(iso: string): string {
  const d = new Date(iso);
  return isNaN(d.getTime()) ? '—' : d.toLocaleString('id-ID', { dateStyle: 'medium', timeStyle: 'short' });
}

/** Versi panel di bilah atas; tooltip berisi status pembaru otomatis. */
function VersionPill() {
  const [info, setInfo] = useState<{ version: string; update: UpdateStatus | null } | null>(null);
  useEffect(() => {
    const load = () => get<{ version: string; update: UpdateStatus | null }>('/api/version').then(setInfo).catch(() => {});
    load();
    const t = setInterval(load, 10 * 60 * 1000);
    return () => clearInterval(t);
  }, []);
  if (!info) return null;
  const u = info.update;
  const latest = u?.latest?.replace(/^v/, '');
  const behind = !!latest && latest !== info.version;
  const failed = u?.last_result === 'rollback' && u.last_tag.replace(/^v/, '') !== info.version;
  const tone = failed ? 'bad' : behind ? 'warn' : '';
  const lines = [`Panel v${info.version}`];
  if (!u) lines.push('Pembaru otomatis belum terpasang di server ini.');
  else {
    lines.push(u.auto ? 'Update otomatis: aktif (cek tiap jam)' : 'Update otomatis: mati');
    lines.push(`Rilis terbaru di GitHub: ${u.latest || 'belum terbaca'}`);
    lines.push(`Dicek: ${when(u.checked_at)}${u.check === 'gagal' ? ' (gagal menghubungi GitHub)' : ''}`);
    if (u.last_tag) lines.push(`Update terakhir: ${u.last_tag} ${u.last_result === 'ok' ? 'berhasil' : 'gagal, dikembalikan'} (${when(u.last_at)})`);
    if (behind && !failed) lines.push(`Versi ${u.latest} akan dipasang otomatis pada pengecekan berikutnya.`);
  }
  return (
    <span class={`pill version ${tone}`} title={lines.join('\n')} aria-label={lines.join('. ')}>
      v{info.version}{behind ? <b> → {u!.latest}</b> : null}
    </span>
  );
}

function Shell() {
  const root = useRef<HTMLDivElement>(null);
  const [refreshing, setRefreshing] = useState(false);
  useEffect(() => {
    connect();
    startData();
    if (emergency) {
      get<{ spec: DesignSpec }>('/api/design/default').then(({ spec }) => { liveSpec.value = spec; applyDesign(spec, root.current!); });
    } else {
      startDesign(root.current!);
      const unmount = mountAgentPanel();
      return unmount;
    }
    return undefined;
  }, []);
  const spec = liveSpec.value;
  const pages = spec?.pages ?? [];
  const id = decodeURIComponent(path.value.replace(/^\/+/, '').split('/')[0] ?? '');
  const page = pages.find(p => p.id === id) ?? pages.find(p => p.id === spec?.nav.items[0]?.page) ?? pages[0];
  const refresh = async () => {
    setRefreshing(true);
    try { await post('/api/router/refresh'); await refreshDashboard(); } catch (e) { routerStatus.value = { ok: false, error: (e as Error).message }; } finally { setRefreshing(false); }
  };
  useEffect(() => { document.title = page ? `${page.title} · Panel MikroTik` : 'Panel MikroTik'; }, [page?.title]);
  const r = dashboard.value?.resources ?? {};
  const total = Number(r['total-memory']), free = Number(r['free-memory']);
  const ram = total ? `${Math.round(100 * (1 - free / total))}%` : '—';
  const ok = routerStatus.value.ok;
  return (
    <div class="panel-root" ref={root}>
      <div class="shell">
        <nav class="nav" aria-label="Menu utama">
          <a class="brand" href="/" title="Panel MikroTik"><span class="brand-mark" aria-hidden="true">M</span><span class="brand-text">Panel MikroTik</span></a>
          <ul>
            {spec?.nav.items.map(item => (
              <li key={item.page}><a href={`/${item.page}`} title={item.label} aria-current={page?.id === item.page ? 'page' : undefined}><span class="nav-icon" aria-hidden="true">{item.icon}</span><span class="nav-label">{item.label}</span></a></li>
            ))}
          </ul>
          <div class="nav-foot">
            <button type="button" class="icon-btn" title="Keluar" aria-label="Keluar" onClick={() => void logout()}>⏻</button>
          </div>
        </nav>
        <div class="main-col">
          <header class="topbar">
            <h1 class="topbar-title">{page?.title ?? 'Panel MikroTik'}</h1>
            <span class="crumb">{r['board-name'] ? `/ ${r['board-name']}${r.version ? ` · RouterOS ${String(r.version).split(' ')[0]}` : ''}` : ''}</span>
            <span class="spacer" />
            <span class={`pill ${ok ? 'ok' : 'bad'}`} title={ok ? undefined : routerStatus.value.error}>
              router <b>● {ok ? 'terhubung' : 'terputus'}</b> {ok ? ago(dashboard.value?.sampled_at) : ''}
            </span>
            {r['cpu-load'] != null && <span class="pill hide-sm">CPU {r['cpu-load']}% · RAM {ram}</span>}
            <VersionPill />
            <button type="button" class="pill-btn" onClick={refresh} disabled={refreshing} title="Ambil data terbaru dari router">{refreshing ? '…' : '↻'} <span class="hide-sm">Perbarui</span></button>
          </header>
          <main class="content">
            {emergency && <p class="alert">Mode darurat: desain bawaan ditampilkan dan agen dimatikan. <a href="/" onClick={e => { e.preventDefault(); location.assign('/'); }}>Kembali ke desain biasa</a></p>}
            {page ? <DesignPage page={page} renderers={renderers} /> : <p class="muted design-page">Memuat tampilan…</p>}
          </main>
        </div>
        <TabBar items={spec?.nav.items ?? []} current={page?.id} />
      </div>
    </div>
  );
}

function App() {
  if (authed.value === null) return <p class="boot">Memuat…</p>;
  return authed.value ? <Shell /> : <Login />;
}

void checkSession();
render(<App />, document.getElementById('app')!);
void navigate;
