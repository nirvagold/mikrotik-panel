// Klien API: cookie sesi + token CSRF di header untuk semua permintaan selain GET.
import { signal } from '@preact/signals';

/** null = belum dicek, false = harus login. */
export const authed = signal<boolean | null>(null);
/** true bila admin dikeluarkan karena sesinya berakhir (bukan menekan Keluar). */
export const sessionEnded = signal(false);
let csrf = '';

export class ApiError extends Error {
  constructor(message: string, readonly status: number, readonly data: Record<string, unknown> = {}) {
    super(message);
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers['content-type'] = 'application/json';
  if (method !== 'GET') headers['x-csrf-token'] = csrf;
  let res: Response;
  try {
    res = await fetch(path, { method, headers, credentials: 'same-origin', body: body === undefined ? undefined : JSON.stringify(body) });
  } catch {
    throw new ApiError('Panel tidak bisa dihubungi. Periksa koneksi.', 0);
  }
  const text = await res.text();
  let data: Record<string, any> = {};
  try { data = text ? JSON.parse(text) : {}; } catch {
    // Bukan JSON: biasanya proxy/server sedang dinyalakan ulang.
    throw new ApiError(res.status >= 500 || res.status === 0 ? 'Panel sedang dinyalakan ulang. Coba lagi sebentar lagi.' : `Jawaban panel tidak terbaca (${res.status}).`, res.status);
  }
  if (res.status === 401 && path !== '/api/login') {
    if (authed.value) sessionEnded.value = true;
    authed.value = false;
  }
  if (!res.ok) throw new ApiError(data.error ?? `Permintaan gagal (${res.status}).`, res.status, data);
  return data as T;
}

export const get = <T>(path: string) => request<T>('GET', path);
export const post = <T>(path: string, body?: unknown) => request<T>('POST', path, body ?? {});
export const put = <T>(path: string, body?: unknown) => request<T>('PUT', path, body ?? {});

export async function checkSession() {
  try {
    csrf = (await get<{ csrf: string }>('/api/session')).csrf;
    authed.value = true;
  } catch {
    authed.value = false;
  }
}

/** Untuk kanal realtime yang putus: false hanya bila server jelas menolak sesi (401). */
export async function sessionStillValid(): Promise<boolean> {
  try {
    csrf = (await get<{ csrf: string }>('/api/session')).csrf;
    return true;
  } catch (e) {
    return !(e instanceof ApiError && e.status === 401);
  }
}

export async function login(password: string) {
  csrf = (await post<{ csrf: string }>('/api/login', { password })).csrf;
  // Masuk lagi setelah sesi berakhir: muat ulang halaman yang sama supaya semua data dan
  // kanal realtime mulai bersih (alamat halaman tetap, draft chat tersimpan di browser).
  if (sessionEnded.value) { location.reload(); return; }
  authed.value = true;
}

export async function logout() {
  try {
    await post('/api/logout');
  } finally {
    authed.value = false;
    location.assign('/');
  }
}
