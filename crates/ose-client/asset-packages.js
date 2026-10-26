// Register early; Rust starts warming only after the renderer is available. Canvas-only clients keep
// loading their small UI normally. Registration/storage failures retain the ordinary asset loader.
(() => {
  let sequence = 0, worker, status = 'registering';
  let showDownloads, latestPacks = [];
  const captions = ['Gathering the parts','Engine section secured','Fins in position','Fuel tank attached',
    'Capsule bolted on','Solar panels fitted','Nose cone secured','All systems go'];
  function sceneryProgress() {
    const canvas = document.querySelector('[data-ose="gpu"]'), box = document.getElementById('scene-assembly');
    if (!canvas || !box) return;
    const state = canvas.getAttribute('data-ose-menu-scene');
    const known = canvas.hasAttribute('data-ose-menu-progress');
    const value = state === 'ready' ? 100 : known ? Math.max(0,Math.min(99,Number(canvas.getAttribute('data-ose-menu-progress')) || 0)) : 0;
    const stage = value === 100 ? 7 : Math.min(6,Math.floor(value*7/100));
    box.dataset.stage = String(stage);
    box.setAttribute('aria-hidden',String(state === 'ready' || state === 'static'));
    if (known || state === 'ready') {
      box.dataset.progress = String(value); box.setAttribute('aria-valuenow',String(value));
    } else {delete box.dataset.progress;box.removeAttribute('aria-valuenow');}
    box.setAttribute('aria-valuetext',captions[stage]);
    const caption=box.querySelector('.assembly-caption');if(caption)caption.textContent=captions[stage];
    box.querySelector('.assembly-fill')?.style.setProperty('transform',`scaleX(${value/100})`);
    for (const part of box.querySelectorAll('[data-assembly-step]')) part.classList.toggle('is-fitted',Number(part.dataset.assemblyStep)<=stage);
  }
  function groupName(group) {
    return ({ksc:'Space Center',vab:'Vehicle Assembly','materials-shared':'Shared textures',
      'flight-shared':'Flight systems','mission-control':'Mission Control'})[group]
      || (group.startsWith('planet-') ? `${group.slice(7)} scenery` : 'Destination assets');
  }
  function packProgress(packs) {
    latestPacks = packs;
    const box=document.getElementById('asset-downloads');if(!box)return;
    const groups=new Map();
    for (const pack of packs) {
      if (!pack || typeof pack.group!=='string' || ['core','menu'].includes(pack.group)) continue;
      if(!groups.has(pack.group))groups.set(pack.group,{fraction:0,count:0,ready:true,pending:false,loading:false});
      const group=groups.get(pack.group);group.count++;
      group.fraction+=Math.max(0,Math.min(1,Number(pack.fraction)||0));
      group.ready &&= pack.state==='ready';
      group.pending ||= ['queued','loading'].includes(pack.state);group.loading ||= pack.state==='loading';
    }
    const active=[...groups].find(([,g])=>g.loading) || [...groups].find(([,g])=>g.pending);
    if(!active){clearTimeout(showDownloads);showDownloads=undefined;box.hidden=true;return;}
    const ready=[...groups.values()].filter(g=>g.ready).length;
    const value=Math.min(99,Math.floor(100*[...groups.values()].reduce((sum,g)=>sum+g.fraction/g.count,0)/groups.size));
    box.setAttribute('aria-valuenow',String(value));
    box.setAttribute('aria-valuetext',`${groupName(active[0])}. ${ready} of ${groups.size} packs ready.`);
    const name=box.querySelector('.pack-name'),count=box.querySelector('.pack-count');
    if(name)name.textContent=groupName(active[0]);if(count)count.textContent=`${ready}/${groups.size} ready`;
    box.querySelector('.pack-fill')?.style.setProperty('transform',`scaleX(${value/100})`);
    if(box.hidden && !showDownloads)showDownloads=setTimeout(()=>{showDownloads=undefined;box.hidden=false;},250);
  }
  function observeScenery() {
    sceneryProgress(); packProgress(latestPacks);
    const canvas=document.querySelector('[data-ose="gpu"]');
    if(canvas)new MutationObserver(sceneryProgress).observe(canvas,{attributes:true,
      attributeFilter:['data-ose-menu-progress','data-ose-menu-scene']});
  }
  if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',observeScenery,{once:true});
  else observeScenery();
  if('serviceWorker' in navigator)navigator.serviceWorker.addEventListener('message',event=>{
    if(event.source && navigator.serviceWorker.controller && event.source!==navigator.serviceWorker.controller)return;
    if(event.data?.type==='ose-asset-progress' && Array.isArray(event.data.packs))packProgress(event.data.packs);
  });
  if('serviceWorker' in navigator)navigator.serviceWorker.addEventListener('controllerchange',()=>{
    const next=navigator.serviceWorker.controller;
    if(!next || next===worker)return;
    worker=next;
    message({type:'init'}).then(result=>{
      if(worker!==next || !result.supported)return;
      const usage=document.querySelector('[data-ose="gpu"]')?.getAttribute('data-ose-package-usage');
      if(usage)window.oseAssetPackages.prioritize(usage);
    }).catch(()=>{});
  });
  function message(data) {
    return new Promise((resolve, reject) => {
      const channel = new MessageChannel();
      channel.port1.onmessage = event => {channel.port1.close(); resolve(event.data);};
      channel.port1.onmessageerror = reject;
      channel.port1.start();
      worker.postMessage(data,[channel.port2]);
    });
  }
  const ready = (async () => {
    if (!('serviceWorker' in navigator) || !isSecureContext) {status='fallback';return false;}
    const registration = await navigator.serviceWorker.register('/asset-worker.js', {scope:'/'});
    status = 'claiming';
    const active = registration.active || (await navigator.serviceWorker.ready).active;
    worker = navigator.serviceWorker.controller || active;
    const controlled = navigator.serviceWorker.controller ? Promise.resolve() : new Promise(resolve => {
      navigator.serviceWorker.addEventListener('controllerchange', resolve, {once:true});
    });
    status = 'catalog';
    const supported = (await message({type:'init'})).supported;
    await controlled;
    status = supported ? 'ready' : 'fallback';
    return supported;
  })().catch(() => {status = 'fallback';return false;});
  window.oseAssetPackages = {ready, get status() {return status;}, async prioritize(usage) {
    const ticket = ++sequence;
    if (!await ready) return;
    const canvas = document.querySelector('[data-ose="gpu"]');
    canvas?.setAttribute('data-ose-package-usage',usage);
    canvas?.setAttribute('data-ose-preload','loading');
    try {
      const result = await message({type:'usage',usage});
      if (ticket === sequence) canvas?.setAttribute('data-ose-preload',result.ready ? 'ready' : 'partial');
    } catch {
      if (ticket === sequence) canvas?.setAttribute('data-ose-preload','partial');
    }
  }};
})();
