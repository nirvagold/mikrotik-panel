// Cermin dari crates/agent/src/design.rs. Server adalah sumber kebenaran;
// frontend hanya merender spec yang sudah tervalidasi.
export type ThemeMode = 'light' | 'dark' | 'system';
export type Font = 'system' | 'inter' | 'ibm_plex_sans' | 'source_sans' | 'serif' | 'mono';

export type Palette = {
  bg: string; surface: string; surface_alt: string; text: string; muted: string; border: string;
  accent: string; on_accent: string; ok: string; warn: string; danger: string;
};

export type Tokens = {
  mode: ThemeMode; light: Palette; dark: Palette; font: Font; font_mono: Font;
  font_size: number; radius: number; density: number; shadow: 'none' | 'soft' | 'strong';
};

export type BlockKind =
  | { kind: 'metrics'; props: { items: string[] } }
  | { kind: 'traffic_chart'; props: { days: number; account?: string | null } }
  | { kind: 'customer_table'; props: { title: string; filter: string; columns: string[]; row_limit: number } }
  | { kind: 'low_quota'; props: { limit: number } }
  | { kind: 'router_status'; props: Record<string, never> }
  | { kind: 'topology'; props: Record<string, never> }
  | { kind: 'logs'; props: { topics: string[]; limit: number } }
  | { kind: 'recommendations'; props: { limit: number } }
  | { kind: 'action_button'; props: { action_id: string; label?: string | null } }
  | { kind: 'text'; props: { markdown: string } }
  | { kind: 'builtin'; props: { name: string } };

export type Block = { id: string; span: number } & BlockKind;
export type Page = { id: string; title: string; columns: number; blocks: Block[] };
export type NavItem = { page: string; label: string; icon: string };
export type DesignSpec = {
  tokens: Tokens;
  nav: { position: 'left' | 'top' | 'compact'; items: NavItem[] };
  pages: Page[];
  custom_css: string;
};

export type DesignVersion = { id: string; parent_id: string | null; status: 'draft' | 'active' | 'archived'; source: string; created_at: number; spec: DesignSpec };

// Pesan percakapan (sama dengan ai::Message di Rust).
export type Part =
  | { type: 'text'; text: string }
  | { type: 'tool_use'; id: string; name: string; input: unknown }
  | { type: 'tool_result'; tool_use_id: string; content: string; is_error?: boolean }
  | { type: 'opaque'; block: unknown };
export type ChatMessage = { id: string; role: 'user' | 'assistant'; parts: Part[]; design_version_id: string | null; created_at: number };

export type Permission = 'free' | 'approve' | 'critical';
export type ProposalCard = {
  id: string; conversation_id: string; tool_use_id: string | null; tool: string; input: Record<string, unknown>;
  permission: Permission; preview: string | null; label: string;
  status: 'pending' | 'approved' | 'rejected' | 'done' | 'failed'; result?: string | null;
};
export type Conversation = { id: string; title: string | null; updated_at: number };
