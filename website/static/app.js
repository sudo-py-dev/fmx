'use strict';
const preferredLanguages = navigator.languages?.length ? navigator.languages : [navigator.language || 'en'];
const language = preferredLanguages.map(value => {
  const code = value.toLowerCase().split(/[-_]/)[0];
  return code === 'iw' ? 'he' : code;
}).find(code => Object.hasOwn(translations, code)) || 'en';
const messages = translations[language];
const markupKeys = new Set(['heroTitle', 'mediaTypes', 'featureTitle', 'featureIntro', 'faqTitle']);
function translatePage() {
  document.documentElement.lang = language;
  document.documentElement.dir = language === 'he' ? 'rtl' : 'ltr';
  document.querySelectorAll('[data-i18n]').forEach(element => {
    const key = element.dataset.i18n;
    if (!Object.hasOwn(messages, key)) return;
    if (markupKeys.has(key)) element.innerHTML = messages[key];
    else element.textContent = messages[key];
  });
  document.querySelectorAll('[data-i18n-attr]').forEach(element => {
    const [key, attribute] = element.dataset.i18nAttr.split(':');
    if (Object.hasOwn(messages, key)) element.setAttribute(attribute, messages[key]);
  });
}
translatePage();
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
