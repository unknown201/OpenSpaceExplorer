import {expect,test,type Page} from '@playwright/test';

// Orbital colonies (docs/colonies-design.md, section 8), end to end through the real client and a local server:
// a crewed pod with a colony trunk goes straight to orbit, founds a colony there from the trunk's part window,
// and a second craft rolls out of the colony's dock, in orbit beside the station and paid for from its stock.

async function build(page:Page,name:string,parts:string[]){
  await page.locator('[data-ose="vab-clear"]').click();
  for(const part of parts){
    await page.locator('[data-ose="vab-search"]').fill(part);
    const card=page.locator('[data-ose="vab-part"]').first();
    await card.waitFor({timeout:5000});
    await card.dblclick();
  }
  await page.locator('[data-ose="vab-search"]').fill('');
  await page.locator('[data-ose="vab-name"]').fill(name);
}

test('a station founds a colony in orbit and launches a craft from its dock',async({page})=>{
  test.setTimeout(240_000);
  page.on('dialog',d=>{
    if(d.type()==='prompt')d.accept('Gateway Colony');else d.accept();
  });
  await page.goto('/');
  await page.locator('[data-ose="player-name"]').fill('orbital colonist');
  await page.locator('[data-ose="login"]').click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if(!await page.locator('#create-name').isVisible())await page.locator('#create-details > summary').click();
  await page.locator('#create-name').fill(`orbital colony ${Date.now()}`);
  await page.selectOption('#create-mode','Sandbox');
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-launchpad"]').waitFor();
  const hints=page.getByRole('button',{name:'No more hints',exact:true});
  if(await hints.isVisible())await hints.click();

  // The station: a crewed pod over a small colony trunk, saved, then put straight into orbit.
  await page.locator('[data-ose="ksc-vab"]').first().click();
  await build(page,'Gateway',['pod_1v_conical_crew','colony_trunk_small']);
  await page.locator('[data-ose="vab-save"]').click();
  await page.locator('[data-ose="vab-crumb-ksc"]').evaluate(()=>(document.getElementById('vab-back') as HTMLElement).click());
  await page.locator('[data-ose="ksc-launchpad"]').click();
  await page.locator('[data-ose="craft"]').filter({hasText:'Gateway'}).first().click();
  await page.locator('[data-ose="lp-orbit"]').click();
  const hud=page.locator('#flight-hud');
  await expect(hud).toHaveClass(/open/);
  const station=(await hud.getAttribute('data-ose-vessel'))!;
  await expect(hud).toHaveAttribute('data-ose-situation','Orbit',{timeout:20_000});

  // Found the colony from the trunk's part window.
  await page.locator('#fh-parts-btn').click();
  await page.locator('#fh-parts-list [data-part]').filter({hasText:/trunk/i}).first().click();
  await page.locator('[data-act="fh-act-colony"]').click();
  await expect(page.locator('body')).toContainText('Gateway Colony founded',{timeout:15_000});

  // A second craft from the colony's dock: in orbit, next to the station, a vessel of its own.
  await page.locator('#fh-ksc').click();
  await page.locator('[data-ose="ksc-vab"]').first().click();
  await build(page,'Shuttle',['pod_1v_conical_crew']);
  const site=page.locator('[data-ose="vab-site"]');
  const dock=site.locator('option',{hasText:'Gateway Colony dock (Kerbin orbit)'});
  await expect(dock).toHaveCount(1,{timeout:15_000});
  await site.selectOption((await dock.getAttribute('value'))!);
  await page.locator('[data-ose="vab-launch"]').click();
  await expect(hud).toHaveClass(/open/,{timeout:20_000});
  await expect.poll(()=>hud.getAttribute('data-ose-vessel'),{timeout:20_000}).not.toBe(station);
  await expect(hud).toHaveAttribute('data-ose-situation','Orbit',{timeout:20_000});
  // the station is about 400 m ahead: the dock trails it on the same orbit
  const nearest=async()=>Number((await hud.getAttribute('data-ose-nearest-m'))??'NaN');
  // (the first reading after the launch can come from an older cached state of the station)
  await expect.poll(async()=>Math.abs(await nearest()-400),{timeout:20_000}).toBeLessThan(50);

  // The colony's card counts the launch and still lists the station as its own.
  await page.locator('#fh-ksc').click();
  await page.locator('[data-ose="ksc-tracking"]').first().click();
  await page.locator('[data-ose="ts-tab-colonies"]').click();
  await page.locator('[data-ose="ts-colony"]').filter({hasText:'Gateway Colony'}).first().click();
  const card=page.locator('#tracking');
  await expect(card).toContainText('in orbit');
  await expect(card).toContainText(/Launches\s*1/);
});
