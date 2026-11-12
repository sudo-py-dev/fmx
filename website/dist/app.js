'use strict';
// Each language has its own server-rendered page; the page's lang attribute is the source of truth.
const language = Object.hasOwn(translations, document.documentElement.lang) ? document.documentElement.lang : 'en';
const messages = translations[language];
const choiceKey = 'fmx-language';
const redirectedKey = 'fmx-language-redirected';
function readStorage(key) {
  try { return localStorage.getItem(key); } catch { return null; }
}
function writeStorage(key, value) {
  try { localStorage.setItem(key, value); } catch { /* Storage can be unavailable; the links still work. */ }
}
function preferredLanguage() {
  const preferred = navigator.languages?.length ? navigator.languages : [navigator.language || 'en'];
  return preferred.map(value => {
    const code = value.toLowerCase().split(/[-_]/)[0];
    return code === 'iw' ? 'he' : code;
  }).find(code => Object.hasOwn(translations, code)) || 'en';
}
document.querySelectorAll('[data-language]').forEach(link => link.addEventListener('click', () => writeStorage(choiceKey, link.dataset.language)));
// Send first-time visitors of the English home page to their language once; an explicit choice always wins.
// The automatic redirect is recorded separately so returning to / later keeps the English page.
const redirectTo = location.pathname === '/' && language === 'en' && !readStorage(choiceKey) && !readStorage(redirectedKey) && preferredLanguage();
if (redirectTo && redirectTo !== 'en') {
  writeStorage(redirectedKey, redirectTo);
  location.replace(`/${redirectTo}/${location.hash}`);
}
const repository = 'sudo-py-dev/fmx';
const patterns = {
  windows: /windows.*setup\.exe$/i,
  macos: /macos.*\.dmg$/i,
  linux: /amd64\.deb$/i,
  portable: /linux.*x86_64.*\.tar\.gz$/i,
  android: /\.apk$/i,
};
async function loadRelease() {
  const status = document.querySelector('#release-status');
  try {
    const response = await fetch(`https://api.github.com/repos/${repository}/releases/latest`, {signal: AbortSignal.timeout(7000)});
    if (!response.ok) throw new Error('Release unavailable');
    const release = await response.json();
    if (!Array.isArray(release.assets)) throw new Error('Invalid release');
    const available = {};
    for (const [platform, pattern] of Object.entries(patterns)) {
      const asset = release.assets.find(item => pattern.test(item.name) && item.browser_download_url?.startsWith(`https://github.com/${repository}/releases/download/`));
      if (!asset) continue;
      available[platform] = asset.browser_download_url;
      document.querySelectorAll(`[data-platform="${platform}"]`).forEach(link => {
        link.href = asset.browser_download_url;
        const label = platform === 'portable' ? messages.downloadPortable : platform === 'android' ? messages.downloadAndroid : messages.downloadFor.replace('{platform}', {windows:'Windows', macos:'macOS', linux:'Linux'}[platform]);
        link.replaceChildren(document.createTextNode(`${label} `));
        const arrow = document.createElement('span'); arrow.textContent = '↓'; link.append(arrow);
      });
    }
    const detected = /Android/i.test(navigator.userAgent) ? 'android' : /Windows/i.test(navigator.userAgent) ? 'windows' : /Macintosh|MacIntel/i.test(navigator.userAgent) ? 'macos' : /Linux/i.test(navigator.userAgent) ? 'linux' : null;
    if (detected && available[detected]) {
      const primary = document.querySelector('#smart-download');
      primary.href = available[detected];
      primary.textContent = `${messages.downloadFor.replace('{platform}', {windows:'Windows',macos:'macOS',linux:'Linux',android:'Android'}[detected])} ↓`;
    }
    status.textContent = `${release.tag_name || messages.latestRelease} · ${messages.releaseServed}`;
  } catch {
    status.textContent = messages.releaseFallback;
  }
}
loadRelease();
