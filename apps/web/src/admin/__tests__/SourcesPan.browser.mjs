import assert from 'node:assert/strict';
import { chromium, webkit, expect } from '@playwright/test';
import { createServer } from 'vite';

// From apps/web. Each instance uses its own strict port; never attaches to a shared browser.
const server = await createServer({ configFile: 'vite.admin.config.ts',
  server: { host: '127.0.0.1', port: 5202, strictPort: true } });
let browser;
try {
  await server.listen();
  const url = server.resolvedUrls.local[0];
  console.log(`Owned SourcesPan Vite: PID ${process.pid}, ${url}`);
  for (const [engineName, engine] of [['chromium', chromium], ['webkit', webkit]]) {
    browser = await engine.launch({ headless: true });
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.goto(url);
    await page.evaluate(async () => {
      const harness = await import('/src/admin/__tests__/SourcesPan.browserHarness.tsx');
      window.sourcesFixture = harness.mountSourcesFixture({ hostRunning: false });
    });
    await expect(page.getByRole('heading', { name: 'Defaults for new personal mixes' })).toBeVisible();
    await expect(page.getByText(/saved on this computer and apply only to new personal mixes/)).toBeVisible();

    const mono = page.getByRole('slider', { name: /browser-test vocal source name default pan/ });
    await expect(mono).toHaveValue('50');
    await mono.fill('-50');
    await expect(page.getByText('Saved').first()).toBeVisible();
    assert.equal(await page.evaluate(() => window.sourcesFixture.calls.includes('save-defaults')), true);

    // One grouped balance entry for the reciprocal pair; no per-member pan controls.
    const balance = page.getByRole('slider', { name: /keys left and Browser-test keys right default balance/ });
    await expect(balance).toHaveValue('-25');
    assert.equal(await page.getByRole('slider', { name: /keys left default pan/ }).count(), 0);

    await expect(page.getByRole('button', { name: 'Link', exact: true })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Unlink' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Reverse' })).toBeEnabled();

    // Host running: link actions are disabled, defaults editing stays enabled.
    await page.evaluate(async () => {
      await window.sourcesFixture.dispose();
      const harness = await import('/src/admin/__tests__/SourcesPan.browserHarness.tsx');
      window.sourcesFixture = harness.mountSourcesFixture({ hostRunning: true });
    });
    await expect(page.getByRole('button', { name: 'Link', exact: true })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Unlink' })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Reverse' })).toBeDisabled();
    await expect(page.getByRole('slider', { name: /browser-test vocal source name default pan/ })).toBeEnabled();

    for (const [width, height] of [[320, 720], [390, 844], [1280, 900]]) {
      await page.setViewportSize({ width, height });
      for (const theme of ['light', 'dark']) {
        await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
        await page.emulateMedia({ reducedMotion: 'reduce' });
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false, `${engineName}: overflow ${width}/${theme}`);
        for (const control of await page.getByRole('slider').all()) {
          const box = await control.boundingBox();
          assert.ok(box.width >= 48 && box.height >= 48, `${engineName}: small slider target ${width}/${theme}`);
        }
        for (const control of await page.getByRole('button').all()) {
          const box = await control.boundingBox();
          assert.ok(box.width >= 44 && box.height >= 44, `${engineName}: small button target ${width}/${theme}`);
        }
        if (engineName === 'chromium' && width === 320) {
          const path = `/var/folders/sm/chc_1z6x5zs_124gk_g__ynm0000gn/T/opencode/sources-pan-${width}-${theme}.png`;
          await page.screenshot({ path, fullPage: true });
          console.log(`Screenshot: ${path}`);
        }
      }
    }
    assert.deepEqual(errors, [], `${engineName}: browser console/page errors`);
    await page.evaluate(() => window.sourcesFixture.dispose());
    console.log(`PASS ${engineName}: grouped pair, mono pan save, host-running link gating, 320/390/desktop light/dark`);
    await browser.close();
    browser = undefined;
  }
} finally {
  await browser?.close();
  await server.close();
  console.log(`Closed owned SourcesPan Vite/browser (PID ${process.pid}).`);
}
