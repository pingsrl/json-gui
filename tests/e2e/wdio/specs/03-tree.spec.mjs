import { expect } from '@wdio/globals';
import { loadFixture, getStoreState } from '../helpers/index.mjs';

async function waitForExpandedNode(nodeKey, timeout = 5000) {
  await browser.waitUntil(
    () => browser.execute((key) => {
      const store = window.__jsonStore.getState();
      const node = store.rootChildren.find((child) => child.key === key);
      return Boolean(node) && store.expandedNodes.has(node.id);
    }, nodeKey),
    { timeout, timeoutMsg: `nodo ${nodeKey} non espanso` }
  );
}

describe('Interazioni con l\'albero', () => {
  before(async () => {
    await loadFixture(browser);
  });

  it('espande un nodo tramite toggleNode', async () => {
    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      const usersNode = store.rootChildren.find(c => c.key === 'users');
      if (usersNode) await store.toggleNode(usersNode.id);
    });
    await waitForExpandedNode('users');

    const expanded = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      const usersNode = store.rootChildren.find(c => c.key === 'users');
      return usersNode ? store.expandedNodes.has(usersNode.id) : false;
    });
    expect(expanded).toBe(true);
  });

  it('i figli di "users" sono 3', async () => {
    const childrenCount = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      const usersNode = store.rootChildren.find(c => c.key === 'users');
      if (!usersNode) return 0;
      return (store.expandedNodes.get(usersNode.id) ?? []).length;
    });
    expect(childrenCount).toBe(3);
  });

  it('collassa un nodo tramite toggleNode', async () => {
    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      const usersNode = store.rootChildren.find(c => c.key === 'users');
      if (usersNode) await store.toggleNode(usersNode.id); // toggle chiude
    });

    const expanded = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      const usersNode = store.rootChildren.find(c => c.key === 'users');
      return usersNode ? store.expandedNodes.has(usersNode.id) : true;
    });
    expect(expanded).toBe(false);
  });

  it('expandAll espande tutti i nodi', async () => {
    await browser.execute(async () => {
      await window.__jsonStore.getState().expandAll();
    });
    await browser.waitUntil(
      () => browser.execute(() => window.__jsonStore.getState().expandedNodes.size >= 4),
      { timeout: 15000, timeoutMsg: 'expandAll non ha popolato expandedNodes' }
    );

    const state = await getStoreState(browser);
    expect(state.expandedNodesSize).toBeGreaterThanOrEqual(4);
  });

  it('collapseAll riporta a zero nodi espansi', async () => {
    await browser.execute(() => {
      window.__jsonStore.getState().collapseAll();
    });

    const state = await getStoreState(browser);
    expect(state.expandedNodesSize).toBe(0);
  });

  it('expandSubtree espande solo il sottoalbero del nodo', async () => {
    const usersNodeId = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      return store.rootChildren.find(c => c.key === 'users')?.id;
    });
    expect(usersNodeId).toBeDefined();

    await browser.execute(async (nodeId) => {
      await window.__jsonStore.getState().expandSubtree(nodeId);
    }, usersNodeId);
    await browser.waitUntil(
      () => browser.execute((id) => {
        const store = window.__jsonStore.getState();
        return store.expandedNodes.has(id);
      }, usersNodeId),
      { timeout: 5000, timeoutMsg: 'expandSubtree non ha espanso il nodo target' }
    );

    const expanded = await browser.execute((nodeId) => {
      return window.__jsonStore.getState().expandedNodes.has(nodeId);
    }, usersNodeId);
    expect(expanded).toBe(true);

    // Il nodo settings non deve essere espanso
    const settingsExpanded = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      const settingsNode = store.rootChildren.find(c => c.key === 'settings');
      return settingsNode ? store.expandedNodes.has(settingsNode.id) : true;
    });
    expect(settingsExpanded).toBe(false);
  });

  it('selectNode imposta selectedNodeId', async () => {
    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      const settingsNode = store.rootChildren.find(c => c.key === 'settings');
      if (settingsNode) await store.selectNode(settingsNode);
    });

    const selectedId = await browser.execute(() =>
      window.__jsonStore.getState().selectedNodeId
    );
    expect(selectedId).not.toBe(null);
  });

  it('il nodo selezionato ha path corretto', async () => {
    const path = await browser.execute(() =>
      window.__jsonStore.getState().selectedNodePath
    );
    expect(path).toContain('settings');
  });
});
