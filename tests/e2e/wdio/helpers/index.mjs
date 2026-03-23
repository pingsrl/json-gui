/**
 * Aspetta che il window.__jsonStore sia disponibile nel webview.
 * L'app React impiega qualche centinaio di ms per montarsi.
 */
export async function waitForApp(browser, timeout = 10000) {
  await browser.waitUntil(
    () => browser.execute(() => typeof window.__jsonStore !== 'undefined'),
    { timeout, timeoutMsg: 'window.__jsonStore non disponibile: assicurati che il build sia stato fatto con npm run build' }
  );
}

/**
 * Carica il file fixture di test tramite lo store Zustand.
 * Equivale a aprire il file dall'app.
 */
export async function loadFixture(browser) {
  await waitForApp(browser);
  await browser.execute(async (fixturePath) => {
    await window.__jsonStore.getState().openFile(fixturePath);
  }, global.FIXTURE_PATH);
  // Aspetta che rootNode sia popolato
  await browser.waitUntil(
    () => browser.execute(() => window.__jsonStore.getState().rootNode !== null),
    { timeout: 10000, timeoutMsg: 'rootNode non popolato dopo openFile' }
  );
}

/**
 * Aspetta che una condizione sullo store sia vera.
 * @param {Function} predicate - (state) => boolean, serializzata ed eseguita nel browser
 */
export async function waitForStore(browser, predicate, timeout = 5000) {
  await browser.waitUntil(
    () => browser.execute((pred) => {
      try {
        return (new Function('state', `return (${pred})(state)`))(
          window.__jsonStore.getState()
        );
      } catch { return false; }
    }, predicate.toString()),
    { timeout }
  );
  return browser.execute(() => window.__jsonStore.getState());
}

/**
 * Restituisce lo stato corrente dello store.
 */
export async function getStoreState(browser) {
  return browser.execute(() => {
    const s = window.__jsonStore.getState();
    return {
      rootNode: s.rootNode,
      rootChildrenCount: s.rootChildren.length,
      rootChildrenKeys: s.rootChildren.map(c => c.key),
      expandedNodesSize: s.expandedNodes.size,
      selectedNodeId: s.selectedNodeId,
      searchResultsCount: s.searchResults?.length ?? 0,
      hasActiveSearch: s.hasActiveSearch,
      searching: s.searching,
      filePath: s.filePath,
    };
  });
}
