// Titik masuk modul agen untuk aplikasi utama.
//
//   import './agent/design.css';
//   import { mountAgentPanel, startDesign, DesignPage } from './agent';
//   startDesign(document.querySelector('.panel-root')!);   // sekali, setelah login
//   mountAgentPanel();                                       // panel chat (Shadow DOM)
//   <DesignPage page={...} renderers={{ metrics: Metrics, customer_table: CustomerTable, ... }} />
export { mountAgentPanel } from './mount';
export { DesignPage, type BlockRenderers } from './DesignView';
export * as design from './design-store';
export { startDesign, liveSpec } from './design-store';
export type * from './types';
