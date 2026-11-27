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

let retry: ReturnType<typeof setTimeout> | undefined;
let downSince = 0;

/** Dipanggil saat kanal realtime putus (mis. panel restart karena update). */
let onDown: (() => Promise<boolean>) | null = null;

/** `check` mengembalikan false bila sesi login sudah tidak berlaku (berhenti menyambung ulang). */
export function onConnectionLost(check: () => Promise<boolean>) {
  onDown = check;
}

export const connection = { down: false };

export function connect() {
  if (source) return;
  source = new EventSource('/api/events');
  for (const kind of handlers.keys()) attach(kind);
  source.onopen = () => {
    if (wasDown) dispatch('resync', {});
    wasDown = false;
    downSince = 0;
    setDown(false);
  };
  source.onerror = () => {
    wasDown = true;
    downSince ||= Date.now();
    if (Date.now() - downSince > 4000) setDown(true);
    // Browser berhenti menyambung ulang bila server menjawab selain 200 (mis. 401 setelah
    // restart). Tanpa ini angka di layar membeku diam-diam; jadi cek sesi dan sambung lagi sendiri.
    if (source?.readyState === EventSource.CLOSED) {
      source = null;
      clearTimeout(retry);
      retry = setTimeout(async () => {
        const ok = onDown ? await onDown().catch(() => true) : true;
        if (ok) connect(); else setDown(false);
      }, 3000);
    }
  };
}

function setDown(v: boolean) {
  if (connection.down === v) return;
  connection.down = v;
  dispatch('connection', { down: v });
}

export function disconnect() {
  clearTimeout(retry);
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
