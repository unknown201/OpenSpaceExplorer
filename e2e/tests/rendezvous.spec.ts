import {expect,test} from '@playwright/test';

test('optional rendezvous plans preserve manual flight and release the hold on steering',async({page})=>{
  await page.goto('/');
  await page.locator('[data-ose="player-name"]').fill('rendezvous pilot');
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if(!await page.locator('#create-name').isVisible())await page.locator('#create-details > summary').click();
  await page.locator('#create-name').fill(`rendezvous ${Date.now()}`);
  await page.selectOption('#create-mode','Sandbox');
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-launchpad"]').waitFor();
  const hints=page.getByRole('button',{name:'No more hints',exact:true});
  if(await hints.isVisible())await hints.click();
  async function launch(){
    await page.locator('[data-ose="ksc-launchpad"]').click();
    await page.locator('[data-ose="craft"]').filter({hasText:'Stock12_StartRocket'}).first().click();
    await page.locator('[data-ose="lp-orbit"]').click();
    await expect(page.locator('#flight-hud')).toHaveClass(/open/);
    return (await page.locator('#flight-hud').getAttribute('data-ose-vessel'))!;
  }
  const core=await launch();
  // Give the core actual orbital separation before inserting the next test craft at the launch longitude.
  await page.waitForTimeout(5500);
  await page.locator('#fh-ksc').click();
  const delivery=await launch();expect(delivery).not.toBe(core);
  await page.locator('#rdv-panel > summary').click();
  await expect(page.locator('#rdv-level')).toHaveValue('off');
  await page.selectOption('#rdv-destination',core);
  await page.locator('#rdv-plan').click();
  await expect(page.locator('#rdv-panel')).toHaveAttribute('data-phase','Intercept burn');
  await expect(page.locator('#rdv-values')).toContainText('Δv remaining');
  await expect(page.locator('#fh-throttle-pct')).toHaveText('0%');
  // The maneuver radial should follow the same box as the dedicated guidance hold.
  await page.locator('#fh-sas-maneuver').click();
  await expect(page.locator('#rdv-hold')).toHaveAttribute('aria-pressed','true');
  await expect(page.locator('#fh-sas-maneuver')).toHaveClass(/on/);
  await expect(page.locator('#fh-sas-stability')).not.toHaveClass(/on/);
  await expect(page.locator('#fh-attitude')).toHaveText('Holding rendezvous cue');
  await expect(page.locator('#fh-throttle-pct')).toHaveText('0%');
  await expect.poll(()=>page.evaluate(()=>{
    const label=[...document.querySelectorAll('#rdv-values dt')].find(e=>e.textContent==='Cue error');
    return parseFloat(label?.nextElementSibling?.textContent||'Infinity');
  }),{timeout:45_000}).toBeLessThan(3);
  await page.keyboard.down('w');await page.waitForTimeout(200);await page.keyboard.up('w');
  await expect(page.locator('#rdv-hold')).toHaveAttribute('aria-pressed','false');
  await expect(page.locator('#fh-sas-maneuver')).not.toHaveClass(/on/);
  await page.locator('#rdv-hold').click();
  await expect(page.locator('#fh-sas-maneuver')).toHaveClass(/on/);
  await page.locator('#fh-sas-prograde').click();
  await expect(page.locator('#rdv-hold')).toHaveAttribute('aria-pressed','false');
  await expect(page.locator('#fh-sas-prograde')).toHaveClass(/on/);
  await page.selectOption('#rdv-level','minimal');await expect(page.locator('#rdv-help')).toBeHidden();
  await page.selectOption('#rdv-level','off');
  await expect(page.locator('#rdv-destination')).toHaveValue(core);
  await expect(page.locator('#rdv-world')).toBeHidden();
  await page.locator('#fh-sas-maneuver').click();
  // Without a scheduled node, the existing maneuver mode holds the current direction.
  await expect(page.locator('#fh-attitude')).toHaveText('Holding a direction');
  await expect(page.locator('#rdv-hold')).toHaveAttribute('aria-pressed','false');
  await expect(page.locator('#fh-throttle-pct')).toHaveText('0%');
});
