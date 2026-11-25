import { render } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import './app.css';
import './agent/design.css';
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
        <h1>Panel MikroTik</h1>
        <label>Kata sandi admin<input type="password" autocomplete="current-password" value={password} onInput={e => setPassword((e.target as HTMLInputElement).value)} required autofocus /></label>
        <button type="submit" disabled={busy}>{busy ? 'Memeriksa…' : 'Masuk'}</button>
        {error && <p class="alert" role="alert">{error}</p>}
      </form>
    </main>
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
  return (
    <div class="panel-root" ref={root}>
      <div class="shell">
        <nav class="nav" aria-label="Menu utama">
          <a class="brand" href="/">Panel MikroTik</a>
          <ul>
            {spec?.nav.items.map(item => (
              <li key={item.page}><a href={`/${item.page}`} aria-current={page?.id === item.page ? 'page' : undefined}><span class="nav-icon" aria-hidden="true">{item.icon}</span>{item.label}</a></li>
            ))}
          </ul>
          <div class="nav-foot">
            <button type="button" class="ghost small" onClick={refresh} disabled={refreshing}>{refreshing ? 'Memperbarui…' : 'Perbarui data'}</button>
            <span class="muted small">{routerStatus.value.ok ? `Sinkron ${ago(dashboard.value?.sampled_at)}` : 'Router tidak terjangkau'}</span>
            <button type="button" class="ghost small" onClick={() => void logout()}>Keluar</button>
          </div>
        </nav>
        <main class="content">
          {emergency && <p class="alert">Mode darurat: desain bawaan ditampilkan dan agen dimatikan. <a href="/" onClick={e => { e.preventDefault(); location.assign('/'); }}>Kembali ke desain biasa</a></p>}
          {page ? <DesignPage page={page} renderers={renderers} /> : <p class="muted design-page">Memuat tampilan…</p>}
        </main>
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
