import assert from 'node:assert/strict';
import { chromium, webkit, expect } from '@playwright/test';
import { createServer } from 'vite';

// Run from apps/web. Owns a new Vite instance and browser, closes only those instances.
const server = await createServer({ configFile: 'vite.musician.config.ts',
  server: { host: '127.0.0.1', port: 5194, strictPort: true } });
let browser;
try {
  await server.listen();
  const url = server.resolvedUrls.local[0];
  console.log(`Owned Vite server: PID ${process.pid}, ${url}`);
  for (const [name, engine] of [['chromium', chromium], ['webkit', webkit]]) {
    browser = await engine.launch({ headless: true });
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.addInitScript(() => {
      window.forbiddenMediaCalls = [];
      const forbidden = name => { window.forbiddenMediaCalls.push(name); throw new Error(`Unexpected media call: ${name}`); };
      window.RTCPeerConnection = class { constructor() { forbidden('RTCPeerConnection'); } };
      HTMLMediaElement.prototype.play = () => forbidden('play');
    });
    await page.goto(url);
    await page.evaluate(async () => {
      const harness = await import('/src/ui/__tests__/LatencyDiagnostics.browserHarness.tsx');
      window.diagnosticsFixture = harness.mountDiagnostics();
    });
    const region = page.getByRole('region', { name: 'Latency diagnostics' });
    const summary = region.locator('summary');
    const details = region.locator('details');
    const reading = label => region.locator('dl > div').filter({ has: page.getByText(label, { exact: true }) }).locator('dd');
    await expect(region).toBeVisible();
    await expect(details).not.toHaveAttribute('open');
    await expect(reading('Network RTT')).toHaveText('42 ms');
    await expect(reading('Browser buffer delay')).toHaveText('152 ms');
    await expect(reading('End-to-end audio latency')).toHaveText('Not measured');
    await expect(reading('Browser target delay')).not.toBeVisible();
    await page.keyboard.press('Tab');
    await summary.focus();
    await page.keyboard.press('Enter');
    await expect(details).toHaveAttribute('open');
    await page.keyboard.press('Space');
    await expect(details).not.toHaveAttribute('open');
    await page.keyboard.press('Space');
    await expect(details).toHaveAttribute('open');
    for (const [width, height] of [[320, 720], [390, 844], [1280, 900], [720, 320]]) {
      await page.setViewportSize({ width, height });
      for (const theme of ['light', 'dark']) {
        await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
        await page.emulateMedia({ reducedMotion: 'reduce' });
        // Real keyboard focus triggers ReceiverView's dock-clearance behavior.
        await summary.scrollIntoViewIfNeeded();
        await summary.evaluate(element => element.blur());
        await summary.focus();
        await expect.poll(() => summary.evaluate(element => {
          const box = element.getBoundingClientRect();
          const dock = document.querySelector('.iem-master-dock').getBoundingClientRect();
          return box.top >= 0 && box.bottom <= dock.top;
        })).toBe(true);
        const focus = await summary.evaluate(element => {
          const style = getComputedStyle(element);
          return { outline: style.outlineStyle, width: parseFloat(style.outlineWidth), height: element.getBoundingClientRect().height };
        });
        assert.notEqual(focus.outline, 'none');
        assert.ok(focus.width >= 2);
        assert.ok(focus.height >= 48);
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false, `${name}: overflow ${width}/${theme}`);
        // Check full text bounds, not just the containing row, for clipping/overflow.
        const clipped = await region.locator('dt, dd, summary').evaluateAll(elements => elements.some(element => {
          if (!element.getClientRects().length) return false;
          const range = document.createRange(); range.selectNodeContents(element);
          const bounds = range.getBoundingClientRect();
          return bounds.left < 0 || bounds.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 1;
        }));
        assert.equal(clipped, false, `${name}: clipped reading ${width}/${theme}`);
        await expect(reading('Requested preference')).toHaveText('0 ms');
        await expect(reading('Request outcome')).toHaveText('Accepted');
        await expect(reading('Browser target delay')).toHaveText('160 ms');
        await expect(reading('Packets lost')).toHaveText('0 packets');
        await expect(reading('Audio concealment')).toHaveText('1.25%');
        // Lower readings can be scrolled completely above the real fixed master dock.
        for (const label of ['Requested preference', 'Request outcome']) {
          await reading(label).evaluate(element => {
            const box = element.getBoundingClientRect();
            const dock = document.querySelector('.iem-master-dock').getBoundingClientRect();
            window.scrollBy(0, box.bottom - dock.top + 24);
          });
          assert.equal(await reading(label).evaluate(element => {
            const box = element.getBoundingClientRect();
            const dock = document.querySelector('.iem-master-dock').getBoundingClientRect();
            return box.top >= 0 && box.bottom <= dock.top;
          }), true, `${name}: preference reading obscured ${width}/${theme}`);
        }
        if (name === 'chromium' && height > 320) {
          const path = `/var/folders/sm/chc_1z6x5zs_124gk_g__ynm0000gn/T/opencode/buffer-diagnostics-${width}-${theme}-card.png`;
          // Component review image only: use a taller capture viewport so the fixed dock does not
          // overlay this crop. All layout/focus assertions above used the original viewport.
          const cardHeight = await region.evaluate(element => element.getBoundingClientRect().height);
          await page.setViewportSize({ width, height: Math.ceil(cardHeight) + 300 });
          await region.evaluate(element => window.scrollTo(0, scrollY + element.getBoundingClientRect().top - 24));
          await region.screenshot({ path });
          await page.setViewportSize({ width, height });
          console.log(`Screenshot: ${path}`);
        }
      }
    }
    await page.setViewportSize({ width: 320, height: 720 });
    await page.evaluate(() => document.documentElement.style.fontSize = '200%');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false, `${name}: 200% text overflow`);
    await page.evaluate(() => document.documentElement.style.fontSize = '');
    for (const state of ['Unsupported', 'Rejected', 'Unavailable', 'Not requested']) {
      await page.evaluate(state => window.diagnosticsFixture.update({ jitter_buffer_preference: { state, requested_ms: null } }), state);
      await expect(reading('Request outcome')).toHaveText(state);
      await expect(reading('Requested preference')).toHaveText(state === 'Not requested' ? 'Not requested' : 'Unavailable');
      await expect(reading('Browser buffer delay')).toHaveText('152 ms');
    }
    await page.evaluate(() => window.diagnosticsFixture.update({ network_rtt_ms: 'Unavailable', buffer_delay_ms: 'Waiting for sample', buffer_target_delay_ms: 'Waiting for sample', buffer_minimum_delay_ms: 'Unavailable', packets_lost_delta: 'Waiting for sample', concealment_percent: 'Unavailable' }));
    await expect(details).toHaveAttribute('open');
    await expect(reading('Network RTT')).toHaveText('Unavailable');
    await expect(reading('Browser buffer delay')).toHaveText('Waiting for sample');
    await expect(reading('Browser target delay')).toHaveText('Waiting for sample');
    await expect(reading('Browser minimum delay')).toHaveText('Unavailable');
    await expect(reading('Packets lost')).toHaveText('Waiting for sample');
    await expect(reading('Audio concealment')).toHaveText('Unavailable');
    await expect(reading('End-to-end audio latency')).toHaveText('Not measured');
    assert.equal(await region.locator('[aria-live], [role="status"], [role="alert"]').count(), 0);
    assert.deepEqual(await page.evaluate(() => window.forbiddenMediaCalls), []);
    assert.deepEqual(errors, [], `${name}: browser errors`);
    await page.evaluate(() => window.diagnosticsFixture.dispose());
    console.log(`PASS ${name}: disclosure keyboard/focus, 320/390/1280/landscape light/dark, text scaling, interval units, preference states, no media/console errors`);
    await browser.close();
    browser = undefined;
  }
} finally {
  await browser?.close();
  await server.close();
  console.log(`Closed owned browser and Vite server (PID ${process.pid}).`);
}
