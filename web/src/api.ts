// Klien API: cookie sesi + token CSRF di header untuk semua permintaan selain GET.
import { signal } from '@preact/signals';

/** null = belum dicek, false = harus login. */
export const authed = signal<boolean | null>(null);
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
  const data = text ? JSON.parse(text) : {};
  if (res.status === 401 && path !== '/api/login') authed.value = false;
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

export async function login(password: string) {
  csrf = (await post<{ csrf: string }>('/api/login', { password })).csrf;
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
