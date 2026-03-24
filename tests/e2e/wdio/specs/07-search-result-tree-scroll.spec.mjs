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

async function runExactValueSearch(query) {
  await browser.execute(async (q) => {
    await window.__jsonStore.getState().search(
      q,
      'values',
      false,
      false,
      true,
      '$.data',
      false,
      false
    );
  }, query);
  await waitForSearchToFinish();
}

async function getTreeViewportState(expectedPath) {
  return browser.execute((path) => {
    const store = window.__jsonStore.getState();
    const selectedId = store.selectedNodeId;
    const selectedPath = store.selectedNodePath;
    const parseMatch = /^\$\.data\.(\d+)\.(\d+)/.exec(selectedPath ?? '');
    const rowKey = parseMatch?.[1] ?? null;
    const leafKey = parseMatch?.[2] ?? null;
    const treeScroller = Array.from(document.querySelectorAll('.app-scrollbar')).find(
      (element) => element.querySelector('[data-node-id]')
    );
    const selectedRow = selectedId !== null
      ? document.querySelector(`[data-node-id="${selectedId}"]`)
      : null;
    const scrollerRect = treeScroller?.getBoundingClientRect() ?? null;
    const rowRect = selectedRow?.getBoundingClientRect() ?? null;
    const dataNode = store.rootChildren.find((node) => node.key === 'data') ?? null;
    const dataChildren = dataNode ? store.expandedNodes.get(dataNode.id) ?? [] : [];
    const rowNode = rowKey !== null
      ? dataChildren.find((node) => node.key === rowKey) ?? null
      : null;
    const rowChildren = rowNode ? store.expandedNodes.get(rowNode.id) ?? [] : [];
    let visibleIndex = -1;
    let cursor = 0;
    const stack = [{ nodes: store.rootChildren, index: 0 }];
    while (stack.length > 0) {
      const frame = stack[stack.length - 1];
      if (frame.index >= frame.nodes.length) {
        stack.pop();
        continue;
      }
      const node = frame.nodes[frame.index];
      frame.index += 1;
      if (node.id === selectedId) {
        visibleIndex = cursor;
        break;
      }
      cursor += 1;
      const children = store.expandedNodes.get(node.id);
      if (children && children.length > 0) {
        stack.push({ nodes: children, index: 0 });
      }
    }
    const parseIndex = (value) => {
      const match = /^\$\.data\.(\d+)\./.exec(value ?? '');
      return match ? Number(match[1]) : null;
    };

    return {
      expectedPath: path,
      selectedPath,
      selectedId,
      selectedIndex: parseIndex(selectedPath),
      visibleIndex,
      dataChildrenCount: dataChildren.length,
      rowNodeId: rowNode?.id ?? null,
      rowNodePresent: Boolean(rowNode),
      rowChildrenCount: rowChildren.length,
      selectedInRowChildren:
        Boolean(leafKey) &&
        rowChildren.some((node) => node.id === selectedId || node.key === leafKey),
      rowExists: Boolean(selectedRow),
      rowText: selectedRow?.textContent ?? null,
      scrollTop: treeScroller?.scrollTop ?? null,
      rowInsideViewport:
        Boolean(scrollerRect) &&
        Boolean(rowRect) &&
        rowRect.top >= scrollerRect.top &&
        rowRect.bottom <= scrollerRect.bottom
    };
  }, expectedPath);
}

async function navigateAndWaitForTreeSelection(result) {
  await browser.execute(async (nodeId) => {
    await window.__jsonStore.getState().navigateToNode(nodeId);
  }, result.node_id);
  let lastState = null;
  try {
    await browser.waitUntil(
      async () => {
        lastState = await getTreeViewportState(result.path);
        return (
          lastState.selectedPath === result.path &&
          lastState.rowExists &&
          lastState.rowInsideViewport
        );
      },
      {
        timeout: LARGE_TIMEOUT,
        interval: 200,
        timeoutMsg: `treeview non allineata al risultato ${result.path}`
      }
    );
  } catch (error) {
    throw new Error(
      `${error.message}\nstate=${JSON.stringify(lastState)}`
    );
  }
  return lastState ?? getTreeViewportState(result.path);
}

describeLarge('search result tree scroll', function () {
  this.timeout(240000);

  before(async () => {
    await loadFixture(browser, global.FIXTURE_PATH, LARGE_TIMEOUT);
  });

  afterEach(async () => {
    await browser.execute(() => {
      window.__jsonStore.getState().clearSearch();
    });
  });

  it('porta in viewport allo stesso modo risultati entro e oltre i primi 1000 nodi', async () => {
    await runExactValueSearch('ROBBERY');
    const earlyResult = await browser.execute(() => {
      const results = window.__jsonStore.getState().searchResults;
      return results.find((result) => {
        const match = /^\$\.data\.(\d+)\./.exec(result.path);
        return match && Number(match[1]) < 1000;
      }) ?? null;
    });

    expect(earlyResult).not.toBe(null);
    expect(earlyResult.path).toContain('$.data.');

    const earlyState = await navigateAndWaitForTreeSelection(earlyResult);
    expect(earlyState.selectedPath).toBe(earlyResult.path);
    expect(earlyState.selectedIndex).not.toBe(null);
    expect(earlyState.selectedIndex).toBeLessThan(1000);
    expect(earlyState.rowExists).toBe(true);
    expect(earlyState.rowInsideViewport).toBe(true);

    await runExactValueSearch('POLICE FACILITY / VEHICLE PARKING LOT');
    const deepResult = await browser.execute(() => {
      const results = window.__jsonStore.getState().searchResults;
      return results.find((result) => {
        const match = /^\$\.data\.(\d+)\./.exec(result.path);
        return match && Number(match[1]) >= 1000;
      }) ?? null;
    });

    expect(deepResult).not.toBe(null);
    expect(deepResult.path).toContain('$.data.');

    const deepState = await navigateAndWaitForTreeSelection(deepResult);
    expect(deepState.selectedPath).toBe(deepResult.path);
    expect(deepState.selectedIndex).not.toBe(null);
    expect(deepState.selectedIndex).toBeGreaterThanOrEqual(1000);
    expect(deepState.rowExists).toBe(true);
    expect(deepState.rowInsideViewport).toBe(true);
    expect(deepState.scrollTop).not.toBe(null);
    expect(earlyState.scrollTop).not.toBe(null);
    expect(deepState.scrollTop).toBeGreaterThan(earlyState.scrollTop);
  });
});
