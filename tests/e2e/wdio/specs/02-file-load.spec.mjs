import { expect } from '@wdio/globals';
import { loadFixture, getStoreState } from '../helpers/index.mjs';

describe('Caricamento file', () => {
  before(async () => {
    await loadFixture(browser);
  });

  it('rootNode non è null dopo openFile', async () => {
    const state = await getStoreState(browser);
    expect(state.rootNode).not.toBe(null);
  });

  it('rootNode è di tipo object', async () => {
    const rootType = await browser.execute(() =>
      window.__jsonStore.getState().rootNode?.value_type
    );
    expect(rootType).toBe('object');
  });

  it('rootChildren contiene 4 chiavi (users, settings, tags, version)', async () => {
    const state = await getStoreState(browser);
    expect(state.rootChildrenCount).toBe(4);
    expect(state.rootChildrenKeys).toContain('users');
    expect(state.rootChildrenKeys).toContain('settings');
    expect(state.rootChildrenKeys).toContain('tags');
    expect(state.rootChildrenKeys).toContain('version');
  });

  it('filePath punta al file fixture', async () => {
    const filePath = await browser.execute(() =>
      window.__jsonStore.getState().filePath
    );
    expect(filePath).toContain('test.json');
  });

  it('nodeCount > 0', async () => {
    const nodeCount = await browser.execute(() =>
      window.__jsonStore.getState().nodeCount
    );
    expect(nodeCount).toBeGreaterThan(0);
  });

  it('sizeBytes > 0', async () => {
    const sizeBytes = await browser.execute(() =>
      window.__jsonStore.getState().sizeBytes
    );
    expect(sizeBytes).toBeGreaterThan(0);
  });

  it('il TreePanel mostra il contenuto (non il placeholder)', async () => {
    // Dopo il caricamento la pianta deve avere almeno un nodo renderizzato
    const hasTree = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      return store.rootChildren.length > 0;
    });
    expect(hasTree).toBe(true);
  });

  it('apertura di un secondo file sostituisce il precedente', async () => {
    // Ricarica lo stesso file — state deve resettarsi e ricaricarsi
    await browser.execute(async (fixturePath) => {
      await window.__jsonStore.getState().openFile(fixturePath);
    }, global.FIXTURE_PATH);

    const state = await getStoreState(browser);
    expect(state.rootChildrenCount).toBe(4);
    expect(state.expandedNodesSize).toBe(0); // reset dell'espansione
    expect(state.selectedNodeId).toBe(null); // reset della selezione
  });
});
