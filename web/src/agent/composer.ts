// Logika keyboard composer, dipisah supaya bisa dites tanpa DOM.
export type KeyIntent = 'send' | 'newline' | 'stop' | 'edit-last' | 'none';

export function keyIntent(e: { key: string; shiftKey: boolean; ctrlKey?: boolean; metaKey?: boolean; isComposing?: boolean; keyCode?: number },
  state: { empty: boolean; running: boolean }): KeyIntent {
  // Jangan kirim saat IME sedang menyusun kata (keyboard Jepang/Cina, dsb.).
  if (e.isComposing || e.keyCode === 229) return 'none';
  if (e.key === 'Escape' && state.running) return 'stop';
  if (e.key === 'Enter') {
    if (e.shiftKey) return 'newline';
    return state.empty ? 'none' : 'send';
  }
  if (e.key === 'ArrowUp' && state.empty && !e.shiftKey) return 'edit-last';
  return 'none';
}

export type Slash = { command: 'undo' | 'redo' | 'reset' | 'new' | 'apply' | 'discard' };

export const SLASH_HELP: [string, string][] = [
  ['/undo', 'Batalkan perubahan desain terakhir'],
  ['/redo', 'Ulangi perubahan yang dibatalkan'],
  ['/terapkan', 'Jadikan draft desain aktif'],
  ['/buang', 'Buang draft, kembali ke desain aktif'],
  ['/reset', 'Kembali ke desain bawaan'],
  ['/baru', 'Mulai percakapan baru'],
];

export function parseSlash(text: string): Slash | null {
  const [cmd] = text.trim().toLowerCase().split(/\s+/, 2);
  switch (cmd) {
    case '/undo': return { command: 'undo' };
    case '/redo': return { command: 'redo' };
    case '/reset': return { command: 'reset' };
    case '/baru': case '/new': return { command: 'new' };
    case '/terapkan': case '/apply': return { command: 'apply' };
    case '/buang': return { command: 'discard' };
    default: return null;
  }
}
