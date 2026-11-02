'use strict';
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
        link.replaceChildren(document.createTextNode(platform === 'portable' ? 'Download portable archive ' : `Download ${platform === 'android' ? 'Android APK' : platform === 'macos' ? 'for macOS' : `for ${platform === 'windows' ? 'Windows' : 'Linux'}`} `));
        const arrow = document.createElement('span'); arrow.textContent = '↓'; link.append(arrow);
      });
    }
    const detected = /Android/i.test(navigator.userAgent) ? 'android' : /Windows/i.test(navigator.userAgent) ? 'windows' : /Macintosh|MacIntel/i.test(navigator.userAgent) ? 'macos' : /Linux/i.test(navigator.userAgent) ? 'linux' : null;
    if (detected && available[detected]) {
      const primary = document.querySelector('#smart-download');
      primary.href = available[detected];
      primary.textContent = `Download for ${ {windows:'Windows',macos:'macOS',linux:'Linux',android:'Android'}[detected]} ↓`;
    }
    status.textContent = `${release.tag_name || 'Latest release'} · Downloads served directly from GitHub`;
  } catch {
    status.textContent = 'Browse the latest GitHub release to choose your download.';
  }
}
loadRelease();
