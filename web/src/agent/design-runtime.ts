// Menerapkan design spec ke halaman TANPA reload: token jadi CSS variables,
// CSS khusus lewat adoptedStyleSheets (tidak butuh 'unsafe-inline' di CSP).
import type { DesignSpec, Font, Palette, Tokens } from './types';

const FONTS: Record<Font, string> = {
  system: 'system-ui, -apple-system, "Segoe UI", Roboto, sans-serif',
  inter: 'Inter, system-ui, sans-serif',
  ibm_plex_sans: '"IBM Plex Sans", system-ui, sans-serif',
  source_sans: '"Source Sans 3", "Source Sans Pro", system-ui, sans-serif',
  serif: 'Georgia, "Times New Roman", serif',
  mono: 'ui-monospace, "JetBrains Mono", Menlo, Consolas, monospace',
};
const SHADOWS = { none: 'none', soft: '0 1px 2px rgb(0 0 0 / .08), 0 4px 14px rgb(0 0 0 / .08)', strong: '0 2px 4px rgb(0 0 0 / .18), 0 12px 32px rgb(0 0 0 / .22)' };

const tokenSheet = new CSSStyleSheet();
const customSheet = new CSSStyleSheet();
let installed = false;

function install() {
  if (installed) return;
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, tokenSheet, customSheet];
  installed = true;
}

const vars = (p: Palette) => Object.entries(p).map(([k, v]) => `--${k.replace('_', '-')}:${v};`).join('');

export function tokensCss(t: Tokens): string {
  const shared = `--font:${FONTS[t.font]};--font-mono:${FONTS[t.font_mono]};--font-size:${t.font_size}px;` +
    `--radius:${t.radius}px;--space:${(8 * t.density) / 100}px;--shadow:${SHADOWS[t.shadow]};`;
  // Token juga dipasang di host panel agen (di luar .panel-root) supaya panel ikut tema,
  // sementara CSS khusus agen (berawalan .panel-root) tetap tidak bisa menjangkaunya.
  const sel = '.panel-root,#agent-host';
  const light = `${sel}{${shared}${vars(t.light)}color-scheme:light}`;
  const dark = `${sel}{${vars(t.dark)}color-scheme:dark}`;
  if (t.mode === 'light') return light;
  if (t.mode === 'dark') return `${sel}{${shared}${vars(t.dark)}color-scheme:dark}`;
  return `${light}@media (prefers-color-scheme: dark){${dark}}`;
}

/** Terapkan spec. Mengembalikan false bila CSS khusus dimatikan karena membuat teks tak terbaca. */
export function applyDesign(spec: DesignSpec, root: HTMLElement): boolean {
  install();
  tokenSheet.replaceSync(tokensCss(spec.tokens));
  root.dataset.nav = spec.nav.position;
  try {
    customSheet.replaceSync(spec.custom_css);
  } catch {
    customSheet.replaceSync('');
    return false;
  }
  if (spec.custom_css && !readable(root)) {
    customSheet.replaceSync('');
    return false;
  }
  return true;
}

/** Bungkus perubahan DOM dengan View Transition bila browser mendukung. */
export function transition(update: () => void) {
  const doc = document as Document & { startViewTransition?: (cb: () => void) => unknown };
  if (doc.startViewTransition && !matchMedia('(prefers-reduced-motion: reduce)').matches) doc.startViewTransition(update);
  else update();
}

/** Sorot sebentar blok yang baru diubah agen. */
export function highlight(blockId: string | undefined) {
  if (!blockId) return;
  const el = document.querySelector<HTMLElement>(`[data-block-id="${CSS.escape(blockId)}"]`);
  if (!el) return;
  el.classList.remove('agent-changed');
  void el.offsetWidth;
  el.classList.add('agent-changed');
  el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  setTimeout(() => el.classList.remove('agent-changed'), 1600);
}

export function setViewport(root: HTMLElement, size: 'desktop' | 'tablet' | 'phone') {
  root.dataset.viewport = size;
}

// ---- cek keterbacaan sederhana (kontras teks utama vs latar) ----
function rgb(color: string): [number, number, number, number] | null {
  const m = color.match(/rgba?\(([^)]+)\)/);
  if (!m) return null;
  const [r, g, b, a = '1'] = m[1].split(/[ ,/]+/).filter(Boolean);
  return [Number(r), Number(g), Number(b), Number(a)];
}
function lum([r, g, b]: number[]) {
  const f = (c: number) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}
export function contrast(a: [number, number, number], b: [number, number, number]) {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}
function readable(root: HTMLElement): boolean {
  const probe = root.querySelector<HTMLElement>('.design-page') ?? root;
  const style = getComputedStyle(probe);
  const fg = rgb(style.color);
  let bgEl: HTMLElement | null = probe;
  let bg: ReturnType<typeof rgb> = null;
  while (bgEl) {
    bg = rgb(getComputedStyle(bgEl).backgroundColor);
    if (bg && bg[3] > 0.5) break;
    bgEl = bgEl.parentElement;
  }
  if (!fg || !bg || bg[3] <= 0.5) return true;
  return contrast([fg[0], fg[1], fg[2]], [bg[0], bg[1], bg[2]]) >= 3 && style.visibility !== 'hidden' && Number(style.opacity) > 0.2;
}

/** ID blok yang paling relevan dari sebuah op desain (untuk disorot). */
export function opTarget(op: Record<string, unknown>): string | undefined {
  if (typeof op.id === 'string') return op.id;
  if (op.block && typeof (op.block as { id?: unknown }).id === 'string') return (op.block as { id: string }).id;
  if (typeof op.new_id === 'string') return op.new_id;
  return undefined;
}
