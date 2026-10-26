// Register early; Rust starts warming only after the renderer is available. Canvas-only clients keep
// loading their small UI normally. Registration/storage failures retain the ordinary asset loader.
(() => {
  let sequence = 0, worker, status = 'registering';
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
    if (!('serviceWorker' in navigator) || !isSecureContext) return false;
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
