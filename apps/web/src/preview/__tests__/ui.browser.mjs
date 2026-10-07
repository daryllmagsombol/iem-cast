import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { chromium, expect } from '@playwright/test';

const servers = [];
let browser;
async function server(config, port) {
  const launch = `import { createServer } from 'vite'; const server = await createServer({ configFile: ${JSON.stringify(config)}, cacheDir: 'node_modules/.vite-uitest-${port}', server: { port: ${port}, strictPort: true, host: '127.0.0.1' } }); await server.listen(); server.printUrls(); process.on('SIGTERM', async () => { await server.close(); process.exit(0); });`;
  const child = spawn(process.execPath, ['--input-type=module', '-e', launch], { cwd: 'apps/web', stdio: ['ignore', 'pipe', 'pipe'] });
  servers.push(child);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Vite startup timeout')), 15000);
    child.stdout.on('data', chunk => { if (String(chunk).includes(`:${port}`)) { clearTimeout(timeout); resolve(); } });
    child.stderr.on('data', chunk => process.stderr.write(chunk));
    child.on('exit', code => { clearTimeout(timeout); reject(new Error(`Vite exited ${code}`)); });
  });
}
try {
  await server('vite.admin.config.ts', 5189);
  await server('vite.musician.config.ts', 5190);
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  await page.addInitScript(() => {
    window.forbiddenAudioCalls = [];
    const forbidden = name => { window.forbiddenAudioCalls.push(name); throw new Error(`Unexpected media call: ${name}`); };
    window.RTCPeerConnection = class { constructor() { forbidden('RTCPeerConnection'); } };
    HTMLMediaElement.prototype.play = () => { forbidden('play'); return Promise.reject(new Error('No playback permitted')); };
    if (navigator.mediaDevices) navigator.mediaDevices.getUserMedia = () => { forbidden('getUserMedia'); return Promise.reject(new Error('No microphone permitted')); };
  });
  const errors = []; page.on('pageerror', e => { errors.push(e.message); console.error('PAGE ERROR:', e.message); });
  page.on('console', m => { if (m.type() === 'error') { errors.push(m.text()); console.error('CONSOLE ERROR:', m.text()); } });
  for (const width of [1280, 320]) {
    await page.setViewportSize({ width, height: 800 });
    await page.goto('http://127.0.0.1:5189/');
    await expect(page.getByRole('heading', { name: 'Operator dashboard' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Start host' })).toBeDisabled();
    await page.getByLabel('Certificate path', { exact: true }).fill('/local/test.pem');
    await expect(page.getByLabel('Certificate path', { exact: true })).toHaveValue('/local/test.pem');
    await page.getByRole('button', { name: 'Open simulated preview' }).click();
    await expect(page.getByText('SIMULATED · No host connection · No audio')).toBeVisible();
    await page.getByRole('button', { name: 'Select simulated device' }).click();
    await expect(page.getByText('Simulated device selected — no capture')).toBeVisible();
    await page.getByRole('button', { name: 'Unmute simulated Lead vocal' }).click();
    await expect(page.getByRole('button', { name: 'Mute simulated Lead vocal' })).toBeVisible();
    for (const theme of ['light', 'dark']) {
      await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `preview overflow ${width}/${theme}`);
    }
    await page.getByRole('button', { name: 'Exit preview' }).click();
    await expect(page.getByRole('heading', { name: 'Operator dashboard' })).toBeVisible();
    assert.deepEqual(await page.evaluate(() => window.forbiddenAudioCalls), []);
    if (width === 1280) await page.screenshot({ path: 'node_modules/.vite-uitest-operator.png', fullPage: true });
  }
  await page.goto('http://127.0.0.1:5189/');
  await page.evaluate(async () => { const h = await import('/src/admin/__tests__/browserHarness.tsx'); window.fixtureCalls = h.mountOperator(); });
  await expect(page.getByText('Browser-test USB fixture', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Select device' }).click();
  await expect(page.getByText(/Selected: Browser-test USB fixture/)).toBeVisible();
  await page.getByLabel('Host network interface').selectOption('192.0.2.10');
  await expect(page.getByRole('button', { name: 'Generate pairing code' })).toBeDisabled();
  assert.equal(await page.evaluate(() => window.fixtureCalls.includes('FORBIDDEN')), false);

  await page.goto('http://127.0.0.1:5190/');
  await expect(page.getByRole('heading', { name: 'Receiver unavailable' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Connect', exact: true })).toHaveCount(0);
  await page.goto('http://127.0.0.1:5189/');
  await page.evaluate(async () => { const h = await import('/src/admin/__tests__/browserHarness.tsx'); window.fixtureCalls = h.mountReceiver(); });
  await page.getByRole('button', { name: 'Start listening' }).click();
  await expect(page.getByRole('button', { name: 'Stop listening' })).toBeVisible();
  const sourceAction = page.getByRole('button', { name: 'Unmute A deliberately long browser-test vocal source name', exact: true });
  const labelFit = await sourceAction.evaluate(e => {
    const range = document.createRange(); range.selectNodeContents(e);
    const style = getComputedStyle(e); const box = e.getBoundingClientRect();
    return { lines: range.getClientRects().length, textWidth: range.getBoundingClientRect().width, contentWidth: box.width - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) - parseFloat(style.borderLeftWidth) - parseFloat(style.borderRightWidth), height: box.height };
  });
  assert.equal(labelFit.lines, 1, 'source action label must remain on one line');
  assert.ok(labelFit.textWidth <= labelFit.contentWidth + 0.5, `source label overflow: ${JSON.stringify(labelFit)}`);
  assert.ok(labelFit.height >= 48);
  await page.keyboard.press('Tab');
  await sourceAction.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('button', { name: 'Mute A deliberately long browser-test vocal source name', exact: true })).toBeVisible();
  await page.screenshot({ path: 'node_modules/.vite-uitest-receiver.png', fullPage: true });
  for (const [width, height] of [[320, 720], [720, 320], [320, 480]]) {
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `receiver overflow ${width}`);
    assert.ok(await page.getByRole('button', { name: 'Stop listening' }).evaluate(e => e.getBoundingClientRect().height) >= 48);
    const slider = page.getByRole('slider', { name: 'A deliberately long browser-test vocal source name', exact: true });
    await page.keyboard.press('Tab');
    await slider.focus();
    const bounds = await slider.boundingBox();
    const dockTop = await page.locator('.iem-master-dock').evaluate(e => e.getBoundingClientRect().top);
    assert.ok(bounds.y + bounds.height <= dockTop, `focused gain obscured at ${width}x${height}`);
    assert.equal(await slider.evaluate(e => getComputedStyle(e).outlineStyle), 'solid');
    const previousGain = Number(await slider.inputValue());
    await page.keyboard.press('ArrowLeft');
    await expect(slider).toHaveValue(String(Math.max(-60, previousGain - 1)));
    await page.getByText('Master attenuation & details', { exact: true }).click();
    await page.getByRole('spinbutton', { name: 'Personal Master attenuation level in decibels' }).focus();
    await slider.focus();
    await page.waitForTimeout(100);
    const expandedBounds = await slider.boundingBox();
    const expandedDock = await page.locator('.iem-master-dock').evaluate(e => e.getBoundingClientRect().top);
    assert.ok(expandedBounds.y + expandedBounds.height <= expandedDock, `expanded dock obscures focus at ${width}x${height}`);
    await page.locator('.iem-master-details').evaluate(e => e.open = false);
  }
  await page.setViewportSize({ width: 320, height: 720 });
  await page.getByText('Master attenuation & details', { exact: true }).click();
  await page.getByRole('spinbutton', { name: 'Personal Master attenuation level in decibels' }).fill('-24');
  await page.getByRole('button', { name: 'Stop listening' }).click();
  assert.ok(await page.evaluate(() => window.fixtureCalls.includes('stop')));
  await page.emulateMedia({ forcedColors: 'active' });
  await page.keyboard.press('Tab');
  await page.getByRole('button', { name: 'Stop listening' }).focus();
  assert.equal(await page.getByRole('button', { name: 'Stop listening' }).evaluate(e => getComputedStyle(e).outlineStyle), 'solid');
  assert.deepEqual(errors, []);
  console.log('PASS: browser admin, native fixture enumeration/selection, isolated preview/exit, mobile receiver unavailable, gesture fixture, 320px/landscape overflow, 48px targets, focus/dock, themes, reduced motion, forced colors; zero browser errors.');
} finally {
  if (browser) await browser.close();
  for (const child of servers) { if (child.exitCode === null) { const exited = once(child, 'exit'); child.kill('SIGTERM'); await exited; } }
}
