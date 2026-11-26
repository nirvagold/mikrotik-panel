const { chromium, devices } = require('playwright');
(async () => {
  const b = await chromium.launch(); const res = {};
  for (const [name, opts] of [['desktop', { viewport: { width: 1400, height: 860 }, deviceScaleFactor: 2 }], ['hp', { ...devices['Pixel 7'] }]]) {
    const ctx = await b.newContext(opts);
    await ctx.addInitScript(() => { const o = Element.prototype.attachShadow; Element.prototype.attachShadow = function (i) { return o.call(this, { ...i, mode: 'open' }); }; });
    const p = await ctx.newPage();
    await p.goto('http://127.0.0.1:8799/'); await p.fill('input[type=password]', 'rahasia-admin-123'); await p.click('button[type=submit]');
    await p.waitForSelector('.panel-root'); await p.waitForTimeout(1200);
    res[name + 'ClosedAtStart'] = await p.locator('textarea[aria-label="Pesan untuk agen"]').count() === 0;
    await p.screenshot({ path: `shots/popup-${name}-tertutup.png` });
    await p.locator('[aria-label="Buka agen AI (Ctrl+K)"]').click(); await p.waitForTimeout(400);
    const ta = p.locator('textarea[aria-label="Pesan untuk agen"]');
    await ta.fill('Ganti warna jadi hijau toska'); await ta.press('Enter'); await p.waitForTimeout(3500);
    await p.screenshot({ path: `shots/popup-${name}-terbuka.png` });
    res[name + 'Floating'] = await p.locator('section.panel.floating').count();
    if (name === 'desktop') { await ta.press('Escape'); await p.waitForTimeout(300); res.escCloses = await ta.count() === 0; }
    await ctx.close();
  }
  console.log(JSON.stringify(res)); await b.close();
})();
