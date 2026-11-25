// Router kecil berbasis History API: /<id-halaman> dan query string.
import { signal } from '@preact/signals';

export const path = signal(location.pathname);
export const query = signal(new URLSearchParams(location.search));

export function navigate(to: string) {
  if (to === location.pathname + location.search) return;
  history.pushState(null, '', to);
  sync();
  window.scrollTo(0, 0);
}

function sync() {
  path.value = location.pathname;
  query.value = new URLSearchParams(location.search);
}

addEventListener('popstate', sync);

/** Tangkap klik pada <a href="/..."> internal supaya tidak memuat ulang halaman. */
document.addEventListener('click', ev => {
  const a = (ev.target as Element | null)?.closest?.('a');
  if (!a || a.target || a.hasAttribute('download') || ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.button !== 0) return;
  const url = new URL(a.href, location.href);
  if (url.origin !== location.origin || url.pathname.startsWith('/api/')) return;
  ev.preventDefault();
  navigate(url.pathname + url.search);
});
