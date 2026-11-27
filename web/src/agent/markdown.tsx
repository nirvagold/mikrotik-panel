// Markdown ringan yang aman: menghasilkan VNode, tidak pernah innerHTML.
// Mendukung judul #–####, paragraf, **tebal**, *miring*, `kode`, [tautan](https://…),
// blok ```kode```, daftar - / 1. (bertingkat), tabel | a | b |, kutipan >, dan garis ---.
import type { ComponentChildren } from 'preact';

const INLINE = /(`[^`]+`|\*\*[^*]+\*\*|__[^_]+__|\*[^*\s][^*]*\*|\[[^\]]+\]\([^)\s]+\))/g;

function safeHref(url: string): string | null {
  return /^https?:\/\//i.test(url) ? url : null;
}

function inline(text: string, key: string | number = 0): ComponentChildren[] {
  const out: ComponentChildren[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  const re = new RegExp(INLINE.source, 'g');
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const t = m[0];
    const k = `${key}-${m.index}`;
    if (t.startsWith('`')) out.push(<code key={k}>{t.slice(1, -1)}</code>);
    else if (t.startsWith('**') || t.startsWith('__')) out.push(<strong key={k}>{inline(t.slice(2, -2), k)}</strong>);
    else if (t.startsWith('[')) {
      const link = t.match(/^\[([^\]]+)\]\(([^)\s]+)\)$/)!;
      const href = safeHref(link[2]);
      out.push(href ? <a key={k} href={href} target="_blank" rel="noopener noreferrer">{link[1]}</a> : link[1]);
    } else out.push(<em key={k}>{t.slice(1, -1)}</em>);
    last = m.index + t.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

const LIST = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const TABLE_SEP = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;
const BLOCK_START = /^(```|#{1,6}\s|\s*([-*+]|\d+[.)])\s+|>|\s*(-{3,}|\*{3,}|_{3,})\s*$)/;

const indentOf = (s: string) => s.replace(/\t/g, '    ').length;

function cells(row: string): string[] {
  return row.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map(c => c.trim());
}

type Item = { depth: number; ordered: boolean; text: string };

/** Daftar bertingkat dari baris-baris berindentasi. */
function renderList(items: Item[], key: string): ComponentChildren {
  const base = items[0].depth;
  const ordered = items[0].ordered;
  const lis: ComponentChildren[] = [];
  let i = 0;
  while (i < items.length) {
    const item = items[i];
    let j = i + 1;
    while (j < items.length && items[j].depth > base) j++;
    const children = items.slice(i + 1, j);
    lis.push(<li key={i}>{inline(item.text, `${key}-${i}`)}{children.length > 0 && renderList(children, `${key}-${i}`)}</li>);
    i = j;
  }
  return ordered ? <ol key={key}>{lis}</ol> : <ul key={key}>{lis}</ul>;
}

function blocks(lines: string[], prefix = ''): ComponentChildren[] {
  const out: ComponentChildren[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const key = `${prefix}${i}`;
    if (line.trimStart().startsWith('```')) {
      const lang = line.trim().slice(3).trim();
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trimStart().startsWith('```')) code.push(lines[i++]);
      i++;
      out.push(<pre key={key} data-lang={lang || undefined}><code>{code.join('\n')}</code></pre>);
      continue;
    }
    if (!line.trim()) { i++; continue; }
    const heading = line.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/);
    if (heading) {
      const level = Math.min(heading[1].length, 4);
      out.push(<p key={key} class={`md-heading h${level}`} role="heading" aria-level={level + 1}>{inline(heading[2], key)}</p>);
      i++;
      continue;
    }
    if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) { out.push(<hr key={key} />); i++; continue; }
    if (line.trimStart().startsWith('>')) {
      const quoted: string[] = [];
      while (i < lines.length && lines[i].trimStart().startsWith('>')) quoted.push(lines[i++].trimStart().replace(/^>\s?/, ''));
      out.push(<blockquote key={key}>{blocks(quoted, `${key}q`)}</blockquote>);
      continue;
    }
    if (line.includes('|') && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1])) {
      const head = cells(line);
      const align = cells(lines[i + 1]).map(c => c.endsWith(':') ? (c.startsWith(':') ? 'center' : 'right') : undefined);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) rows.push(cells(lines[i++]));
      out.push(
        <div key={key} class="md-table">
          <table>
            <thead><tr>{head.map((c, n) => <th key={n} style={align[n] ? { textAlign: align[n] } : undefined}>{inline(c, `${key}h${n}`)}</th>)}</tr></thead>
            <tbody>{rows.map((r, ri) => <tr key={ri}>{head.map((_, n) => <td key={n} style={align[n] ? { textAlign: align[n] } : undefined}>{inline(r[n] ?? '', `${key}r${ri}c${n}`)}</td>)}</tr>)}</tbody>
          </table>
        </div>,
      );
      continue;
    }
    if (LIST.test(line)) {
      const items: Item[] = [];
      while (i < lines.length) {
        const m = lines[i].match(LIST);
        if (m) { items.push({ depth: indentOf(m[1]), ordered: /\d/.test(m[2]), text: m[3] }); i++; continue; }
        // Baris lanjutan berindentasi ikut item sebelumnya; baris kosong di antara item dilewati.
        if (items.length && lines[i].trim() && /^\s{2,}/.test(lines[i])) { items[items.length - 1].text += ' ' + lines[i].trim(); i++; continue; }
        if (!lines[i].trim() && i + 1 < lines.length && LIST.test(lines[i + 1]) && indentOf(lines[i + 1].match(LIST)![1]) >= items[0].depth) { i++; continue; }
        break;
      }
      out.push(renderList(items, key));
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && !BLOCK_START.test(lines[i])
      && !(lines[i].includes('|') && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1]))) para.push(lines[i++]);
    if (!para.length) para.push(lines[i++]);
    out.push(<p key={key}>{para.flatMap((l, j) => (j ? [<br key={`b${j}`} />, ...inline(l, `${key}-${j}`)] : inline(l, `${key}-${j}`)))}</p>);
  }
  return out;
}

export function Markdown({ text }: { text: string }) {
  return <div class="md">{blocks(text.replace(/\r\n/g, '\n').split('\n'))}</div>;
}
