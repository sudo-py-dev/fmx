'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const root = path.join(__dirname, '..', 'static');
const html = fs.readFileSync(path.join(root, 'index.html'), 'utf8');
const code = ['translations.js', 'app.js'].map(file => fs.readFileSync(path.join(root, file), 'utf8')).join('\n');

async function visit(languages, release) {
  const elements = [...html.matchAll(/data-i18n="([^"]+)"/g)].map(([, key]) => ({dataset: {i18n: key}}));
  const attributes = [...html.matchAll(/data-i18n-attr="([^"]+)"/g)].map(([, attr]) => ({dataset: {i18nAttr: attr}, setAttribute(name, value) { this[name] = value; }}));
  const link = () => ({replaceChildren(node) { this.text = node.textContent; }, append(node) { this.text += node.textContent; }});
  const links = Object.fromEntries(['windows', 'macos', 'linux', 'portable', 'android'].map(platform => [platform, link()]));
  const primary = {};
  const status = {};
  const document = {
    documentElement: {},
    querySelectorAll(selector) {
      if (selector === '[data-i18n]') return elements;
      if (selector === '[data-i18n-attr]') return attributes;
      const platform = selector.match(/data-platform="([^"]+)"/)?.[1];
      return platform ? [links[platform]] : [];
    },
    querySelector(selector) { return selector === '#release-status' ? status : primary; },
    createTextNode(textContent) { return {textContent}; },
    createElement() { return {}; },
  };
  const context = vm.createContext({document, navigator: {languages, language: languages[0], userAgent: 'Windows'}, AbortSignal, fetch: async () => {
    if (!release) throw new Error('offline');
    return {ok: true, json: async () => release};
  }});
  vm.runInContext(code, context);
  await new Promise(resolve => setImmediate(resolve));
  return {document, elements, attributes, links, primary, status, context};
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

test('detects preferred languages, regional variants, Hebrew direction, and fallback', async () => {
  for (const [languages, expected] of [[['fr-CA'], 'fr'], [['es-MX'], 'es'], [['de-DE'], 'de'], [['ru-RU'], 'ru'], [['iw-IL'], 'he'], [['ja', 'fr'], 'fr'], [['en-GB', 'fr'], 'en'], [['ja'], 'en']]) {
    const page = await visit(languages, null);
    assert.equal(page.document.documentElement.lang, expected);
    assert.equal(page.document.documentElement.dir, expected === 'he' ? 'rtl' : 'ltr');
    if (expected !== 'en') {
      assert.ok(page.elements.find(element => element.dataset.i18n === 'intro').textContent);
      assert.ok(page.attributes.find(element => element.dataset.i18nAttr === 'imageAlt:alt').alt);
    }
    assert.equal(page.status.textContent, vm.runInContext('messages.releaseFallback', page.context));
  }
});

test('localizes release links and detected download without trusting asset names', async () => {
  const url = 'https://github.com/sudo-py-dev/fmx/releases/download/v1/fmx-windows-setup.exe';
  const page = await visit(['fr-FR'], {tag_name: 'v1', assets: [{name: 'fmx-windows-setup.exe', browser_download_url: url}, {name: 'fmx-linux-amd64.deb', browser_download_url: 'https://example.com/fake'}]});
  assert.equal(page.links.windows.href, url);
  assert.match(page.links.windows.text, /Télécharger pour Windows/);
  assert.match(page.primary.textContent, /Télécharger pour Windows/);
  assert.equal(page.links.linux.href, undefined);
  assert.match(page.status.textContent, /v1 · Téléchargements/);
});
