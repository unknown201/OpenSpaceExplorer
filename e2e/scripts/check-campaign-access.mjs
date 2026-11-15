import { chromium } from '@playwright/test';
import assert from 'node:assert/strict';
import path from 'node:path';
const base = process.env.OSE_BASE_URL || 'http://127.0.0.1:7796';
const browser = await chromium.launch({headless:true, args:['--disable-gpu']});
const contexts = [];
async function pilot(code = '') {
  const context = await browser.newContext({viewport:{width:1440,height:1000}}); contexts.push(context);
  const page = await context.newPage();
  await page.goto(base);
  console.log('Loaded menu');
  await page.locator('#login-code').fill(code);
  await page.locator('#play').click();
  await page.locator('#screen-sessions.open').waitFor();
  await page.locator('#sessions .empty').waitFor();
  return page;
}
try {
  const owner = await pilot();
  const login = await owner.locator('#account-code').inputValue();
  assert.equal(login.length,32);
  const identity = await owner.locator('#player').textContent();
  await owner.locator('#create-details > summary').click();
  await owner.locator('#create-name').fill('Browser access test');
  await owner.locator('#create-mode').selectOption('Sandbox');
  await owner.locator('#create').click();
  console.log('Created campaign');
  await owner.locator('#screen-ksc.open').waitFor();
  console.log('Reached KSC');
  await owner.locator('body').click({position:{x:5,y:5}});
  await owner.keyboard.press('Escape');
  if (!(await owner.locator('#screen-paused.open').isVisible())) await owner.keyboard.press('Escape');
  await owner.locator('#screen-paused.open').waitFor();
  await owner.locator('#share-current').click();
  await owner.waitForFunction(() => document.querySelector('#paused-share-code').value.length === 32);
  const share = await owner.locator('#paused-share-code').inputValue();
  assert.notEqual(share,login);
  const friend = await pilot();
  assert.equal(await friend.locator('[data-ose="session"]').count(),0);
  await friend.locator('#join-code').fill(share);
  await friend.locator('#join-code-submit').click();
  await friend.locator('[data-ose="session"]').waitFor();
  assert.match(await friend.locator('#sessions').textContent(),/Browser access test/);
  await friend.locator('[data-ose="session-share"]').click();
  await friend.waitForFunction(code => document.querySelector('#session-share-code').value === code,share);
  // Fresh browser profile has no storage; the private login code restores the same server account.
  const context = await browser.newContext({viewport:{width:1440,height:1000}}); contexts.push(context);
  const otherMachine = await context.newPage();
  await otherMachine.goto(base);
  await otherMachine.locator('#login-code').fill(login);
  await otherMachine.locator('#play').click();
  await otherMachine.locator('[data-ose="session"]').waitFor();
  assert.equal(await otherMachine.locator('#player').textContent(),identity);
  assert.match(await otherMachine.locator('#sessions').textContent(),/Owned by you/);
  assert.equal(await otherMachine.locator('[data-ose="session"]').count(),1);
  await friend.screenshot({path:path.resolve('../.cache/campaign-sharing.png')});
  await otherMachine.screenshot({path:path.resolve('../.cache/campaign-account.png')});
  console.log('PASS: private list, in-game sharing, friend join, member sharing, and login from a clean browser profile.');
} catch (error) {
  for (let i=0;i<contexts.length;i++) {
    const page=contexts[i].pages()[0]; if (page) {
      console.log('UI state', i, await page.locator('#menu-note').textContent({timeout:1000}).catch(()=>null), await page.locator('.screen.open').evaluateAll(els=>els.map(e=>e.id)));
      await page.screenshot({path:path.resolve(`../.cache/campaign-failure-${i}.png`)});
    }
  }
  throw error;
} finally {
  for(const context of contexts) await context.close();
  await browser.close();
}

