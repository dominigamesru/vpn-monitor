// Скриншоты для README из галереи docs/screens.html. Запуск из корня репо: node .claude/skills/release/readme_shots.js
const { chromium } = require('playwright');
const path = require('path');
const root = process.cwd();
(async () => {
  const b = await chromium.launch();
  const p = await b.newPage({ viewport: { width: 1640, height: 1000 }, deviceScaleFactor: 2 });
  await p.goto('file://' + path.join(root, 'docs/screens.html')); await p.waitForTimeout(3500);
  const figs = await p.$$('figure');
  const want = { 'Можно работать': 'flyout-ok', 'SVN качает медленно (VPN режет скорость)': 'flyout-slow', 'Окно — «Сейчас», SVN медленный': 'window-now', 'Окно — «История»': 'window-history' };
  for (const f of figs) {
    const cap = await f.$eval('figcaption', (e) => e.textContent);
    if (!want[cap]) continue;
    const box = await f.$('.box');
    if (cap.startsWith('Окно')) {
      const ifr = await box.$('iframe'); await ifr.evaluate((n) => { n.height = 2600; n.width = 1100; n.parentElement.style.overflow = 'visible'; }); await p.waitForTimeout(800);
      const fr = await ifr.contentFrame();
      const sel = cap.includes('История') ? '[data-tab="history"]' : '[data-tab="now"]';
      const el = await fr.$(sel); await el.screenshot({ path: path.join(root, `docs/img/${want[cap]}.png`) });
    } else await box.screenshot({ path: path.join(root, `docs/img/${want[cap]}.png`) });
  }
  await b.close();
})();
