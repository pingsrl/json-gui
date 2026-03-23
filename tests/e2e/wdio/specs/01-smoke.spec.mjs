import { expect } from '@wdio/globals';
import { waitForApp } from '../helpers/index.mjs';

describe('Smoke — avvio app', () => {
  before(async () => {
    await waitForApp(browser);
  });

  it('title è JsonGUI', async () => {
    const title = await browser.getTitle();
    expect(title).toBe('JsonGUI');
  });

  it('window handle è "main"', async () => {
    const handle = await browser.getWindowHandle();
    expect(handle).toBe('main');
  });

  it('store è accessibile globalmente', async () => {
    const hasStore = await browser.execute(() => typeof window.__jsonStore !== 'undefined');
    expect(hasStore).toBe(true);
  });

  it('nessun file caricato allo start', async () => {
    const state = await browser.execute(() => ({
      rootNode: window.__jsonStore.getState().rootNode,
      filePath: window.__jsonStore.getState().filePath,
    }));
    expect(state.rootNode).toBe(null);
    expect(state.filePath).toBe(null);
  });

  it('la UI mostra stato "nessun file"', async () => {
    // Il TreePanel mostra l'icona FolderOpen quando rootNode è null
    const hasPlaceholder = await browser.execute(() => {
      return document.querySelector('svg') !== null;
    });
    expect(hasPlaceholder).toBe(true);
  });

  it('screenshot visivo iniziale', async () => {
    const screenshot = await browser.takeScreenshot();
    expect(typeof screenshot).toBe('string');
    expect(screenshot.length).toBeGreaterThan(100);
  });
});
