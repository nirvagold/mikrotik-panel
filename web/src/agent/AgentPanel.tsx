// Panel chat agen ala Claude. Dirender di Shadow DOM (lihat mount.tsx) supaya
// CSS khusus buatan agen tidak bisa merusak panel ini atau tombol Terapkan/Reset.
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { get, post } from '../api';
import { on } from '../events';
import { keyIntent, parseSlash, SLASH_HELP } from './composer';
import * as design from './design-store';
import { Markdown } from './markdown';
import type { ChatMessage, Conversation, Part, ProposalCard } from './types';

const TOOL_LABELS: Record<string, string> = {
  get_design: 'Membaca desain', set_tokens: 'Mengubah warna & font', set_theme_mode: 'Mengganti mode tema',
  add_block: 'Menambah blok', update_block: 'Mengubah blok', duplicate_block: 'Menggandakan blok', move_block: 'Memindah blok',
  remove_block: 'Menghapus blok', set_page_layout: 'Mengatur layout', add_page: 'Menambah halaman', remove_page: 'Menghapus halaman',
  set_nav: 'Mengatur menu', set_custom_css: 'Menulis CSS', preview_viewport: 'Pratinjau ukuran layar', revert_design: 'Mengembalikan desain',
  request_publish: 'Minta terapkan desain', router_snapshot: 'Membaca router', router_query: 'Membaca konfigurasi', get_topology: 'Memetakan topologi',
  get_traffic: 'Membaca trafik', get_logs: 'Membaca log', list_customers: 'Membaca pelanggan', customer_detail: 'Membaca detail pelanggan',
  snapshot_diff: 'Membandingkan konfigurasi', net_diagnose: 'Ping / traceroute', get_command_tree: 'Mengecek perintah RouterOS',
  search_routeros_docs: 'Mencari dokumentasi', check_command: 'Memvalidasi perintah', topup_quota: 'Usulan top-up',
  disconnect_session: 'Usulan putus sesi', set_customer_profile: 'Usulan ganti profil', set_customer_enabled: 'Usulan aktif/nonaktif',
  set_radius_user: 'Usulan RADIUS', propose_command: 'Usulan perintah', create_action_button: 'Usulan tombol baru',
  update_action_button: 'Usulan ubah tombol', delete_action_button: 'Usulan hapus tombol', run_action_button: 'Usulan jalankan tombol',
  revert_action_run: 'Usulan kembalikan perubahan', reboot_router: 'Usulan reboot', audit_router: 'Mengaudit router',
  create_recommendation: 'Membuat saran', traffic_report: 'Menyusun laporan', remember: 'Menyimpan memori', recall: 'Mencari memori',
  update_memory: 'Mengubah memori', forget: 'Menghapus memori', ask_user: 'Bertanya', show_card: 'Menampilkan data',
  set_conversation_title: 'Memberi judul', list_files: 'Melihat kode', read_file: 'Membaca kode', search_code: 'Mencari di kode',
  edit_file: 'Mengedit kode', create_file: 'Membuat file', delete_file: 'Menghapus file', show_diff: 'Menampilkan diff',
  build_and_test: 'Build & test', deploy_code: 'Usulan pasang kode', rollback_code: 'Rollback kode',
};

type ToolChip = { name: string; status: 'started' | 'done' | 'error' };
type Question = { question: string; options: string[] };

function storage(key: string, value?: string): string {
  try {
    if (value === undefined) return localStorage.getItem(key) ?? '';
    if (value) localStorage.setItem(key, value); else localStorage.removeItem(key);
  } catch { /* penyimpanan bisa diblokir; abaikan */ }
  return '';
}

const textOf = (parts: Part[]) => parts.filter((p): p is Extract<Part, { type: 'text' }> => p.type === 'text').map(p => p.text).join('');
/** Pesan user pertama berisi blok konteks otomatis; yang ditampilkan hanya teks admin. */
const userText = (m: ChatMessage) => {
  const texts = m.parts.filter((p): p is Extract<Part, { type: 'text' }> => p.type === 'text');
  return texts.length > 1 && texts[0].text.startsWith('[Konteks panel]') ? texts.slice(1).map(t => t.text).join('') : textOf(m.parts);
};
const isToolResults = (m: ChatMessage) => m.role === 'user' && m.parts.every(p => p.type === 'tool_result');

export function AgentPanel() {
  const [open, setOpen] = useState(() => storage('agent-open') !== '0');
  const [docked, setDocked] = useState(() => storage('agent-docked') !== '0');
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [conversation, setConversation] = useState<string | null>(() => storage('agent-conversation') || null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [proposals, setProposals] = useState<ProposalCard[]>([]);
  const [streaming, setStreaming] = useState('');
  const [tools, setTools] = useState<ToolChip[]>([]);
  const [thinking, setThinking] = useState(false);
  const [runId, setRunId] = useState<string | null>(null);
  const [question, setQuestion] = useState<Question | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [draftText, setDraftText] = useState(() => storage(`agent-draft:${storage('agent-conversation') || 'new'}`));
  const [problem, setProblem] = useState('');
  const [typed, setTyped] = useState<Record<string, string>>({});
  const input = useRef<HTMLTextAreaElement>(null);
  const log = useRef<HTMLDivElement>(null);
  const convRef = useRef(conversation);
  convRef.current = conversation;
  const running = runId !== null;

  async function loadConversations() {
    try { setConversations((await get<{ conversations: Conversation[] }>('/api/agent/conversations')).conversations); } catch { /* tampil kosong */ }
  }
  async function loadMessages(id: string | null) {
    if (!id) { setMessages([]); setProposals([]); return; }
    try {
      const result = await get<{ messages: ChatMessage[]; proposals: ProposalCard[]; run_id: string | null }>(`/api/agent/conversations/${encodeURIComponent(id)}/messages`);
      setMessages(result.messages);
      setProposals(result.proposals);
      setRunId(result.run_id);
    } catch (e) { setProblem((e as Error).message); }
  }

  useEffect(() => { void loadConversations(); void loadMessages(conversation); }, []);
  useEffect(() => { storage('agent-conversation', conversation ?? ''); }, [conversation]);
  useEffect(() => {
    storage('agent-open', open ? '1' : '0');
    storage('agent-docked', docked ? '1' : '0');
    // Halaman utama memberi ruang untuk panel yang ditempel di samping.
    if (open && docked) document.documentElement.dataset.agentDock = ''; else delete document.documentElement.dataset.agentDock;
  }, [open, docked]);

  // Event realtime dari server.
  useEffect(() => {
    const mine = (d: { conversation?: string }) => d.conversation === convRef.current;
    const offs = [
      on('chat.delta', (d: { conversation: string; text: string }) => { if (mine(d)) { setThinking(false); setStreaming(s => s + d.text); } }),
      on('chat.thinking', (d: { conversation: string }) => { if (mine(d)) setThinking(true); }),
      on('chat.tool', (d: { conversation: string; name: string; status: ToolChip['status'] }) => {
        if (!mine(d)) return;
        setThinking(false);
        setTools(list => {
          const i = list.findIndex(t => t.name === d.name && t.status === 'started');
          if (d.status === 'started' || i < 0) return [...list, { name: d.name, status: d.status }];
          const next = list.slice(); next[i] = { name: d.name, status: d.status }; return next;
        });
      }),
      on('chat.question', (d: { conversation: string } & Question) => { if (mine(d)) setQuestion({ question: d.question, options: d.options }); }),
      on('chat.done', (d: { conversation: string }) => {
        if (!mine(d)) return;
        setRunId(null); setStreaming(''); setTools([]); setThinking(false);
        void loadMessages(d.conversation); void loadConversations();
      }),
      on('proposal.updated', (d: { card: ProposalCard }) => {
        if (d.card.conversation_id !== convRef.current) return;
        setProposals(list => list.some(p => p.id === d.card.id) ? list.map(p => p.id === d.card.id ? d.card : p) : [...list, d.card]);
      }),
      on('resync', () => { void loadMessages(convRef.current); void loadConversations(); }),
    ];
    return () => offs.forEach(off => off());
  }, []);

  // Pintasan global: Ctrl/Cmd+K buka-tutup, Ctrl+Z / Ctrl+Shift+Z undo-redo desain (di luar kolom teks).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === 'k') { e.preventDefault(); setOpen(o => !o); requestAnimationFrame(() => input.current?.focus()); return; }
      const target = e.composedPath()[0] as HTMLElement | undefined;
      const typing = target && (target.tagName === 'TEXTAREA' || target.tagName === 'INPUT' || target.isContentEditable);
      if (mod && e.key.toLowerCase() === 'z' && !typing) { e.preventDefault(); void (e.shiftKey ? design.redo() : design.undo()); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // Auto-scroll saat ada isi baru, kecuali admin sedang menggulir ke atas.
  useLayoutEffect(() => {
    const el = log.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 160) el.scrollTop = el.scrollHeight;
  }, [messages, streaming, tools, proposals, question]);

  // Textarea tumbuh otomatis (CSSOM, aman untuk CSP).
  useLayoutEffect(() => {
    const el = input.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
  }, [draftText, open]);

  function updateDraft(value: string) {
    setDraftText(value);
    storage(`agent-draft:${conversation ?? 'new'}`, value);
  }

  async function runSlash(text: string): Promise<boolean> {
    const slash = parseSlash(text);
    if (!slash) return false;
    try {
      switch (slash.command) {
        case 'undo': if (!(await design.undo())) setProblem('Tidak ada perubahan desain untuk dibatalkan.'); break;
        case 'redo': if (!(await design.redo())) setProblem('Tidak ada yang bisa diulang.'); break;
        case 'apply': await design.publish(); break;
        case 'discard': await design.discard(); break;
        case 'reset': await design.resetToDefault(); break;
        case 'new': newConversation(); break;
        case 'theme': await send(`Ganti mode tema ke ${slash.arg === 'dark' ? 'gelap' : slash.arg === 'light' ? 'terang' : 'ikut sistem'}.`); return true;
      }
      updateDraft('');
    } catch (e) { setProblem((e as Error).message); }
    return true;
  }

  async function send(text: string, replaceFrom?: string) {
    const body = text.trim();
    if (!body || running) return;
    setProblem(''); setQuestion(null); setStreaming(''); setTools([]);
    try {
      const result = await post<{ conversation_id: string; run_id: string; messages: ChatMessage[] }>('/api/agent/messages', {
        conversation_id: conversation, text: body, replace_from: replaceFrom ?? null,
      });
      setConversation(result.conversation_id);
      setMessages(result.messages);
      setRunId(result.run_id);
      setEditing(null);
      updateDraft('');
      storage(`agent-draft:new`, '');
    } catch (e) { setProblem((e as Error).message); }
  }

  async function submit() {
    const text = draftText.trim();
    if (!text) return;
    if (text.startsWith('/') && (await runSlash(text))) return;
    await send(text, editing ?? undefined);
  }

  async function stop() {
    if (!runId) return;
    try { await post(`/api/agent/runs/${encodeURIComponent(runId)}/cancel`); } catch (e) { setProblem((e as Error).message); }
  }

  function startEdit(m: ChatMessage) {
    setEditing(m.id);
    updateDraft(userText(m));
    requestAnimationFrame(() => { input.current?.focus(); input.current?.setSelectionRange(input.current.value.length, input.current.value.length); });
  }

  function onKeyDown(e: KeyboardEvent) {
    const intent = keyIntent(e, { empty: !draftText.trim(), running });
    if (intent === 'send') { e.preventDefault(); void submit(); }
    else if (intent === 'stop') { e.preventDefault(); void stop(); }
    else if (intent === 'edit-last') {
      const last = [...messages].reverse().find(m => m.role === 'user' && !isToolResults(m));
      if (last) { e.preventDefault(); startEdit(last); }
    } else if (e.key === 'Escape' && editing) { setEditing(null); updateDraft(''); }
  }

  function regenerate(assistantIndex: number) {
    const prior = messages.slice(0, assistantIndex).reverse().find(m => m.role === 'user' && !isToolResults(m));
    if (prior) void send(userText(prior), prior.id);
  }

  async function revertHere(m: ChatMessage) {
    if (!m.design_version_id) return;
    try { await design.revertTo(m.design_version_id); } catch (e) { setProblem((e as Error).message); }
  }

  async function decide(card: ProposalCard, approve: boolean) {
    try {
      const result = await post<{ card: ProposalCard }>(`/api/agent/proposals/${encodeURIComponent(card.id)}/${approve ? 'approve' : 'reject'}`,
        approve && card.permission === 'critical' ? { typed: typed[card.id] ?? '' } : {});
      setProposals(list => list.map(p => p.id === card.id ? result.card : p));
    } catch (e) { setProblem((e as Error).message); }
  }

  function newConversation() {
    setConversation(null); setMessages([]); setProposals([]); setEditing(null); setQuestion(null);
    updateDraft(storage('agent-draft:new'));
    requestAnimationFrame(() => input.current?.focus());
  }

  const slashHints = draftText.startsWith('/') && !draftText.includes(' ')
    ? SLASH_HELP.filter(([cmd]) => cmd.startsWith(draftText.toLowerCase())) : [];
  const visible = messages.filter(m => !isToolResults(m));
  const cardsFor = (m: ChatMessage) => {
    const ids = new Set(m.parts.filter(p => p.type === 'tool_use').map(p => (p as { id: string }).id));
    return proposals.filter(p => ids.has(p.tool_use_id ?? ''));
  };
  const orphanCards = proposals.filter(p => !messages.some(m => cardsFor(m).includes(p)));

  if (!open) {
    return <button class="launcher" type="button" onClick={() => { setOpen(true); requestAnimationFrame(() => input.current?.focus()); }} aria-label="Buka agen AI (Ctrl+K)">
      <span aria-hidden="true">✦</span> Agen AI {running && <i class="pulse" aria-label="sedang bekerja" />}
    </button>;
  }

  return (
    <section class={`panel ${docked ? 'docked' : 'floating'}`} aria-label="Agen AI">
      <header class="head">
        <span class="mark" aria-hidden="true">✦</span>
        <select class="conv" value={conversation ?? ''} aria-label="Percakapan"
          onChange={e => { const id = (e.target as HTMLSelectElement).value || null; setConversation(id); void loadMessages(id); }}>
          <option value="">Percakapan baru</option>
          {conversations.map(c => <option key={c.id} value={c.id}>{c.title || 'Tanpa judul'}</option>)}
        </select>
        <button class="icon" type="button" title="Percakapan baru" aria-label="Percakapan baru" onClick={newConversation}>＋</button>
        <button class="icon" type="button" title={docked ? 'Mengambang' : 'Tempel di samping'} aria-label="Ubah posisi panel" onClick={() => setDocked(d => !d)}>{docked ? '⧉' : '▥'}</button>
        <button class="icon" type="button" title="Tutup (Ctrl+K)" aria-label="Tutup panel agen" onClick={() => setOpen(false)}>✕</button>
      </header>

      {design.hasUnpublished.value && (
        <div class="draftbar" role="status">
          <span>{design.viewing.value === 'draft' ? 'Draft desain belum diterapkan' : 'Menampilkan versi aktif'}</span>
          <button type="button" class="link" onClick={() => design.view(design.viewing.value === 'draft' ? 'active' : 'draft')}>
            {design.viewing.value === 'draft' ? 'Lihat versi aktif' : 'Lihat draft'}</button>
          <div class="draftactions">
            <button type="button" class="ghost" onClick={() => void design.discard().catch(e => setProblem(e.message))}>Buang</button>
            <button type="button" class="primary" onClick={() => void design.publish().catch(e => setProblem(e.message))}>Terapkan</button>
          </div>
        </div>
      )}
      {design.customCssDisabled.value && <div class="warn" role="alert">CSS khusus dimatikan sementara karena membuat teks sulit dibaca. Minta agen memperbaikinya, atau ketik /undo.</div>}

      <div class="log" ref={log} role="log" aria-live="polite" aria-label="Percakapan dengan agen">
        {!visible.length && !running && (
          <div class="intro">
            <h2>Apa yang ingin Anda ubah?</h2>
            <p>Saya bisa mengubah tampilan panel secara langsung, membaca router, menyusun tombol aksi MikroTik, dan memberi saran.</p>
            {['Buat tema gelap dengan aksen hijau dan sudut lebih bulat', 'Taruh tabel pelanggan offline di paling atas Ringkasan', 'Buatkan tombol top-up 10 GiB untuk pelanggan yang dipilih', 'Audit keamanan router saya'].map(s =>
              <button key={s} type="button" class="suggest" onClick={() => { updateDraft(s); input.current?.focus(); }}>{s}</button>)}
          </div>
        )}
        {visible.map((m, i) => {
          const index = messages.indexOf(m);
          if (m.role === 'user') {
            return <article key={m.id} class={`msg user ${editing === m.id ? 'editing' : ''}`}>
              <div class="bubble">{userText(m)}</div>
              <div class="actions">
                <button type="button" onClick={() => startEdit(m)} disabled={running}>Edit</button>
                <button type="button" onClick={() => void navigator.clipboard?.writeText(userText(m))}>Salin</button>
              </div>
            </article>;
          }
          const text = textOf(m.parts);
          const used = m.parts.filter(p => p.type === 'tool_use') as Extract<Part, { type: 'tool_use' }>[];
          const lastOfTurn = !visible[i + 1] || visible[i + 1].role === 'user';
          return <article key={m.id} class="msg assistant">
            {used.length > 0 && <ul class="chips">{used.map(t => <li key={t.id} class="chip done">{TOOL_LABELS[t.name] ?? t.name}</li>)}</ul>}
            {text && <Markdown text={text} />}
            {cardsFor(m).map(card => <Card key={card.id} card={card} typed={typed[card.id] ?? ''} onType={v => setTyped(t => ({ ...t, [card.id]: v }))} onDecide={decide} />)}
            {lastOfTurn && !running && <div class="actions">
              {text && <button type="button" onClick={() => void navigator.clipboard?.writeText(text)}>Salin</button>}
              <button type="button" onClick={() => regenerate(index)}>Ulangi</button>
              {m.design_version_id && <button type="button" onClick={() => void revertHere(m)} title="Kembalikan desain seperti saat pesan ini">Kembalikan ke sini</button>}
            </div>}
          </article>;
        })}
        {orphanCards.map(card => <Card key={card.id} card={card} typed={typed[card.id] ?? ''} onType={v => setTyped(t => ({ ...t, [card.id]: v }))} onDecide={decide} />)}
        {running && (
          <article class="msg assistant live">
            {tools.length > 0 && <ul class="chips">{tools.map((t, i) => <li key={i} class={`chip ${t.status}`}>{TOOL_LABELS[t.name] ?? t.name}{t.status === 'started' ? '…' : t.status === 'error' ? ' ✕' : ' ✓'}</li>)}</ul>}
            {streaming ? <Markdown text={streaming} /> : <p class="muted">{thinking ? 'Berpikir…' : 'Menyiapkan…'}</p>}
            <span class="caret" aria-hidden="true" />
          </article>
        )}
        {question && !running && (
          <div class="question">
            <p>{question.question}</p>
            {question.options.map(o => <button key={o} type="button" onClick={() => void send(o)}>{o}</button>)}
          </div>
        )}
      </div>

      <form class="composer" onSubmit={e => { e.preventDefault(); void submit(); }}>
        {problem && <p class="problem" role="alert">{problem} <button type="button" class="link" onClick={() => setProblem('')}>Tutup</button></p>}
        {editing && <p class="editing-note">Mengedit pesan. Percakapan dan desain setelah pesan ini akan diganti. <button type="button" class="link" onClick={() => { setEditing(null); updateDraft(''); }}>Batal</button></p>}
        {slashHints.length > 0 && <ul class="slash">{slashHints.map(([cmd, help]) =>
          <li key={cmd}><button type="button" onClick={() => { updateDraft(cmd.split(' ')[0] + (cmd.includes(' ') ? ' ' : '')); input.current?.focus(); }}><b>{cmd}</b> {help}</button></li>)}</ul>}
        <textarea ref={input} rows={1} value={draftText} maxLength={8000} placeholder="Tulis pesan… (Enter kirim, Shift+Enter baris baru, / untuk perintah)"
          aria-label="Pesan untuk agen" onInput={e => updateDraft((e.target as HTMLTextAreaElement).value)} onKeyDown={onKeyDown} />
        {running
          ? <button type="button" class="send stop" onClick={() => void stop()} aria-label="Hentikan (Esc)">■</button>
          : <button type="submit" class="send" disabled={!draftText.trim()} aria-label="Kirim (Enter)">↑</button>}
      </form>
    </section>
  );
}

function Card({ card, typed, onType, onDecide }: { card: ProposalCard; typed: string; onType: (v: string) => void; onDecide: (c: ProposalCard, ok: boolean) => void }) {
  const critical = card.permission === 'critical';
  const phrase = card.label.trim().toLowerCase();
  return (
    <div class={`card ${card.permission} ${card.status}`}>
      <div class="card-head"><span class="badge">{critical ? 'Kritis' : 'Perlu persetujuan'}</span> <b>{card.label}</b></div>
      {card.preview && <pre class="cli"><code>{card.preview}</code></pre>}
      {card.status === 'pending' ? (
        <div class="card-actions">
          {critical && <label class="confirm"><span>Ketik <b>{phrase}</b> untuk melanjutkan</span>
            <input value={typed} onInput={e => onType((e.target as HTMLInputElement).value)} autoComplete="off" spellcheck={false} /></label>}
          <button type="button" class="ghost" onClick={() => onDecide(card, false)}>Tolak</button>
          <button type="button" class={critical ? 'danger' : 'primary'} disabled={critical && typed.trim().toLowerCase() !== phrase} onClick={() => onDecide(card, true)}>
            {critical ? 'Jalankan' : 'Setujui'}</button>
        </div>
      ) : <p class="card-status">{({ approved: 'Disetujui, sedang dijalankan…', rejected: 'Ditolak.', done: 'Selesai.', failed: 'Gagal.' } as Record<string, string>)[card.status]}{card.result ? ` ${card.result}` : ''}</p>}
    </div>
  );
}
