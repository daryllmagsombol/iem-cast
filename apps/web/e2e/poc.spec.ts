import { test, expect } from '@playwright/test';

// Automated browser gate (plan Task 9). Mocks/static only.
// This is NOT fulfillment of the real wireless/hardware requirement.
// Emulated WebKit is not an iPhone.

test('site loads its base path and shows the simulated demo notice', async ({ page }) => {
  await page.goto('/iem-cast/');
  await expect(page.getByText(/POC work in progress/i).first()).toBeVisible();
  await page.goto('/iem-cast/#/demo');
  await expect(page.getByText(/SIMULATED · silent demo · no host connection/i).first()).toBeVisible();
});

test('mobile_layout_has_no_horizontal_overflow', async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 720 });
  await page.goto('/iem-cast/#/demo');
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth + 1,
  );
  expect(overflow).toBe(false);
});

test('keyboard_focus_is_visible', async ({ page }) => {
  await page.goto('/iem-cast/#/demo');
  await page.keyboard.press('Tab');
  const outline = await page.evaluate(() => {
    const el = document.activeElement as HTMLElement | null;
    if (!el) return 'none';
    const style = getComputedStyle(el);
    return `${style.outlineStyle} ${style.boxShadow}`;
  });
  expect(outline).not.toBe('none none');
});

test('demo makes no LAN, signaling, or media requests', async ({ page }) => {
  const offending: string[] = [];
  page.on('request', (request) => {
    const url = request.url();
    if (/^(ws|wss):/.test(url)) offending.push(url);
    if (/\/api\/|\.local|stun:|turn:/.test(url)) offending.push(url);
  });
  await page.goto('/iem-cast/#/demo');
  await page.getByRole('button', { name: /start demo/i }).click();
  await page.waitForTimeout(1500);
  expect(offending).toEqual([]);
});

test('demo never claims measured end-to-end latency', async ({ page }) => {
  await page.goto('/iem-cast/#/demo');
  await page.getByRole('button', { name: /start demo/i }).click();
  await expect(page.getByText(/not measured/i).first()).toBeVisible();
});
