import { pausable, reduced } from './assets/ui/ui.js';

const host = document.querySelector('.hero__window[data-ui-pause-host]');
if (host && !reduced()) {
  const control = {
    pause: () => host.classList.add('is-ui-paused'),
    resume: () => host.classList.remove('is-ui-paused'),
  };
  pausable(host, control);
}
