import { chromium, expect } from '@playwright/test';
import assert from 'node:assert/strict';
import net from 'node:net';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const mails=[];
const smtp=net.createServer(socket=>{
  socket.write('220 test SMTP\r\n'); let buffer='', data=null;
  socket.on('data',chunk=>{buffer+=chunk.toString();let i;while((i=buffer.indexOf('\r\n'))>=0){const line=buffer.slice(0,i);buffer=buffer.slice(i+2);
    if(data!==null){if(line==='.') {mails.push({text:data,received:Date.now()});data=null;socket.write('250 queued\r\n');}else data+=line+'\n';}
    else if(line==='DATA'){data='';socket.write('354 send data\r\n');}
    else if(line==='QUIT'){socket.end('221 bye\r\n');}
    else socket.write('250 test\r\n');
  }});
});
await new Promise(resolve=>smtp.listen(0,'127.0.0.1',resolve));
const reserve=net.createServer();await new Promise(resolve=>reserve.listen(0,'127.0.0.1',resolve));const port=reserve.address().port;await new Promise(resolve=>reserve.close(resolve));
const dir=await fs.mkdtemp(path.join(os.tmpdir(),'ose-otp-browser-'));
const exe=path.join(root,'.cache/otp-preview.exe');await fs.copyFile(path.join(root,'target/debug/ose-server.exe'),exe);
const server=spawn(exe,['--allow-guest-accounts','--listen',`127.0.0.1:${port}`,'--db',path.join(dir,'accounts.json'),'--save',path.join(dir,'save.json'),'--static-dir',path.join(root,'target/otp-web')],{
 env:{...process.env,OSE_SMTP_HOST:'127.0.0.1',OSE_SMTP_PORT:String(smtp.address().port),OSE_SMTP_FROM:'OpenSpaceExplorer <ose@example.com>'},stdio:'ignore',windowsHide:true});
const base=`http://127.0.0.1:${port}`;
for(let i=0;i<100;i++){try{if((await fetch(base+'/health')).ok)break;}catch{}await new Promise(resolve=>setTimeout(resolve,100));}
const browser=await chromium.launch({headless:true,args:['--disable-gpu']});const contexts=[];
async function page(token){const c=await browser.newContext({viewport:{width:1440,height:1000}});contexts.push(c);if(token)await c.addInitScript(t=>localStorage.setItem('ose.token',t),token);const p=await c.newPage();await p.goto(base);return p;}
async function readCode(email, since){await expect.poll(()=>mails.slice(since).find(m=>m.text.includes(email))).toBeTruthy();const mail=mails.slice(since).find(m=>m.text.includes(email));return mail.text.match(/code is: (\d{6})/)[1];}
async function signIn(email){const p=await page();await p.locator('#screen-login.open').waitFor();await p.locator('#login-email').fill(email);const since=mails.length;await p.locator('#login').click();const code=await readCode(email,since);await expect(p.locator('#menu-note')).toContainText('Code sent');await p.locator('#login-otp').fill(code);await p.locator('#login').click();await p.locator('#screen-name.open').waitFor();await p.locator('#play').click();await p.locator('#screen-sessions.open').waitFor();assert.equal(await p.locator('#login-code').count(),0);return p;}
async function create(p,name){await p.locator('#create-details > summary').click();await p.locator('#create-name').fill(name);await p.locator('#create-mode').selectOption('Sandbox');await p.locator('#create').click();await p.locator('#screen-ksc.open').waitFor();}
// A raw protocol connection creates a legacy identity for this migration test only.
async function legacyToken(){const ws=new WebSocket(base.replace('http:','ws:')+'/ws');await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject;});ws.binaryType='arraybuffer';const name=new TextEncoder().encode('Existing pilot');const packet=Uint8Array.from([4,68,0,name.length,...name]);const reply=new Promise(resolve=>ws.onmessage=e=>resolve(new Uint8Array(e.data)));ws.send(packet);const bytes=await reply;let i=0;function v(){let n=0,shift=0,b;do{b=bytes[i++];n|=(b&127)<<shift;shift+=7;}while(b&128);return n;}assert.equal(v(),5);v();const length=v();const token=new TextDecoder().decode(bytes.slice(i,i+length));ws.close();return token;}
try{
 const oldToken=await legacyToken();const legacy=await page(oldToken);await legacy.locator('#play').click();await legacy.locator('#screen-sessions.open').waitFor();await create(legacy,'Existing campaign');await legacy.locator('#ksc-sessions').click();await legacy.locator('#account-details').evaluate(el=>el.open=true);await legacy.locator('#account-email').fill('legacy@example.com');let since=mails.length;await legacy.locator('#account-send-otp').click();const oldCode=await readCode('legacy@example.com',since);await expect(legacy.locator('#menu-note')).toContainText('Code sent');await legacy.locator('#account-otp').fill(oldCode);await legacy.locator('#account-verify-otp').click();await expect(legacy.locator('#account-status')).toContainText('Verified: legacy@example.com');const legacyIdentity=await legacy.locator('#player').textContent();console.log('PASS: verified email keeps existing account and campaign.');
 const owner=await signIn('pilot@example.com');const identity=await owner.locator('#player').textContent();await create(owner,'OTP browser game');await owner.locator('body').click({position:{x:5,y:5}});await owner.keyboard.press('Escape');if(!(await owner.locator('#screen-paused.open').isVisible()))await owner.keyboard.press('Escape');await owner.locator('#share-current').click();await expect.poll(()=>owner.locator('#paused-share-code').inputValue()).toHaveLength(32);const share=await owner.locator('#paused-share-code').inputValue();
 const friend=await signIn('friend@example.com');assert.equal(await friend.locator('[data-ose="session"]').count(),0);await friend.locator('#join-code').fill(share);await friend.locator('#join-code-submit').click();await friend.locator('[data-ose="session"]').waitFor();console.log('PASS: email signup and game sharing.');
 const last=mails.filter(m=>m.text.includes('pilot@example.com')||m.text.includes('legacy@example.com')).at(-1).received;const delay=Math.max(0,last+61000-Date.now());if(delay){console.log('Waiting for the real resend cooldown before testing another machine.');await new Promise(resolve=>setTimeout(resolve,delay));}
 const other=await signIn('pilot@example.com');await other.locator('[data-ose="session"]').waitFor();assert.equal(await other.locator('#player').textContent(),identity);assert.match(await other.locator('#sessions').textContent(),/OTP browser game/);
 const recovered=await signIn('legacy@example.com');await recovered.locator('[data-ose="session"]').waitFor();assert.equal(await recovered.locator('#player').textContent(),legacyIdentity);assert.match(await recovered.locator('#sessions').textContent(),/Existing campaign/);
 await other.screenshot({path:path.join(root,'.cache/email-otp-campaigns.png')});await other.locator('#account-details').evaluate(el=>el.open=true);await other.screenshot({path:path.join(root,'.cache/email-otp-account.png')});console.log('PASS: new-machine email OTP restores both new and existing accounts and games.');
}catch(e){for(let i=0;i<contexts.length;i++){const p=contexts[i].pages()[0];if(p){console.log('UI state',i,await p.locator('#menu-note').textContent({timeout:1000}).catch(()=>null));await p.screenshot({path:path.join(root,`.cache/otp-failure-${i}.png`)});}}throw e;}
finally{await browser.close();server.kill();await new Promise(resolve=>server.once('exit',resolve));await new Promise(resolve=>smtp.close(resolve));for(const name of ['accounts.json','save.json','save.json.tmp'])await fs.rm(path.join(dir,name),{force:true});await fs.rmdir(dir).catch(()=>{});}
