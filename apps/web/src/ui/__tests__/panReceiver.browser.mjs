import assert from 'node:assert/strict';
import { chromium, webkit, expect } from '@playwright/test';
import { createServer } from 'vite';

// From apps/web. Owned strict port 5198 and independently launched browsers; never attaches to a
// shared session. Real events at phone + desktop sizes, light/dark, reduced motion.
const server = await createServer({ configFile: 'vite.musician.config.ts',
  server: { host: '127.0.0.1', port: 5198, strictPort: true } });
let browser;
try {
  await server.listen();
  const url = server.resolvedUrls.local[0];
  console.log(`Owned pan receiver Vite: PID ${process.pid}, ${url}`);
  for (const [engineName, engine] of [['chromium', chromium], ['webkit', webkit]]) {
    browser = await engine.launch({ headless: true });
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.goto(url);
    await page.evaluate(async () => {
      const harness = await import('/src/ui/__tests__/panReceiver.browserHarness.tsx');
      window.panReceiver = harness.mountPanReceiver();
    });
    const events = () => page.evaluate(() => window.panReceiver.events);

    // Mono non-master source would be pannable; the fixture's mono source is masterLr (no pan).
    // Valid pairs: Keys pair AND the both-masterLr pair each render one shared balance; plus the
    // disabled invalid-pair group => three "Linked stereo pair" headings.
    await expect(page.getByText('Linked stereo pair')).toHaveCount(3);
    const balance = page.getByRole('slider', { name: 'Keys L / Keys R balance', exact: true });
    await expect(balance).toHaveValue('40');
    await expect(page.getByRole('slider', { name: 'Keys L', exact: true })).toHaveCount(1);
    await expect(page.getByRole('slider', { name: 'Keys R', exact: true })).toHaveCount(1);
    await expect(page.getByText('Left · Keys L')).toBeVisible();
    await expect(page.getByText('Right · Keys R')).toBeVisible();
    const keysGroup = page.getByRole('region', { name: 'Keys L and Keys R stereo pair' });
    await expect(keysGroup.getByTestId('pair-balance-helper')).toHaveText(
      'Center keeps left and right unchanged. Moving toward one side turns down the other side.',
    );

    // A VALID reciprocal both-masterLr pair uses the balance law: grouped pair, one shared balance,
    // and no per-member mono pan.
    const masterBalance = page.getByRole('slider', { name: 'Master L / Master R balance', exact: true });
    await expect(masterBalance).toBeEnabled();
    await expect(masterBalance).toHaveValue('10');
    await expect(page.getByRole('slider', { name: 'Master L pan', exact: true })).toHaveCount(0);
    await expect(page.getByRole('slider', { name: 'Master R pan', exact: true })).toHaveCount(0);
    await masterBalance.fill('50');
    await expect.poll(events).toContainEqual({ kind: 'balance', args: ['masterL', 'masterR', 0.5] });

    // Invalid (non-reciprocal) pair: its balance is disabled and it exposes no per-member pan.
    const invalidBalance = page.getByRole('slider', { name: `${'A deliberately long browser-test vocal source name'} L / ${'A deliberately long browser-test vocal source name'} R balance`, exact: true });
    await expect(invalidBalance).toBeDisabled();
    await expect(page.getByRole('slider', { name: /L pan$/ })).toHaveCount(0);
    await expect(page.getByRole('slider', { name: /R pan$/ })).toHaveCount(0);

    // Unpaired legacy MasterLr shows no pan.
    await expect(page.getByRole('slider', { name: `${'A deliberately long browser-test vocal source name'} pan`, exact: true })).toHaveCount(0);

    // Real range/keyboard events on the shared balance emit normalized pair payloads once.
    await balance.fill('60');
    await expect.poll(events).toContainEqual({ kind: 'balance', args: ['keysL', 'keysR', 0.6] });
    await balance.press('Home');
    await expect.poll(events).toContainEqual({ kind: 'balance', args: ['keysL', 'keysR', -1] });
    await page.getByRole('button', { name: 'Center Keys L / Keys R balance' }).click();
    await expect.poll(events).toContainEqual({ kind: 'balance', args: ['keysL', 'keysR', 0] });

    // Independent member gain still routes to the per-source callback, never balance.
    const before = (await events()).length;
    await page.getByRole('slider', { name: 'Keys L', exact: true }).fill('-24');
    await expect.poll(events).toContainEqual({ kind: 'gain', args: ['keysL', -24] });
    assert.ok((await events()).slice(before).every(e => e.kind === 'gain'), `${engineName}: gain touched balance`);

    for (const [width, height] of [[320, 720], [390, 844], [1280, 900], [720, 320]]) {
      await page.setViewportSize({ width, height });
      for (const theme of ['light', 'dark']) {
        await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
        await page.emulateMedia({ reducedMotion: 'reduce' });
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false, `${engineName}: overflow ${width}/${theme}`);
        const clipped = await page.locator('label, [data-testid="pan-readout"], [data-testid="pair-balance-helper"]').evaluateAll(elements => elements.some(element => {
          const range = document.createRange(); range.selectNodeContents(element);
          const bounds = range.getBoundingClientRect();
          return bounds.left < 0 || bounds.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 1;
        }));
        assert.equal(clipped, false, `${engineName}: clipped label/readout ${width}/${theme}`);
      }
    }

    // 200% text scaling on the narrowest width, and 48px targets.
    await page.setViewportSize({ width: 320, height: 720 });
    await page.evaluate(() => document.documentElement.style.fontSize = '200%');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false, `${engineName}: 200% overflow`);
    await page.evaluate(() => document.documentElement.style.fontSize = '');
    for (const control of await page.locator('.iem-range, .iem-channel-action button').all()) {
      const box = await control.boundingBox();
      if (box) assert.ok(box.height >= 44, `${engineName}: control under target`);
    }
    // Visible keyboard-focus indicator on the shared balance. WebKit omits range inputs from native
    // Tab order (macOS platform behavior), so we focus then use a real key, which is what grants
    // `:focus-visible` in both engines.
    await balance.scrollIntoViewIfNeeded();
    await balance.focus();
    await page.keyboard.press('ArrowLeft');
    const focus = await balance.evaluate(element => {
      const style = getComputedStyle(element);
      return { outline: style.outlineStyle, width: parseFloat(style.outlineWidth), active: document.activeElement === element };
    });
    assert.equal(focus.active, true, `${engineName}: balance not focusable`);
    assert.notEqual(focus.outline, 'none');
    assert.ok(focus.width >= 2);

    if (engineName === 'chromium') {
      const path = `/var/folders/sm/chc_1z6x5zs_124gk_g__ynm0000gn/T/opencode/pan-receiver-390-light.png`;
      await page.setViewportSize({ width: 390, height: 1500 });
      await page.evaluate(() => document.documentElement.dataset.theme = 'light');
      await page.screenshot({ path, fullPage: true });
      console.log(`Screenshot: ${path}`);
    }
    assert.deepEqual(errors, [], `${engineName}: browser console/page errors`);
    await page.evaluate(() => window.panReceiver.dispose());
    console.log(`PASS ${engineName}: pair grouping, single balance, invalid-pair disabled, no masterLr pan, normalized payloads, 320/390/desktop/landscape light/dark, 200% text, focus/targets`);
    await browser.close();
    browser = undefined;
  }
} finally {
  await browser?.close();
  await server.close();
  console.log(`Closed owned pan receiver Vite/browser (PID ${process.pid}).`);
}
