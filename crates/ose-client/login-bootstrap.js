// Accept the first sign-in click even while the game client is downloading.
(() => {
  const button = document.getElementById('login');
  const status = document.getElementById('login-status');
  const reload = document.getElementById('login-reload');
  let ready = false, pending = false;
  status.textContent = 'Loading sign-in…';
  button.addEventListener('click', event => {
    if (ready) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    pending = true;
    status.textContent = 'Loading sign-in… Your request will continue when ready.';
  }, true);
  reload.addEventListener('click', () => location.reload());
  const timeout = setTimeout(() => {
    if (ready) return;
    status.textContent = 'Sign-in could not finish loading. Check your connection and reload to try again.';
    status.classList.add('error');
    reload.hidden = false;
  }, 30000);
  window.addEventListener('TrunkApplicationStarted', () => {
    ready = true;
    clearTimeout(timeout);
    reload.hidden = true;
    status.textContent = '';
    status.classList.remove('error');
    if (pending) button.click();
  }, {once: true});
})();
