const { chromium, devices } = require('playwright');
(async () => {
  const b = await chromium.launch(); const res = {};
  const ctx = await b.newContext({ ...devices['Pixel 7'] });
  await ctx.addInitScript(() => {
    const o = Element.prototype.attachShadow; Element.prototype.attachShadow = function (i) { return o.call(this, { ...i, mode: 'open' }); };
    // visualViewport tiruan: Playwright tidak bisa memunculkan keyboard layar.
    const fake = new EventTarget(); let h = null, t = 0;
    Object.defineProperties(fake, { height: { get: () => h ?? innerHeight }, offsetTop: { get: () => t }, width: { get: () => innerWidth } });
    Object.defineProperty(window, 'visualViewport', { value: fake, configurable: true });
    window.__kb = (nh, nt) => { h = nh; t = nt; fake.dispatchEvent(new Event('resize')); fake.dispatchEvent(new Event('scroll')); };
  });
  const p = await ctx.newPage();
  await p.goto('http://127.0.0.1:8799/'); await p.fill('input[type=password]', 'rahasia-admin-123'); await p.click('button[type=submit]');
  await p.waitForSelector('.panel-root'); await p.waitForTimeout(1000);
  await p.locator('.tabbar button.agent, .tabbar [aria-label*="gen"]').first().click().catch(async () => p.evaluate(() => dispatchEvent(new Event('agent:toggle'))));
  await p.waitForTimeout(400);
  const ta = p.locator('textarea[aria-label="Pesan untuk agen"]');
  await p.locator('[aria-label="Percakapan baru"]').click();
  await ta.fill('Tampilkan kartu data'); await ta.press('Enter');
  await p.waitForTimeout(5000);
  res.cards = await p.locator('figure.datacard').count();
  res.rows = await p.locator('figure.datacard tbody tr').count();
  res.bars = await p.locator('.bars li').count();
  res.lines = await p.locator('.chart polyline').count();
  res.topoNodes = await p.locator('.topo g.node').count();
  res.raw = await p.locator('figure.datacard pre.raw').count();
  await p.screenshot({ path: 'shots/kartu-hp.png', fullPage: false });
  await p.evaluate(() => document.getElementById('agent-host').shadowRoot.querySelectorAll('figure.datacard')[3].scrollIntoView({block:'center'})); await p.waitForTimeout(600); await p.screenshot({ path: 'shots/kartu-topologi-hp.png' });
  const vh = p.viewportSize().height;
  await ta.focus();
  await p.evaluate(() => window.__kb(Math.round(innerHeight * 0.55), 0)); await p.waitForTimeout(200);
  const box = async () => p.evaluate(() => {
    const root = document.getElementById('agent-host').shadowRoot;
    const r = root.querySelector('section.panel').getBoundingClientRect();
    const c = root.querySelector('.composer').getBoundingClientRect();
    const h = root.querySelector('.head').getBoundingClientRect();
    return { top: r.top, bottom: r.bottom, composerBottom: c.bottom, headTop: h.top, overflow: document.documentElement.style.overflow };
  });
  res.vh = vh; res.kbOpen = await box();
  await p.screenshot({ path: 'shots/keyboard-terbuka-hp.png' });
  // iOS: halaman tergulir, visualViewport.offsetTop > 0
  await p.evaluate(() => window.__kb(Math.round(innerHeight * 0.55), 120)); await p.waitForTimeout(200);
  res.kbScrolled = await box();
  await p.evaluate(() => window.__kb(innerHeight, 0)); await p.waitForTimeout(200);
  res.kbClosed = await box();
  await p.locator('[aria-label="Tutup panel agen"]').click(); await p.waitForTimeout(200);
  res.overflowAfterClose = await p.evaluate(() => document.documentElement.style.overflow);
  await ctx.close();
  // desktop: tidak terpengaruh
  const d = await b.newContext({ viewport: { width: 1400, height: 860 }, deviceScaleFactor: 2 });
  await d.addInitScript(() => { const o = Element.prototype.attachShadow; Element.prototype.attachShadow = function (i) { return o.call(this, { ...i, mode: 'open' }); }; });
  const q = await d.newPage();
  await q.goto('http://127.0.0.1:8799/'); await q.fill('input[type=password]', 'rahasia-admin-123'); await q.click('button[type=submit]');
  await q.waitForSelector('.panel-root'); await q.waitForTimeout(800);
  await q.locator('[aria-label="Buka agen AI (Ctrl+K)"]').click(); await q.waitForTimeout(1500);
  res.desktop = await q.evaluate(() => { const r = document.getElementById('agent-host').shadowRoot.querySelector('section.panel').getBoundingClientRect(); return { top: r.top, h: r.height, overflow: document.documentElement.style.overflow }; });
  await q.screenshot({ path: 'shots/kartu-desktop.png' });
  console.log(JSON.stringify(res, null, 1)); await b.close();
})();
