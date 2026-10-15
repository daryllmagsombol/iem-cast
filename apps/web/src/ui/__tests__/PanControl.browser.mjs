import assert from 'node:assert/strict';
import { chromium, webkit, expect } from '@playwright/test';
import { createServer } from 'vite';

// From apps/web. This instance uses a dedicated strict port; never attaches to a shared browser.
const server = await createServer({ configFile: 'vite.musician.config.ts',
  server: { host: '127.0.0.1', port: 5196, strictPort: true } });
let browser;
try {
  await server.listen();
  const url = server.resolvedUrls.local[0];
  console.log(`Owned PanControl Vite: PID ${process.pid}, ${url}`);
  for (const [engineName, engine] of [['chromium', chromium], ['webkit', webkit]]) {
    browser = await engine.launch({ headless: true });
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.goto(url);
    await page.evaluate(async () => {
      const harness = await import('/src/ui/__tests__/PanControl.browserHarness.tsx');
      window.panFixture = harness.mountPanControls();
    });
    const region = page.getByRole('region', { name: 'Pan fixture', exact: true });
    const slider = region.getByRole('slider');
    const readout = region.getByTestId('pan-readout');
    const lastEvent = () => page.evaluate(() => window.panFixture.events.at(-1));
    const set = async (value, disabled = false) => {
      await page.evaluate(({ value, disabled }) => window.panFixture.update(value, disabled), { value, disabled });
      await expect(slider).toHaveValue(String(Math.round(value * 100)));
    };
    await expect(slider).toHaveValue('50');
    await expect(slider).toHaveAccessibleName('A deliberately long browser-test source or stereo pair name pan');
    await slider.fill('-50');
    await expect(readout).toHaveText('50% left');
    assert.equal(await lastEvent(), -0.5);
    await set(0.5);
    for (const [key, normalized] of [['ArrowLeft', 0.49], ['ArrowRight', 0.51], ['ArrowDown', 0.49], ['ArrowUp', 0.51], ['PageDown', 0.4], ['PageUp', 0.6], ['Home', -1], ['End', 1]]) {
      await set(0.5);
      const before = await page.evaluate(() => window.panFixture.events.length);
      await slider.press(key);
      await expect(slider).toHaveValue(String(normalized * 100));
      assert.equal(await lastEvent(), normalized);
      assert.equal(await page.evaluate(() => window.panFixture.events.length), before + 1, `${engineName}: duplicate native/custom keyboard event`);
    }
    await set(0.5);
    await region.getByRole('button', { name: /^Move .* left$/ }).click();
    assert.equal(await lastEvent(), 0.49);
    await region.getByRole('button', { name: /^Move .* right$/ }).click();
    assert.equal(await lastEvent(), 0.5);
    const reset = region.getByRole('button', { name: /^Center / });
    await reset.press('Enter');
    assert.equal(await lastEvent(), 'reset');
    await expect(readout).toHaveText('Center');
    await expect(reset).toBeDisabled();
    await set(-1);
    await expect(region.getByRole('button', { name: /^Move .* left$/ })).toBeDisabled();
    await expect(readout).toHaveText('Full left');
    await set(1);
    await expect(region.getByRole('button', { name: /^Move .* right$/ })).toBeDisabled();
    await expect(readout).toHaveText('Full right');
    await set(0.5, true);
    for (const element of await region.locator('input, button').all()) await expect(element).toBeDisabled();
    await set(0.5);
    // Real touch taps are a non-drag alternative to adjusting the range.
    const touchPage = await browser.newPage({ hasTouch: true, viewport: { width: 390, height: 844 } });
    touchPage.on('pageerror', error => errors.push(error.message));
    touchPage.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await touchPage.goto(url);
    await touchPage.evaluate(async () => {
      const harness = await import('/src/ui/__tests__/PanControl.browserHarness.tsx');
      window.panFixture = harness.mountPanControls();
    });
    const touchRegion = touchPage.getByRole('region', { name: 'Pan fixture', exact: true });
    await touchRegion.getByRole('button', { name: /^Move .* left$/ }).tap();
    await expect(touchRegion.getByTestId('pan-readout')).toHaveText('49% right');
    await touchRegion.getByRole('button', { name: /^Center / }).tap();
    await expect(touchRegion.getByTestId('pan-readout')).toHaveText('Center');
    await touchPage.close();
    for (const [width, height] of [[320, 720], [390, 844], [1280, 900], [720, 320]]) {
      await page.setViewportSize({ width, height });
      for (const theme of ['light', 'dark']) {
        await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
        await page.emulateMedia({ reducedMotion: 'reduce' });
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false, `${engineName}: overflow ${width}/${theme}`);
        for (const control of await page.locator('input, button').all()) {
          const box = await control.boundingBox();
          assert.ok(box.width >= 48 && box.height >= 48, `${engineName}: small target ${width}/${theme}`);
        }
        for (const control of await region.locator('button, input').all()) {
          await control.scrollIntoViewIfNeeded();
          await page.keyboard.press('Tab');
          await control.focus();
          const focus = await control.evaluate(element => {
            const style = getComputedStyle(element);
            const box = element.getBoundingClientRect();
            return { outline: style.outlineStyle, width: parseFloat(style.outlineWidth), top: box.top, bottom: box.bottom };
          });
          assert.notEqual(focus.outline, 'none');
          assert.ok(focus.width >= 2 && focus.top >= 0 && focus.bottom <= height);
        }
        const clipped = await page.locator('label, [data-testid="pan-readout"]').evaluateAll(elements => elements.some(element => {
          const range = document.createRange(); range.selectNodeContents(element);
          const bounds = range.getBoundingClientRect();
          return bounds.left < 0 || bounds.right > innerWidth + 1 || element.scrollWidth > element.clientWidth + 1;
        }));
        assert.equal(clipped, false, `${engineName}: clipped label/value ${width}/${theme}`);
        if (engineName === 'chromium' && height > 320) {
          const path = `/var/folders/sm/chc_1z6x5zs_124gk_g__ynm0000gn/T/opencode/pan-control-${width}-${theme}.png`;
          await page.screenshot({ path, fullPage: true });
          console.log(`Screenshot: ${path}`);
        }
      }
    }
    await page.setViewportSize({ width: 320, height: 720 });
    await page.evaluate(() => document.documentElement.style.fontSize = '200%');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false, `${engineName}: 200% text overflow`);
    await expect(readout).toHaveText('50% right');
    assert.deepEqual(errors, [], `${engineName}: browser console/page errors`);
    await page.evaluate(() => window.panFixture.dispose());
    console.log(`PASS ${engineName}: normalized input/keyboard/step/reset, touch alternatives, 320/390/desktop/landscape light/dark, focus/targets/text scaling`);
    await browser.close();
    browser = undefined;
  }
} finally {
  await browser?.close();
  await server.close();
  console.log(`Closed owned PanControl Vite/browser (PID ${process.pid}).`);
}
