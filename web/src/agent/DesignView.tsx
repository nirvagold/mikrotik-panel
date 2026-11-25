// Merender satu halaman dari design spec. Isi tiap jenis blok disediakan aplikasi
// lewat `renderers` (data pelanggan, grafik, dll.), jadi file ini hanya mengurus layout.
import type { ComponentType } from 'preact';
import { Markdown } from './markdown';
import type { Block, Page } from './types';

export type BlockRenderers = Partial<Record<Block['kind'], ComponentType<{ block: Block }>>>;

export function DesignPage({ page, renderers }: { page: Page; renderers: BlockRenderers }) {
  return (
    <section class="design-page" data-page-id={page.id}>
      <h1 class="design-title">{page.title}</h1>
      <div class={`design-grid cols-${page.columns}`}>
        {page.blocks.map(block => {
          const Renderer = renderers[block.kind];
          return (
            <div key={block.id} class={`design-block kind-${block.kind}`} data-block-id={block.id} style={{ gridColumn: `span ${block.span}` }}>
              {block.kind === 'text' ? <Markdown text={block.props.markdown} />
                : Renderer ? <Renderer block={block} />
                : <p class="design-missing">Blok “{block.kind}” belum tersedia.</p>}
            </div>
          );
        })}
      </div>
    </section>
  );
}
