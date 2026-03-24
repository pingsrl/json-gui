import { expect } from '@wdio/globals';
import { loadFixture } from '../helpers/index.mjs';

async function searchObjects(browser, filters, opts = {}) {
  const { keyCaseSensitive = false, valueCaseSensitive = false, path = '' } = opts;
  await browser.execute(async (f, kcs, vcs, p) => {
    await window.__jsonStore.getState().searchObjects(f, kcs, vcs, p);
  }, filters, keyCaseSensitive, valueCaseSensitive, path);
  await browser.waitUntil(
    () => browser.execute(() => !window.__jsonStore.getState().searching),
    { timeout: 10000, timeoutMsg: 'searchObjects non terminata' }
  );
}

describe('Ricerca per oggetti', () => {
  before(async () => {
    await loadFixture(browser);
  });

  afterEach(async () => {
    await browser.execute(() => window.__jsonStore.getState().clearSearch());
  });

  it('operator "equals": role = admin → 1 risultato', async () => {
    await searchObjects(browser, [
      { path: 'role', operator: 'equals', value: 'admin' },
    ]);
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(1);
  });

  it('operator "equals": role = user → 2 risultati', async () => {
    await searchObjects(browser, [
      { path: 'role', operator: 'equals', value: 'user' },
    ]);
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(2);
  });

  it('operator "contains": email contiene "example.com" → 3 risultati', async () => {
    await searchObjects(browser, [
      { path: 'email', operator: 'contains', value: 'example.com' },
    ]);
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(3);
  });

  it('filtri multipli (AND): role=user AND active=true → 1 risultato (Charlie)', async () => {
    await searchObjects(browser, [
      { path: 'role', operator: 'equals', value: 'user' },
      { path: 'active', operator: 'equals', value: 'true' },
    ]);
    const results = await browser.execute(() =>
      window.__jsonStore.getState().searchResults
    );
    expect(results.length).toBe(1);
  });

  it('operator "exists": address.city esiste → 3 risultati', async () => {
    await searchObjects(browser, [
      { path: 'address.city', operator: 'exists' },
    ]);
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(3);
  });

  it('operator "exists": campoinesistente non esiste → 0 risultati', async () => {
    await searchObjects(browser, [
      { path: 'campoinesistente', operator: 'exists' },
    ]);
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(0);
  });

  it('operator "contains": city contiene "o" → almeno 2 (Torino, Roma)', async () => {
    await searchObjects(browser, [
      { path: 'address.city', operator: 'contains', value: 'o' },
    ], { valueCaseSensitive: false });
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBeGreaterThanOrEqual(2);
  });

  it('filtri con path di scope: cerca solo in $.users', async () => {
    await searchObjects(browser, [
      { path: 'role', operator: 'exists' },
    ], { path: '$.users' });
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    // Solo gli elementi di users hanno "role"
    expect(count).toBe(3);
  });

  it('navigateToNode da risultato object search', async () => {
    await searchObjects(browser, [
      { path: 'role', operator: 'equals', value: 'admin' },
    ]);
    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      if (store.searchResults.length > 0) {
        await store.navigateToNode(store.searchResults[0].node_id);
      }
    });
    const selectedId = await browser.execute(() =>
      window.__jsonStore.getState().selectedNodeId
    );
    expect(selectedId).not.toBe(null);
  });

  it('searchMode "object" è impostato dopo searchObjects', async () => {
    await searchObjects(browser, [
      { path: 'role', operator: 'equals', value: 'admin' },
    ]);
    const mode = await browser.execute(() =>
      window.__jsonStore.getState().activeSearchMode
    );
    expect(mode).toBe('object');
  });
});
