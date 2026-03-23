import { expect } from '@wdio/globals';
import { loadFixture } from '../helpers/index.mjs';

async function search(browser, query, opts = {}) {
  const { target = 'both', caseSensitive = false, useRegex = false, exactMatch = false, path = '' } = opts;
  await browser.execute(async (q, t, cs, re, ex, p) => {
    await window.__jsonStore.getState().search(q, t, cs, re, ex, p);
  }, query, target, caseSensitive, useRegex, exactMatch, path);
  // Aspetta fine ricerca
  await browser.waitUntil(
    () => browser.execute(() => !window.__jsonStore.getState().searching),
    { timeout: 10000, timeoutMsg: `search("${query}") non terminata` }
  );
}

describe('Ricerca testuale', () => {
  before(async () => {
    await loadFixture(browser);
  });

  afterEach(async () => {
    // Reset ricerca tra i test
    await browser.execute(() => window.__jsonStore.getState().clearSearch());
  });

  it('trova "Alice" in values', async () => {
    await search(browser, 'Alice');
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBeGreaterThan(0);
  });

  it('ricerca case-insensitive: "alice" trova lo stesso di "Alice"', async () => {
    await search(browser, 'alice', { caseSensitive: false });
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBeGreaterThan(0);
  });

  it('ricerca case-sensitive: "alice" (minuscolo) non trova nulla', async () => {
    await search(browser, 'alice', { caseSensitive: true });
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    // "Alice" è con maiuscola nel JSON, "alice" case-sensitive non trova
    expect(count).toBe(0);
  });

  it('ricerca con exactMatch: "admin" trova solo il valore esatto', async () => {
    await search(browser, 'admin', { exactMatch: true });
    const results = await browser.execute(() =>
      window.__jsonStore.getState().searchResults
    );
    expect(results.length).toBeGreaterThan(0);
    // Tutti i risultati devono avere preview che corrisponde esattamente a "admin"
    for (const r of results) {
      expect(r.value_preview).toMatch(/admin/i);
    }
  });

  it('trova "Milano" (valore annidato in address)', async () => {
    await search(browser, 'Milano');
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBeGreaterThan(0);
  });

  it('cerca solo in keys: "role" trovato', async () => {
    await search(browser, 'role', { target: 'keys' });
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBeGreaterThan(0);
  });

  it('cerca solo in values: "role" non trovato (è una chiave)', async () => {
    await search(browser, 'role', { target: 'values' });
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    // "role" è una chiave, non un valore
    expect(count).toBe(0);
  });

  it('ricerca regex: email che finisce con .com', async () => {
    await search(browser, '.*\\.com$', { useRegex: true, target: 'values' });
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(3); // tre email .com
  });

  it('nessun risultato per termine inesistente', async () => {
    await search(browser, 'ZZZNOMATCH_XYZ_999');
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(0);
  });

  it('clearSearch azzera searchResults', async () => {
    await search(browser, 'Alice');
    await browser.execute(() => window.__jsonStore.getState().clearSearch());
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBe(0);
  });

  it('navigateToNode dopo ricerca imposta selectedNodeId', async () => {
    await search(browser, 'Alice');
    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      const firstResult = store.searchResults[0];
      if (firstResult) await store.navigateToNode(firstResult.node_id);
    });

    const selectedId = await browser.execute(() =>
      window.__jsonStore.getState().selectedNodeId
    );
    expect(selectedId).not.toBe(null);
  });

  it('digitare nel campo input avvia la ricerca', async () => {
    const input = await $('input[type="text"]');
    await input.setValue('gamma');
    await browser.waitUntil(
      () => browser.execute(() => window.__jsonStore.getState().searchResults.length > 0),
      { timeout: 5000, timeoutMsg: 'ricerca da input non completata' }
    );
    const count = await browser.execute(() =>
      window.__jsonStore.getState().searchResults.length
    );
    expect(count).toBeGreaterThan(0);
  });
});
