# Kontrak frontend agen ↔ backend

## Dependensi
- `preact`, `@preact/signals`, Vite (`agent.css?inline`).
- Dari `web/src/api.ts`: `get<T>(path)` dan `post<T>(path, body?)` (JSON, lempar `Error` berisi pesan untuk admin).
- Dari `web/src/events.ts`: `on(kind, handler): () => void`.

## Pemakaian di main.tsx
```ts
import './agent/design.css';
import { startDesign, mountAgentPanel, DesignPage, liveSpec } from './agent';
startDesign(document.querySelector('.panel-root')!); // setelah login
mountAgentPanel();                                     // Shadow DOM closed, #agent-host di body
// halaman: <DesignPage page={liveSpec.value.pages.find(p => p.id === id)} renderers={{ metrics, customer_table, ... }} />
// menu: liveSpec.value.nav
```

## Endpoint
| Method | Path | Body | Balasan |
|---|---|---|---|
| GET | /api/design | | `{draft, active}` (DesignVersion dengan `spec`) |
| POST | /api/design/versions/{id}/revert | | `{draft}`: draft baru salinan versi `id`, parent = draft lama |
| POST | /api/design/apply | | `{active}` |
| POST | /api/design/discard | | `{draft}` (salinan versi aktif) |
| POST | /api/design/reset | | `{draft}` (`default_spec()`) |
| GET | /api/agent/conversations | | `{conversations: [{id, title, updated_at}]}` |
| GET | /api/agent/conversations/{id}/messages | | `{messages: [{id, role, parts, design_version_id, created_at}], proposals: [ProposalCard], run_id \| null}` |
| POST | /api/agent/messages | `{conversation_id \| null, text, replace_from \| null}` | `{conversation_id, run_id, messages}` |
| POST | /api/agent/runs/{id}/cancel | | `{}` |
| POST | /api/agent/proposals/{id}/approve | `{typed?}` | `{card}` |
| POST | /api/agent/proposals/{id}/reject | | `{card}` |

- `replace_from`: hapus pesan itu dan semua sesudahnya, revert draft ke `design_version_id` terakhir sebelum pesan itu, lalu jalankan giliran baru.
- Simpan `TurnOutcome.design_version` pada pesan assistant terakhir giliran itu (untuk tombol "Kembalikan ke sini").
- `ProposalCard = {id, conversation_id, tool_use_id, tool, input, permission, preview, label, status: pending|approved|rejected|done|failed, result}`.
- Kartu kritis: `label` adalah teks yang harus diketik admin. Cek di server dengan `Permission::is_satisfied(approved, typed, label)`, jangan percaya frontend.

## Event SSE
`chat.delta`, `chat.tool`, `chat.thinking`, `chat.question`, `chat.done`, `design.patch {op, spec}`, `design.version`, `design.viewport`, `proposal.updated {card}`, `resync`.

## Test
`composer.test.ts` memakai `node --test --experimental-strip-types`; keluarkan `*.test.ts` dari `tsc` atau tambahkan `@types/node`.
