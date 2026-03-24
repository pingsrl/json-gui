import { expect } from '@wdio/globals';
import { loadFixture } from '../helpers/index.mjs';

const LARGE_TIMEOUT = 120000;
const RUN_LARGE_FIXTURE_E2E =
  process.env.RUN_LARGE_FIXTURE_E2E === '1' && Boolean(process.env.FIXTURE_PATH);
const describeLarge = RUN_LARGE_FIXTURE_E2E ? describe : describe.skip;

async function waitForSearchToFinish(timeout = LARGE_TIMEOUT) {
  await browser.waitUntil(
    () => browser.execute(() => !window.__jsonStore.getState().searching),
    { timeout, timeoutMsg: 'ricerca non terminata' }
  );
}

async function runTextSearch(query, options = {}) {
  const {
    target = 'values',
    caseSensitive = false,
    useRegex = false,
    exactMatch = false,
    path = '',
    multiline = false,
    dotAll = false,
  } = options;

  await browser.execute(
    async (q, t, cs, re, ex, p, ml, ds) => {
      await window.__jsonStore
        .getState()
        .search(q, t, cs, re, ex, p, ml, ds);
    },
    query,
    target,
    caseSensitive,
    useRegex,
    exactMatch,
    path,
    multiline,
    dotAll
  );
  await waitForSearchToFinish();
}

async function runObjectSearch(filters, options = {}) {
  const {
    keyCaseSensitive = false,
    valueCaseSensitive = false,
    path = '',
  } = options;

  await browser.execute(
    async (f, kcs, vcs, p) => {
      await window.__jsonStore
        .getState()
        .searchObjects(f, kcs, vcs, p);
    },
    filters,
    keyCaseSensitive,
    valueCaseSensitive,
    path
  );
  await waitForSearchToFinish();
}

describeLarge('rows.json lazy UX', function () {
  this.timeout(240000);

  before(async () => {
    await loadFixture(browser, global.FIXTURE_PATH, LARGE_TIMEOUT);
  });

  afterEach(async () => {
    await browser.execute(() => {
      window.__jsonStore.getState().clearSearch();
    });
  });

  it('carica rows.json con i nodi root attesi', async () => {
    const state = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      return {
        filePath: store.filePath,
        rootType: store.rootNode?.value_type,
        rootKeys: store.rootChildren.map((child) => child.key),
        nodeCount: store.nodeCount,
      };
    });

    expect(state.filePath).toBe(global.FIXTURE_PATH);
    expect(state.rootType).toBe('object');
    expect(state.rootKeys).toContain('meta');
    expect(state.rootKeys).toContain('data');
    expect(state.nodeCount).toBeGreaterThan(0);
  });

  it('ricerca testuale scoped su meta.view e navigazione funzionano', async () => {
    await runTextSearch('Chicago Police Department', {
      target: 'values',
      path: '$.meta.view',
    });

    const results = await browser.execute(() =>
      window.__jsonStore.getState().searchResults
    );
    expect(results.length).toBeGreaterThan(0);
    expect(results[0].path).toContain('$.meta.view');

    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      await store.navigateToNode(store.searchResults[0].node_id);
    });

    const selectedPath = await browser.execute(() =>
      window.__jsonStore.getState().selectedNodePath
    );
    expect(selectedPath).toContain('$.meta.view');
  });

  it('object search scoped su meta.view trova il nodo oggetto corretto', async () => {
    await runObjectSearch(
      [{ path: 'assetType', operator: 'equals', value: 'dataset' }],
      { path: '$.meta.view' }
    );

    const results = await browser.execute(() =>
      window.__jsonStore.getState().searchResults
    );
    expect(results.length).toBeGreaterThan(0);
    expect(results[0].path).toBe('$.meta.view');

    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      await store.navigateToNode(store.searchResults[0].node_id);
    });

    const selectedPath = await browser.execute(() =>
      window.__jsonStore.getState().selectedNodePath
    );
    expect(selectedPath).toBe('$.meta.view');
  });

  it('apertura nodi lazy annidati e scroll su data caricano nuove righe', async () => {
    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      const metaNode = store.rootChildren.find((child) => child.key === 'meta');
      if (!metaNode) throw new Error('meta node missing');
      if (!store.expandedNodes.has(metaNode.id)) {
        await store.toggleNode(metaNode.id);
      }
    });

    await browser.waitUntil(
      () => browser.execute(() => {
        const store = window.__jsonStore.getState();
        const metaNode = store.rootChildren.find((child) => child.key === 'meta');
        if (!metaNode) return false;
        return (store.expandedNodes.get(metaNode.id) ?? []).some((child) => child.key === 'view');
      }),
      { timeout: LARGE_TIMEOUT, timeoutMsg: 'meta non espanso correttamente' }
    );

    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      const metaNode = store.rootChildren.find((child) => child.key === 'meta');
      const viewNode = metaNode
        ? (store.expandedNodes.get(metaNode.id) ?? []).find((child) => child.key === 'view')
        : null;
      if (!viewNode) throw new Error('view node missing');
      if (!store.expandedNodes.has(viewNode.id)) {
        await store.toggleNode(viewNode.id);
      }
    });

    await browser.waitUntil(
      () => browser.execute(() => {
        const store = window.__jsonStore.getState();
        const metaNode = store.rootChildren.find((child) => child.key === 'meta');
        const viewNode = metaNode
          ? (store.expandedNodes.get(metaNode.id) ?? []).find((child) => child.key === 'view')
          : null;
        if (!viewNode) return false;
        return (store.expandedNodes.get(viewNode.id) ?? []).some((child) => child.key === 'assetType');
      }),
      { timeout: LARGE_TIMEOUT, timeoutMsg: 'view non espanso correttamente' }
    );

    await browser.execute(async () => {
      const store = window.__jsonStore.getState();
      const dataNode = store.rootChildren.find((child) => child.key === 'data');
      if (!dataNode) throw new Error('data node missing');
      if (!store.expandedNodes.has(dataNode.id)) {
        await store.toggleNode(dataNode.id);
      }
    });

    await browser.waitUntil(
      () => browser.execute(() => {
        const store = window.__jsonStore.getState();
        const dataNode = store.rootChildren.find((child) => child.key === 'data');
        if (!dataNode) return false;
        const children = store.expandedNodes.get(dataNode.id) ?? [];
        return children.length > 0;
      }),
      { timeout: LARGE_TIMEOUT, timeoutMsg: 'data non espanso' }
    );

    const initialInfo = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      const dataNode = store.rootChildren.find((child) => child.key === 'data');
      const children = dataNode ? store.expandedNodes.get(dataNode.id) ?? [] : [];
      return {
        realCount: children.filter((child) => !child.synthetic_kind).length,
        hasLoadMore: children.some((child) => child.synthetic_kind === 'load-more'),
      };
    });

    expect(initialInfo.realCount).toBeGreaterThan(0);
    expect(initialInfo.hasLoadMore).toBe(true);

    await browser.waitUntil(
      () => browser.execute((beforeCount) => {
        const scrollers = Array.from(document.querySelectorAll('.app-scrollbar'));
        const treeScroller = scrollers.reduce(
          (best, element) =>
            !best || element.scrollHeight > best.scrollHeight ? element : best,
          null
        );
        if (treeScroller) {
          treeScroller.scrollTop = treeScroller.scrollHeight;
          treeScroller.dispatchEvent(new Event('scroll', { bubbles: true }));
        }

        const store = window.__jsonStore.getState();
        const dataNode = store.rootChildren.find((child) => child.key === 'data');
        if (!dataNode) return false;
        const children = store.expandedNodes.get(dataNode.id) ?? [];
        const realCount = children.filter((child) => !child.synthetic_kind).length;
        return realCount > beforeCount;
      }, initialInfo.realCount),
      {
        timeout: LARGE_TIMEOUT,
        interval: 250,
        timeoutMsg: 'lo scroll non ha caricato una pagina aggiuntiva di data',
      }
    );
  });

  it('ricerca testuale su data e expandAll completano senza rompere la UX', async () => {
    await runTextSearch('ROBBERY', {
      target: 'values',
      path: '$.data',
    });

    const searchInfo = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      return {
        count: store.searchResults.length,
        firstPath: store.searchResults[0]?.path ?? null,
      };
    });
    expect(searchInfo.count).toBeGreaterThan(0);
    expect(searchInfo.firstPath).toContain('$.data.');

    await browser.execute(() => {
      window.__jsonStore.getState().collapseAll();
    });

    await browser.execute(async () => {
      await window.__jsonStore.getState().expandAll();
    });

    await browser.waitUntil(
      () => browser.execute(() => !window.__jsonStore.getState().loading),
      { timeout: 180000, timeoutMsg: 'expandAll non terminato su rows.json' }
    );

    const expandInfo = await browser.execute(() => {
      const store = window.__jsonStore.getState();
      return {
        expandedAll: store.expandedAll,
        expandedNodesSize: store.expandedNodes.size,
      };
    });

    expect(expandInfo.expandedAll).toBe(true);
    expect(expandInfo.expandedNodesSize).toBeGreaterThan(10);
  });
});
