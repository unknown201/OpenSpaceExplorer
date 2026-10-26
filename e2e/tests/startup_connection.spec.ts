import {expect,test,type WebSocketRoute} from '@playwright/test';

test.use({serviceWorkers:'block'});
test.beforeEach(async({page})=>{
  await page.addInitScript(()=>Object.defineProperty(navigator,'gpu',{value:undefined,configurable:true}));
});

test('connects while choosing a name and signs in only when Campaign is requested',async({page,baseURL})=>{
  const messages:Buffer[]=[];
  let release!:()=>void;const held=new Promise<void>(resolve=>{release=resolve;});
  await page.routeWebSocket('**/ws',socket=>{
    const server=socket.connectToServer();
    socket.onMessage(message=>{if(Buffer.isBuffer(message))messages.push(message);server.send(message);});
    server.onMessage(async message=>{await held;socket.send(message);});
  });
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
  try {
    await page.goto(baseURL!);
    const menu=page.locator('[data-ose="menu"]');
    await expect(menu).toHaveAttribute('data-ose-connection','open');
    await expect(page.locator('#menu-note')).toHaveText('');
    expect(messages).toHaveLength(0);
    await page.locator('[data-ose="player-name"]').fill('chosen after connection');
    await page.locator('[data-ose="play"]').click();
    await expect(menu).toHaveAttribute('data-ose-connection','authenticating');
    await page.locator('[data-ose="play"]').click();
    await page.locator('[data-ose="player-name"]').press('Enter');
    await expect.poll(()=>messages.length).toBe(1);
    expect(messages[0].includes(Buffer.from('chosen after connection'))).toBe(true);
    await expect(page.locator('#screen-name')).toHaveClass(/open/);
    release();
    await expect(menu).toHaveAttribute('data-ose-connection','ready');
    await expect(page.locator('[data-ose="session-join"]').first()).toBeVisible();
    expect(errors).toEqual([]);
  } finally {release();}
});

test('Campaign waits safely for a slow connection without opening another socket',async({page,baseURL})=>{
  let connections=0;
  let release!:()=>void;const held=new Promise<void>(resolve=>{release=resolve;});
  await page.routeWebSocket('**/ws',async socket=>{connections++;await held;socket.connectToServer();});
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
  try {
    await page.goto(baseURL!);
    await expect(page.locator('[data-ose="menu"]')).toHaveAttribute('data-ose-connection','connecting');
    await page.locator('[data-ose="player-name"]').fill('impatient pilot');
    await page.locator('[data-ose="play"]').click();
    await page.locator('[data-ose="play"]').click();
    await expect(page.locator('#screen-name')).toHaveClass(/open/);
    expect(connections).toBe(1);
    release();
    await expect(page.locator('[data-ose="session-join"]').first()).toBeVisible();
    expect(errors).toEqual([]);
  } finally {release();}
});

test('a lost connection can retry Campaign without reloading or losing the pilot identity',async({page,baseURL})=>{
  let socket:WebSocketRoute|undefined,connections=0;
  await page.routeWebSocket('**/ws',route=>{connections++;socket=route;route.connectToServer();});
  await page.goto(baseURL!);
  await expect(page.locator('[data-ose="menu"]')).toHaveAttribute('data-ose-connection','open');
  // An idle preconnection can close before the pilot has chosen a campaign.
  await socket!.close();
  await expect(page.locator('#menu-note')).toContainText('Select Campaign to reconnect');
  await page.locator('[data-ose="player-name"]').fill('reconnecting pilot');
  await page.locator('[data-ose="play"]').click();
  await expect(page.locator('[data-ose="session-join"]').first()).toBeVisible();
  const token=await page.evaluate(()=>localStorage.getItem('ose.token'));
  expect(token).toBeTruthy();
  await page.locator('[data-ose="session-join"]').first().click();
  await expect(page.locator('[data-ose="ksc"]')).toBeVisible();
  await socket!.close();
  await expect(page.locator('[data-ose="menu"]')).toHaveAttribute('data-ose-connection','disconnected');
  await page.locator('[data-ose="play"]').click();
  await expect(page.locator('[data-ose="menu"]')).toHaveAttribute('data-ose-connection','ready');
  await expect(page.locator('[data-ose="session-join"]').first()).toBeVisible();
  expect(await page.evaluate(()=>localStorage.getItem('ose.token'))).toBe(token);
  await page.locator('[data-ose="session-join"]').first().click();
  await expect(page.locator('[data-ose="ksc"]')).toBeVisible();
  expect(connections).toBe(3);
});
