// Kartu data dari alat `show_card` (tabel, grafik, topologi). Input datang dari model, jadi
// semua bentuk dibaca longgar dan apa pun yang tidak dikenali jatuh ke tampilan JSON.
// Bentuk yang dikenali:
//   table:    { columns?: string[], rows: (array | object)[] }
//   chart:    { type?: "bar" | "line", labels: string[], series: [{ name, values: number[] }] }
//             atau { points: [{ label, value }] } atau { values: { label: number } }
//   topology: { nodes: [{ id, label?, type?, status? }], edges: [{ from, to, label? }] }
//             atau keluaran get_topology apa adanya (router, interfaces, bridges, vlans, neighbors)

type Json = unknown;
type Obj = Record<string, Json>;
const isObj = (v: Json): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);
const str = (v: Json): string => v === null || v === undefined ? '' : typeof v === 'object' ? JSON.stringify(v) : String(v);
const num = (v: Json): number => { const n = typeof v === 'number' ? v : Number(v); return Number.isFinite(n) ? n : 0; };
const COLORS = ['var(--accent, #f4f4f5)', '#60a5fa', '#34d399', '#f59e0b', '#f472b6', '#a78bfa'];
const MAX_ROWS = 200;

export function ShowCard({ input }: { input: Json }) {
  const o = isObj(input) ? input : {};
  const kind = str(o.kind);
  const data = o.data;
  const title = str(o.title);
  let body;
  try {
    body = kind === 'table' ? <DataTable data={data} />
      : kind === 'chart' ? <Chart data={data} />
      : kind === 'topology' ? <Topology data={data} />
      : null;
  } catch { body = null; }
  return (
    <figure class="datacard">
      {title && <figcaption>{title}</figcaption>}
      {body ?? <pre class="raw">{JSON.stringify(data, null, 2)}</pre>}
    </figure>
  );
}

function DataTable({ data }: { data: Json }) {
  const d = isObj(data) ? data : { rows: data };
  const rows = Array.isArray(d.rows) ? d.rows : [];
  if (!rows.length) return <p class="muted">Tidak ada baris.</p>;
  let columns = Array.isArray(d.columns) ? d.columns.map(str) : [];
  if (!columns.length) {
    if (isObj(rows[0])) columns = [...new Set(rows.flatMap(r => isObj(r) ? Object.keys(r) : []))];
    else columns = (Array.isArray(rows[0]) ? rows[0] : [rows[0]]).map((_, i) => `#${i + 1}`);
  }
  const cell = (r: Json, c: string, i: number) => isObj(r) ? str(r[c]) : Array.isArray(r) ? str(r[i]) : i === 0 ? str(r) : '';
  return (
    <div class="tablewrap">
      <table>
        <thead><tr>{columns.map(c => <th key={c} scope="col">{c}</th>)}</tr></thead>
        <tbody>{rows.slice(0, MAX_ROWS).map((r, ri) => <tr key={ri}>{columns.map((c, i) => <td key={c}>{cell(r, c, i)}</td>)}</tr>)}</tbody>
      </table>
      {rows.length > MAX_ROWS && <p class="muted">{rows.length - MAX_ROWS} baris lain tidak ditampilkan.</p>}
    </div>
  );
}

type Series = { name: string; values: number[] };

function chartData(data: Json): { type: string; labels: string[]; series: Series[]; unit: string } {
  const d = isObj(data) ? data : {};
  const unit = str(d.unit);
  const type = str(d.type) === 'line' ? 'line' : 'bar';
  if (Array.isArray(d.points)) {
    const pts = d.points.filter(isObj);
    return { type, unit, labels: pts.map(p => str(p.label ?? p.x)), series: [{ name: str(d.name), values: pts.map(p => num(p.value ?? p.y)) }] };
  }
  if (isObj(d.values)) {
    const entries = Object.entries(d.values);
    return { type, unit, labels: entries.map(e => e[0]), series: [{ name: '', values: entries.map(e => num(e[1])) }] };
  }
  const labels = Array.isArray(d.labels) ? d.labels.map(str) : [];
  let series: Series[] = [];
  if (Array.isArray(d.series)) series = d.series.filter(isObj).map(s => ({ name: str(s.name), values: Array.isArray(s.values) ? s.values.map(num) : [] }));
  else if (Array.isArray(d.values)) series = [{ name: '', values: d.values.map(num) }];
  return { type, unit, labels, series };
}

const fmt = (n: number) => Math.abs(n) >= 1000 ? n.toLocaleString('id-ID', { maximumFractionDigits: 0 }) : n.toLocaleString('id-ID', { maximumFractionDigits: 2 });

function Chart({ data }: { data: Json }) {
  const { type, labels, series, unit } = chartData(data);
  const count = Math.max(labels.length, ...series.map(s => s.values.length), 0);
  if (!series.length || !count) return null;
  const max = Math.max(...series.flatMap(s => s.values), 0) || 1;
  const legend = series.length > 1 && <ul class="legend">{series.map((s, i) => <li key={i}><i style={{ background: COLORS[i % COLORS.length] }} />{s.name || `Seri ${i + 1}`}</li>)}</ul>;
  const u = unit ? ` ${unit}` : '';

  if (type === 'line') {
    const W = 320, H = 140, pad = 4;
    const x = (i: number) => count === 1 ? W / 2 : pad + (i * (W - 2 * pad)) / (count - 1);
    const y = (v: number) => H - pad - (v / max) * (H - 2 * pad);
    return (
      <div class="chart">
        {legend}
        <div class="chart-max">{fmt(max)}{u}</div>
        <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={`Grafik garis, maksimum ${fmt(max)}${u}`} preserveAspectRatio="none">
          <line x1="0" y1={H - pad} x2={W} y2={H - pad} class="axis" />
          {series.map((s, si) => <polyline key={si} fill="none" stroke={COLORS[si % COLORS.length]} stroke-width="2" vector-effect="non-scaling-stroke"
            points={s.values.map((v, i) => `${x(i)},${y(v)}`).join(' ')} />)}
        </svg>
        <div class="chart-x"><span>{labels[0] ?? ''}</span><span>{labels[count - 1] ?? ''}</span></div>
      </div>
    );
  }

  // Batang horizontal: label tetap terbaca di popup yang sempit.
  return (
    <div class="chart">
      {legend}
      <ul class="bars">
        {Array.from({ length: Math.min(count, 60) }, (_, i) => (
          <li key={i}>
            <span class="bar-label" title={labels[i]}>{labels[i] ?? `#${i + 1}`}</span>
            <span class="bar-track">
              {series.map((s, si) => <span key={si} class="bar" title={`${s.name ? s.name + ': ' : ''}${fmt(s.values[i] ?? 0)}${u}`}
                style={{ width: `${Math.max(0, ((s.values[i] ?? 0) / max) * 100)}%`, background: COLORS[si % COLORS.length] }} />)}
            </span>
            <span class="bar-value">{series.map(s => fmt(s.values[i] ?? 0)).join(' / ')}{u}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

type TNode = { id: string; label: string; type: string; status: string };
type TEdge = { from: string; to: string; label: string };

/** Ubah keluaran get_topology menjadi simpul dan sisi. */
function fromRouter(d: Obj): { nodes: TNode[]; edges: TEdge[] } {
  const root = str(d.router) || 'Router';
  const nodes: TNode[] = [{ id: '#router', label: root, type: 'router', status: '' }];
  const edges: TEdge[] = [];
  const has = new Set<string>(['#router']);
  const add = (id: string, label: string, type: string, status = '') => { if (!has.has(id)) { has.add(id); nodes.push({ id, label, type, status }); } };
  const bridged = new Map<string, string>();
  if (isObj(d.bridges)) for (const [br, ports] of Object.entries(d.bridges)) {
    if (Array.isArray(ports)) for (const p of ports) if (isObj(p)) bridged.set(str(p.interface), br);
  }
  const vlanParent = new Map<string, string>();
  if (Array.isArray(d.vlans)) for (const v of d.vlans) if (isObj(v)) vlanParent.set(str(v.name), str(v.interface));
  const ifaces = Array.isArray(d.interfaces) ? d.interfaces.filter(isObj) : [];
  for (const i of ifaces) {
    const name = str(i.name);
    const status = i.disabled === true ? 'disabled' : i.running === false ? 'down' : 'up';
    const hosts = num(i.arp_hosts);
    add(`if:${name}`, hosts ? `${name} (${hosts} host)` : name, str(i.type) || 'interface', status);
  }
  for (const i of ifaces) {
    const name = str(i.name);
    const parent = bridged.get(name) ?? vlanParent.get(name);
    edges.push({ from: parent && has.has(`if:${parent}`) ? `if:${parent}` : '#router', to: `if:${name}`, label: '' });
  }
  if (Array.isArray(d.neighbors)) for (const n of d.neighbors) if (isObj(n)) {
    const id = `nb:${str(n['mac-address']) || str(n.address) || str(n.identity)}`;
    add(id, str(n.identity) || str(n.address) || 'Perangkat', 'neighbor', '');
    const via = `if:${str(n.interface).split(',')[0]}`;
    edges.push({ from: has.has(via) ? via : '#router', to: id, label: str(n.address) });
  }
  return { nodes, edges };
}

function graph(data: Json): { nodes: TNode[]; edges: TEdge[] } | null {
  if (!isObj(data)) return null;
  if (Array.isArray(data.nodes)) {
    const nodes = data.nodes.filter(isObj).map(n => ({ id: str(n.id ?? n.name), label: str(n.label ?? n.name ?? n.id), type: str(n.type), status: str(n.status) }));
    const edges = (Array.isArray(data.edges) ? data.edges : Array.isArray(data.links) ? data.links : []).filter(isObj)
      .map(e => ({ from: str(e.from ?? e.source), to: str(e.to ?? e.target), label: str(e.label) }));
    return { nodes, edges };
  }
  if (Array.isArray(data.interfaces) || 'router' in data) return fromRouter(data);
  return null;
}

function Topology({ data }: { data: Json }) {
  const g = graph(data);
  if (!g || !g.nodes.length) return null;
  const ids = new Set(g.nodes.map(n => n.id));
  const edges = g.edges.filter(e => ids.has(e.from) && ids.has(e.to) && e.from !== e.to);
  // Tingkat tiap simpul lewat BFS dari akar (router, atau simpul tanpa induk).
  const parents = new Map<string, string>();
  for (const e of edges) if (!parents.has(e.to)) parents.set(e.to, e.from);
  const roots = g.nodes.filter(n => n.type === 'router' || !parents.has(n.id)).map(n => n.id);
  const level = new Map<string, number>();
  const queue = roots.length ? [...roots] : [g.nodes[0].id];
  queue.forEach(r => level.set(r, 0));
  while (queue.length) {
    const cur = queue.shift()!;
    for (const e of edges) if (e.from === cur && !level.has(e.to)) { level.set(e.to, (level.get(cur) ?? 0) + 1); queue.push(e.to); }
  }
  for (const n of g.nodes) if (!level.has(n.id)) level.set(n.id, 0);
  const columns: TNode[][] = [];
  for (const n of g.nodes.slice(0, 150)) (columns[level.get(n.id)!] ??= []).push(n);
  const colW = 150, rowH = 34, boxW = 128, boxH = 24;
  const pos = new Map<string, { x: number; y: number }>();
  const tallest = Math.max(...columns.map(c => c?.length ?? 0));
  columns.forEach((col, ci) => col?.forEach((n, ri) => {
    const offset = ((tallest - col.length) * rowH) / 2;
    pos.set(n.id, { x: 4 + ci * colW, y: 4 + offset + ri * rowH });
  }));
  const W = Math.max(colW * columns.length, colW), H = tallest * rowH + 8;
  const clip = (s: string) => s.length > 18 ? s.slice(0, 17) + '…' : s;
  return (
    <div class="topo">
      <svg viewBox={`0 0 ${W} ${H}`} width="100%" style={{ maxWidth: `${W}px`, minWidth: `${Math.min(W, 300)}px` }} role="img" aria-label={`Topologi: ${g.nodes.length} perangkat/port, ${edges.length} sambungan`}>
        {edges.map((e, i) => {
          const a = pos.get(e.from), b = pos.get(e.to);
          if (!a || !b) return null;
          const x1 = a.x + boxW, y1 = a.y + boxH / 2, x2 = b.x, y2 = b.y + boxH / 2, mx = (x1 + x2) / 2;
          return <path key={i} class="edge" d={`M${x1},${y1} C${mx},${y1} ${mx},${y2} ${x2},${y2}`}><title>{e.label}</title></path>;
        })}
        {g.nodes.map(n => {
          const p = pos.get(n.id);
          if (!p) return null;
          return <g key={n.id} class={`node ${n.type === 'router' ? 'root' : ''} ${n.status}`} transform={`translate(${p.x},${p.y})`}>
            <title>{`${n.label}${n.type ? ` · ${n.type}` : ''}${n.status ? ` · ${n.status}` : ''}`}</title>
            <rect width={boxW} height={boxH} rx="5" />
            <text x="8" y={boxH / 2 + 4}>{clip(n.label)}</text>
          </g>;
        })}
      </svg>
    </div>
  );
}
