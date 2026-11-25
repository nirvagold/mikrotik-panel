// State desain global: draft (dilihat admin, diubah agen live) dan versi aktif.
import { computed, signal } from '@preact/signals';
import { get, post } from '../api';
import { on } from '../events';
import { applyDesign, highlight, opTarget, setViewport, transition } from './design-runtime';
import type { DesignSpec, DesignVersion } from './types';

export const draft = signal<DesignVersion | null>(null);
export const active = signal<DesignVersion | null>(null);
/** Admin melihat draft (default) atau versi aktif untuk membandingkan. */
export const viewing = signal<'draft' | 'active'>('draft');
/** Spec yang sedang dirender, termasuk patch live yang belum jadi versi. */
export const liveSpec = signal<DesignSpec | null>(null);
export const customCssDisabled = signal(false);
export const hasUnpublished = computed(() => !!draft.value && !!active.value && draft.value.id !== active.value.id);

const redoStack: string[] = [];
let root: HTMLElement | null = null;

function render(spec: DesignSpec, target?: string) {
  liveSpec.value = spec;
  if (!root) return;
  transition(() => {
    customCssDisabled.value = !applyDesign(spec, root!);
  });
  requestAnimationFrame(() => highlight(target));
}

export async function loadDesign() {
  const result = await get<{ draft: DesignVersion; active: DesignVersion }>('/api/design');
  draft.value = result.draft;
  active.value = result.active;
  render(viewing.value === 'draft' ? result.draft.spec : result.active.spec);
}

/** Pasang runtime desain pada elemen akar panel (`.panel-root`). Panggil sekali. */
export function startDesign(panelRoot: HTMLElement) {
  root = panelRoot;
  on('design.patch', (data: { op: Record<string, unknown>; spec: DesignSpec }) => {
    if (viewing.value !== 'draft') viewing.value = 'draft';
    render(data.spec, opTarget(data.op));
  });
  on('design.version', () => { redoStack.length = 0; void loadDesign(); });
  on('design.viewport', (data: { size: 'desktop' | 'tablet' | 'phone' }) => root && setViewport(root, data.size));
  on('resync', () => void loadDesign());
  void loadDesign();
}

export function view(which: 'draft' | 'active') {
  viewing.value = which;
  const version = which === 'draft' ? draft.value : active.value;
  if (version) render(version.spec);
}

/** Undo = kembali ke induk draft (server membuat draft baru salinan induk). */
export async function undo() {
  const current = draft.value;
  if (!current?.parent_id) return false;
  redoStack.push(current.id);
  await revertTo(current.parent_id, false);
  return true;
}

export async function redo() {
  const id = redoStack.pop();
  if (!id) return false;
  await revertTo(id, false);
  return true;
}

export async function revertTo(versionId: string, clearRedo = true) {
  if (clearRedo) redoStack.length = 0;
  const result = await post<{ draft: DesignVersion }>(`/api/design/versions/${encodeURIComponent(versionId)}/revert`);
  draft.value = result.draft;
  viewing.value = 'draft';
  render(result.draft.spec);
}

export async function publish() {
  const result = await post<{ active: DesignVersion }>('/api/design/apply');
  active.value = result.active;
  draft.value = result.active;
}

export async function discard() {
  const result = await post<{ draft: DesignVersion }>('/api/design/discard');
  draft.value = result.draft;
  render(result.draft.spec);
}

export async function resetToDefault() {
  const result = await post<{ draft: DesignVersion }>('/api/design/reset');
  draft.value = result.draft;
  render(result.draft.spec);
}
