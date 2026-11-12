'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const root = path.join(__dirname, '..', 'static');
const html = fs.readFileSync(path.join(root, 'index.html'), 'utf8');
const code = ['translations.js', 'app.js'].map(file => fs.readFileSync(path.join(root, file), 'utf8')).join('\n');

async function visit(languages, release, {lang = 'en', pathname = '/', stored = null} = {}) {
  const elements = [...html.matchAll(/data-i18n="([^"]+)"/g)].map(([, key]) => ({dataset: {i18n: key}}));
  const attributes = [...html.matchAll(/data-i18n-attr="([^"]+)"/g)].map(([, attr]) => ({dataset: {i18nAttr: attr}, setAttribute(name, value) { this[name] = value; }}));
  const link = () => ({replaceChildren(node) { this.text = node.textContent; }, append(node) { this.text += node.textContent; }});
  const links = Object.fromEntries(['windows', 'macos', 'linux', 'portable', 'android'].map(platform => [platform, link()]));
  const switcher = [{dataset: {language: 'de'}, addEventListener(type, handler) { this[type] = handler; }}];
  const primary = {};
  const status = {};
  const storage = new Map(stored ? [['fmx-language', stored]] : []);
  const location = {pathname, hash: '', replace(url) { this.redirected = url; }};
  const document = {
    documentElement: {lang},
    querySelectorAll(selector) {
      if (selector === '[data-i18n]') return elements;
      if (selector === '[data-i18n-attr]') return attributes;
      if (selector === '[data-language]') return switcher;
      const platform = selector.match(/data-platform="([^"]+)"/)?.[1];
      return platform ? [links[platform]] : [];
    },
    querySelector(selector) { return selector === '#release-status' ? status : primary; },
    createTextNode(textContent) { return {textContent}; },
    createElement() { return {}; },
  };
  const localStorage = {getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value)};
  const context = vm.createContext({document, location, localStorage, navigator: {languages, language: languages[0], userAgent: 'Windows'}, AbortSignal, fetch: async () => {
    if (!release) throw new Error('offline');
    return {ok: true, json: async () => release};
  }});
  vm.runInContext(code, context);
  await new Promise(resolve => setImmediate(resolve));
  return {document, elements, attributes, links, primary, status, context, location, storage, switcher};
}

test('every page key and release message is translated for each supported language', async () => {
  const page = await visit(['en'], null);
  const catalogs = vm.runInContext('translations', page.context);
  const keys = new Set([...page.elements.map(element => element.dataset.i18n), ...page.attributes.map(element => element.dataset.i18nAttr.split(':')[0]), 'downloadFor', 'downloadPortable', 'downloadAndroid', 'releaseServed', 'releaseFallback', 'latestRelease']);
  for (const language of ['es', 'fr', 'de', 'he', 'ru']) {
    for (const key of keys) assert.ok(catalogs[language][key]?.trim(), `${language}: ${key}`);
    assert.ok(catalogs[language].downloadFor.includes('{platform}'));
  }
});

test('home page sends first-time visitors to their preferred language once', async () => {
  for (const [languages, expected] of [[['fr-CA'], '/fr/'], [['es-MX'], '/es/'], [['de-DE'], '/de/'], [['ru-RU'], '/ru/'], [['iw-IL'], '/he/'], [['ja', 'fr'], '/fr/'], [['en-GB', 'fr'], undefined], [['ja'], undefined]]) {
    const page = await visit(languages, null);
    assert.equal(page.location.redirected, expected, languages.join());
  }
  assert.equal((await visit(['fr-FR'], null, {stored: 'en'})).location.redirected, undefined);
  assert.equal((await visit(['fr-FR'], null, {lang: 'de', pathname: '/de/'})).location.redirected, undefined);
  const page = await visit(['en'], null);
  page.switcher[0].click();
  assert.equal(page.storage.get('fmx-language'), 'de');
});

test('uses the server-rendered page language for release messages', async () => {
  const page = await visit(['en'], null, {lang: 'he', pathname: '/he/'});
  assert.equal(page.status.textContent, vm.runInContext('translations.he.releaseFallback', page.context));
});

test('fonts are self-hosted', () => {
  const css = fs.readFileSync(path.join(root, 'style.css'), 'utf8');
  assert.doesNotMatch(css, /@import|https?:\/\//);
  for (const [, file] of css.matchAll(/url\(\/assets\/fonts\/([^)]+)\)/g)) assert.ok(fs.existsSync(path.join(root, 'assets', 'fonts', file)), file);
});

test('every language has a server-rendered page with hreflang alternates', () => {
  const dist = path.join(__dirname, '..', 'dist');
  const catalogs = vm.runInContext(`${fs.readFileSync(path.join(root, 'translations.js'), 'utf8')}; translations`, vm.createContext({}));
  const codes = ['en', 'es', 'fr', 'de', 'he', 'ru'];
  const sitemap = fs.readFileSync(path.join(dist, 'sitemap.xml'), 'utf8');
  for (const code of codes) {
    const url = `https://fmx-dun.vercel.app/${code === 'en' ? '' : `${code}/`}`;
    const page = fs.readFileSync(path.join(dist, code === 'en' ? '' : code, 'index.html'), 'utf8');
    assert.match(page, new RegExp(`<html lang="${code}"`));
    assert.ok(page.includes(`<link rel="canonical" href="${url}">`), code);
    assert.ok(sitemap.includes(`<loc>${url}</loc>`), code);
    for (const other of codes) assert.ok(page.includes(`hreflang="${other}"`), `${code} -> ${other}`);
    assert.ok(page.includes('hreflang="x-default" href="https://fmx-dun.vercel.app/"'));
    assert.ok(!page.includes('<!--'), `${code}: unreplaced marker`);
    assert.doesNotMatch(page, /googleapis|gstatic|googletagmanager|google-analytics/, `${code}: third-party Google resource`);
    if (code !== 'en') {
      assert.ok(page.includes(`dir="${code === 'he' ? 'rtl' : 'ltr'}"`));
      assert.ok(page.includes(catalogs[code].heroTitle), `${code}: heading`);
      assert.ok(page.includes(`aria-current="page">`), `${code}: switcher`);
    }
  }
});

test('localizes release links and detected download without trusting asset names', async () => {
  const url = 'https://github.com/sudo-py-dev/fmx/releases/download/v1/fmx-windows-setup.exe';
  const page = await visit(['fr-FR'], {tag_name: 'v1', assets: [{name: 'fmx-windows-setup.exe', browser_download_url: url}, {name: 'fmx-linux-amd64.deb', browser_download_url: 'https://example.com/fake'}]}, {lang: 'fr', pathname: '/fr/'});
  assert.equal(page.links.windows.href, url);
  assert.match(page.links.windows.text, /Télécharger pour Windows/);
  assert.match(page.primary.textContent, /Télécharger pour Windows/);
  assert.equal(page.links.linux.href, undefined);
  assert.match(page.status.textContent, /v1 · Téléchargements/);
});
