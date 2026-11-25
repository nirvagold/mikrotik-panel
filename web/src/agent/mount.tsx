// Pasang panel agen di Shadow DOM. CSS khusus buatan agen hanya berlaku di
// `.panel-root` dokumen utama, jadi tidak bisa menyembunyikan panel atau tombol Reset.
import { render } from 'preact';
import { AgentPanel } from './AgentPanel';
import panelCss from './agent.css?inline';

export function mountAgentPanel(): () => void {
  const host = document.createElement('div');
  host.id = 'agent-host';
  document.body.append(host);
  const shadow = host.attachShadow({ mode: 'closed' });
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(panelCss);
  shadow.adoptedStyleSheets = [sheet];
  const mountPoint = document.createElement('div');
  shadow.append(mountPoint);
  render(<AgentPanel />, mountPoint);
  return () => { render(null, mountPoint); host.remove(); };
}
