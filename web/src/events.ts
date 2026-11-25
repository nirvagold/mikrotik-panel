// Satu EventSource untuk semua update realtime. `resync` dikirim server bila klien tertinggal,
// dan juga dipancarkan lokal setelah koneksi pulih.
type Handler = (data: any) => void;

const handlers = new Map<string, Set<Handler>>();
let source: EventSource | null = null;
let wasDown = false;

function dispatch(kind: string, data: unknown) {
  handlers.get(kind)?.forEach(h => {
    try { h(data); } catch (e) { console.error(e); }
  });
}

function attach(kind: string) {
  source?.addEventListener(kind, ev => {
    const text = (ev as MessageEvent).data;
    dispatch(kind, text ? JSON.parse(text) : {});
  });
}

export function connect() {
  if (source) return;
  source = new EventSource('/api/events');
  for (const kind of handlers.keys()) attach(kind);
  source.onopen = () => {
    if (wasDown) dispatch('resync', {});
    wasDown = false;
  };
  source.onerror = () => { wasDown = true; };
}

export function disconnect() {
  source?.close();
  source = null;
}

export function on(kind: string, handler: Handler): () => void {
  let set = handlers.get(kind);
  if (!set) {
    set = new Set();
    handlers.set(kind, set);
    attach(kind);
  }
  set.add(handler);
  return () => set!.delete(handler);
}
