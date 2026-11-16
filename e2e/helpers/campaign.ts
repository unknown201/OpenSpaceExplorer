import { expect, type Page } from '@playwright/test';

// A fresh connection leaves the test's room and drains its pending gameplay updates before deletion.
export async function deleteTestCampaign(page: Page, name: string) {
  await page.reload();
  await page.locator('[data-ose="play"]').click();
  const session = page.locator('[data-ose="session"]').filter({ hasText: name });
  const button = session.locator('[data-ose="session-delete"]');
  await expect(button).toBeEnabled();
  page.once('dialog', dialog => dialog.accept());
  await button.click();
  await expect(session).toHaveCount(0);
}
