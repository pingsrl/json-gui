/**
 * Test suite per le funzioni pure di store.ts.
 * Non richiede Tauri né DOM: eseguibili con `npm test`.
 *
 * Obiettivo: bloccare le PR di Dependabot che rompono
 * la logica di costruzione dell'albero e delle cache.
 */

import { describe, it, expect } from 'vitest'
import {
  buildVisibleNodes,
  buildVisibleSubtreeSizeMap,
  countVisibleNodes,
  getVisibleSlice,
  findVisibleNodeIndex,
  insertVisibleChildren,
  sortSearchResults,
  decoratePagedChildren,
  makeLoadMoreNode,
  UNKNOWN_COUNT_SENTINEL,
  LARGE_NODE_PAGE_SIZE,
} from './store'
import type { NodeDto, SearchResult, VNode } from './store'

// ── helpers ───────────────────────────────────────────────────────────────────

function makeNode(
  id: number,
  key: string | null,
  type: string = 'string',
  childrenCount = 0,
): NodeDto {
  return {
    id,
    key,
    value_type: type,
    value_preview: String(id),
    children_count: childrenCount,
  }
}

// ── buildVisibleNodes ─────────────────────────────────────────────────────────

describe('buildVisibleNodes', () => {
  it('ritorna lista vuota se rootChildren è vuoto', () => {
    const result = buildVisibleNodes([], new Map())
    expect(result).toEqual([])
  })

  it('mostra solo i rootChildren se nessun nodo è espanso', () => {
    const roots = [makeNode(1, 'a'), makeNode(2, 'b'), makeNode(3, 'c')]
    const result = buildVisibleNodes(roots, new Map())
    expect(result).toHaveLength(3)
    expect(result.map((v) => v.node.id)).toEqual([1, 2, 3])
  })

  it('tutti i rootChildren hanno depth 0', () => {
    const roots = [makeNode(1, 'a'), makeNode(2, 'b')]
    const result = buildVisibleNodes(roots, new Map())
    result.forEach((v) => expect(v.depth).toBe(0))
  })

  it('i figli di un nodo espanso hanno depth 1', () => {
    const child1 = makeNode(10, 'x')
    const child2 = makeNode(11, 'y')
    const parent = makeNode(1, 'root', 'object', 2)
    const expanded = new Map([[1, [child1, child2]]])
    const result = buildVisibleNodes([parent], expanded)
    expect(result).toHaveLength(3)
    expect(result[0].depth).toBe(0) // parent
    expect(result[1].depth).toBe(1) // child1
    expect(result[2].depth).toBe(1) // child2
  })

  it("rispetta l'ordine DFS: parent poi figli poi fratelli", () => {
    const c1 = makeNode(10, 'c1')
    const c2 = makeNode(11, 'c2')
    const p1 = makeNode(1, 'p1', 'object', 2)
    const p2 = makeNode(2, 'p2')
    const expanded = new Map([[1, [c1, c2]]])
    const result = buildVisibleNodes([p1, p2], expanded)
    expect(result.map((v) => v.node.id)).toEqual([1, 10, 11, 2])
  })

  it('nodo non espanso non mostra i figli anche se la mappa ha altri espansi', () => {
    const child = makeNode(10, 'c')
    const p1 = makeNode(1, 'p1', 'object', 1)
    const p2 = makeNode(2, 'p2', 'object', 1) // non espanso
    const expanded = new Map([[1, [child]]]) // solo p1 espanso
    const result = buildVisibleNodes([p1, p2], expanded)
    expect(result.map((v) => v.node.id)).toEqual([1, 10, 2])
  })

  it('supporta espansione profonda ricorsiva', () => {
    const level3 = makeNode(100, 'deep')
    const level2 = makeNode(20, 'mid', 'object', 1)
    const level1 = makeNode(10, 'top', 'object', 1)
    const root = makeNode(1, 'root', 'object', 1)
    const expanded = new Map([
      [1, [level1]],
      [10, [level2]],
      [20, [level3]],
    ])
    const result = buildVisibleNodes([root], expanded)
    expect(result.map((v) => v.node.id)).toEqual([1, 10, 20, 100])
    expect(result.map((v) => v.depth)).toEqual([0, 1, 2, 3])
  })

  it('collassare un nodo rimuove tutti i discendenti', () => {
    const grandchild = makeNode(100, 'gc')
    const child = makeNode(10, 'c', 'object', 1)
    const parent = makeNode(1, 'p', 'object', 1)

    // tutto espanso
    const fullyExpanded = new Map([
      [1, [child]],
      [10, [grandchild]],
    ])
    const fullResult = buildVisibleNodes([parent], fullyExpanded)
    expect(fullResult).toHaveLength(3)

    // collasso parent → nessun discendente visibile
    const collapsed = new Map<number, NodeDto[]>()
    const collapsedResult = buildVisibleNodes([parent], collapsed)
    expect(collapsedResult).toHaveLength(1)
    expect(collapsedResult[0].node.id).toBe(1)
  })

  it('non mostra i figli di un nodo rimosso dalla mappa expanded', () => {
    const child = makeNode(10, 'c')
    const parent = makeNode(1, 'p', 'object', 1)
    const expanded = new Map([[1, [child]]])

    const withChild = buildVisibleNodes([parent], expanded)
    expect(withChild).toHaveLength(2)

    expanded.delete(1)
    const withoutChild = buildVisibleNodes([parent], expanded)
    expect(withoutChild).toHaveLength(1)
  })

  it('restituisce VNode con riferimento al NodeDto originale', () => {
    const node = makeNode(42, 'test')
    const result = buildVisibleNodes([node], new Map())
    expect(result[0].node).toBe(node) // stesso riferimento
  })

  it('gestisce un array di elementi con chiavi numeriche', () => {
    const items = [0, 1, 2].map((i) => makeNode(i + 10, String(i)))
    const arrayNode = makeNode(1, 'arr', 'array', 3)
    const expanded = new Map([[1, items]])
    const result = buildVisibleNodes([arrayNode], expanded)
    expect(result).toHaveLength(4)
    expect(result[1].node.key).toBe('0')
    expect(result[2].node.key).toBe('1')
    expect(result[3].node.key).toBe('2')
  })
})

describe('insertVisibleChildren', () => {
  it('inserisce i figli subito dopo il parent mantenendo depth +1', () => {
    const parent = makeNode(1, 'parent', 'object', 2)
    const sibling = makeNode(2, 'sibling')
    const child1 = makeNode(10, 'c1')
    const child2 = makeNode(11, 'c2')

    const result = insertVisibleChildren(
      buildVisibleNodes([parent, sibling], new Map()),
      [[1, [child1, child2]]],
    )

    expect(result.map((v) => v.node.id)).toEqual([1, 10, 11, 2])
    expect(result.map((v) => v.depth)).toEqual([0, 1, 1, 0])
  })

  it('gestisce espansioni successive su livelli già visibili', () => {
    const parent = makeNode(1, 'parent', 'object', 1)
    const child = makeNode(10, 'child', 'object', 1)
    const grandchild = makeNode(100, 'gc')

    const withChild = insertVisibleChildren(
      buildVisibleNodes([parent], new Map()),
      [[1, [child]]],
    )
    const withGrandchild = insertVisibleChildren(withChild, [[10, [grandchild]]])

    expect(withGrandchild.map((v) => v.node.id)).toEqual([1, 10, 100])
    expect(withGrandchild.map((v) => v.depth)).toEqual([0, 1, 2])
  })
})

describe('visible slice helpers', () => {
  it('calcola il conteggio visibile senza materializzare tutto l albero', () => {
    const grandchild = makeNode(100, 'gc')
    const child = makeNode(10, 'c', 'object', 1)
    const sibling = makeNode(2, 's')
    const parent = makeNode(1, 'p', 'object', 1)
    const expanded = new Map<number, NodeDto[]>([
      [1, [child]],
      [10, [grandchild]],
    ])

    const sizeMap = buildVisibleSubtreeSizeMap([parent, sibling], expanded)

    expect(sizeMap.get(10)).toBe(2)
    expect(sizeMap.get(1)).toBe(3)
    expect(countVisibleNodes([parent, sibling], expanded, sizeMap)).toBe(4)
  })

  it('estrae una slice visibile corretta a meta albero', () => {
    const grandchild = makeNode(100, 'gc')
    const child = makeNode(10, 'c', 'object', 1)
    const sibling = makeNode(2, 's')
    const parent = makeNode(1, 'p', 'object', 1)
    const expanded = new Map<number, NodeDto[]>([
      [1, [child]],
      [10, [grandchild]],
    ])
    const sizeMap = buildVisibleSubtreeSizeMap([parent, sibling], expanded)

    const slice = getVisibleSlice([parent, sibling], expanded, 1, 2, sizeMap)

    expect(slice.map((v) => v.node.id)).toEqual([10, 100])
    expect(slice.map((v) => v.depth)).toEqual([1, 2])
  })

  it('trova l indice visibile di un nodo in preorder', () => {
    const grandchild = makeNode(100, 'gc')
    const child = makeNode(10, 'c', 'object', 1)
    const sibling = makeNode(2, 's')
    const parent = makeNode(1, 'p', 'object', 1)
    const expanded = new Map<number, NodeDto[]>([
      [1, [child]],
      [10, [grandchild]],
    ])

    expect(findVisibleNodeIndex([parent, sibling], expanded, 1)).toBe(0)
    expect(findVisibleNodeIndex([parent, sibling], expanded, 10)).toBe(1)
    expect(findVisibleNodeIndex([parent, sibling], expanded, 100)).toBe(2)
    expect(findVisibleNodeIndex([parent, sibling], expanded, 2)).toBe(3)
  })
})

// ── NodeDto type contract ─────────────────────────────────────────────────────

describe('NodeDto shape', () => {
  it('children_count è 0 per nodi foglia', () => {
    const node = makeNode(1, 'x', 'string', 0)
    expect(node.children_count).toBe(0)
  })

  it('key può essere null per nodi radice array', () => {
    const node = makeNode(1, null, 'object', 0)
    expect(node.key).toBeNull()
  })
})

// ── VNode type contract ───────────────────────────────────────────────────────

describe('VNode shape', () => {
  it('ogni VNode ha node e depth', () => {
    const root = makeNode(1, 'root')
    const result: VNode[] = buildVisibleNodes([root], new Map())
    expect(result[0]).toHaveProperty('node')
    expect(result[0]).toHaveProperty('depth')
  })
})

function makeSearchResult(
  nodeId: number,
  fileOrder: number,
  key: string | null,
  valuePreview: string,
  kind: 'node' | 'object' = 'node',
): SearchResult {
  return {
    node_id: nodeId,
    file_order: fileOrder,
    key,
    path: `$.${key ?? nodeId}`,
    value_preview: valuePreview,
    kind,
  }
}

describe('sortSearchResults', () => {
  it("ordina per pertinenza prima dell'ordine nel file", () => {
    const results = [
      makeSearchResult(1, 20, 'titleLong', 'something'),
      makeSearchResult(2, 10, 'title', 'something else'),
    ]

    const sorted = sortSearchResults(results, 'title', 'relevance')

    expect(sorted.map((r) => r.node_id)).toEqual([2, 1])
  })

  it("ordina per ordine nel file quando richiesto", () => {
    const results = [
      makeSearchResult(1, 20, 'title', 'value'),
      makeSearchResult(2, 10, 'title', 'value'),
    ]

    const sorted = sortSearchResults(results, 'title', 'file')

    expect(sorted.map((r) => r.node_id)).toEqual([2, 1])
  })

  it("ordina sempre per ordine nel file per i risultati object search", () => {
    const results = [
      makeSearchResult(1, 20, null, '{2 keys}', 'object'),
      makeSearchResult(2, 10, null, '{3 keys}', 'object'),
    ]

    const sorted = sortSearchResults(results, 'title', 'relevance')

    expect(sorted.map((r) => r.node_id)).toEqual([2, 1])
  })
})

// ── decoratePagedChildren ─────────────────────────────────────────────────────

describe('decoratePagedChildren — conteggio noto', () => {
  it('non aggiunge load-more se tutti i figli sono già presenti', () => {
    const parent = makeNode(1, 'p', 'array', 3)
    const children = [makeNode(10, '0'), makeNode(11, '1'), makeNode(12, '2')]
    const result = decoratePagedChildren(parent, children, 0)
    expect(result).toHaveLength(3)
    expect(result.every((n) => !n.synthetic_kind)).toBe(true)
  })

  it('aggiunge load-more se ci sono altri figli da caricare', () => {
    const parent = makeNode(1, 'p', 'array', 5)
    const children = [makeNode(10, '0'), makeNode(11, '1'), makeNode(12, '2')]
    const result = decoratePagedChildren(parent, children, 0)
    expect(result).toHaveLength(4)
    expect(result[3].synthetic_kind).toBe('load-more')
    expect(result[3].next_offset).toBe(3)
    expect(result[3].remaining_count).toBe(2)
  })

  it('il testo load-more mostra il numero di rimanenti', () => {
    const parent = makeNode(1, 'p', 'array', 10)
    const children = Array.from({ length: 3 }, (_, i) => makeNode(10 + i, String(i)))
    const result = decoratePagedChildren(parent, children, 0)
    const loadMore = result.find((n) => n.synthetic_kind === 'load-more')!
    expect(loadMore.value_preview).toContain('7')
    expect(loadMore.value_preview).toContain('remaining')
  })
})

describe('decoratePagedChildren — conteggio sconosciuto (lazy grande)', () => {
  const makeUnknownParent = () => makeNode(1, 'data', 'array', UNKNOWN_COUNT_SENTINEL)

  it('aggiunge load-more quando la pagina è piena', () => {
    const parent = makeUnknownParent()
    const children = Array.from({ length: LARGE_NODE_PAGE_SIZE }, (_, i) => makeNode(100 + i, String(i)))
    const result = decoratePagedChildren(parent, children, 0, LARGE_NODE_PAGE_SIZE)
    expect(result).toHaveLength(LARGE_NODE_PAGE_SIZE + 1)
    const loadMore = result[result.length - 1]
    expect(loadMore.synthetic_kind).toBe('load-more')
    expect(loadMore.next_offset).toBe(LARGE_NODE_PAGE_SIZE)
    // Nessun "remaining" numerico nel testo
    expect(loadMore.value_preview).not.toContain('remaining')
    expect(loadMore.value_preview).toContain('…')
  })

  it('non aggiunge load-more quando la pagina è parziale (fine array)', () => {
    const parent = makeUnknownParent()
    const children = Array.from({ length: 42 }, (_, i) => makeNode(100 + i, String(i)))
    const result = decoratePagedChildren(parent, children, 0, LARGE_NODE_PAGE_SIZE)
    expect(result).toHaveLength(42)
    expect(result.every((n) => !n.synthetic_kind)).toBe(true)
  })

  it('load-more usa offset corretto dopo la prima pagina', () => {
    const parent = makeUnknownParent()
    const children = Array.from({ length: LARGE_NODE_PAGE_SIZE }, (_, i) => makeNode(200 + i, String(i)))
    const result = decoratePagedChildren(parent, children, LARGE_NODE_PAGE_SIZE, LARGE_NODE_PAGE_SIZE)
    const loadMore = result[result.length - 1]
    expect(loadMore.next_offset).toBe(LARGE_NODE_PAGE_SIZE * 2)
  })
})

// ── makeLoadMoreNode ──────────────────────────────────────────────────────────

describe('makeLoadMoreNode', () => {
  it('conteggio noto: mostra "N remaining"', () => {
    const node = makeLoadMoreNode(1, 100, 250)
    expect(node.value_preview).toContain('remaining')
    expect(node.remaining_count).toBe(150)
    expect(node.next_offset).toBe(100)
  })

  it('conteggio sconosciuto: mostra "…" senza remaining numerico', () => {
    const node = makeLoadMoreNode(1, 1000, UNKNOWN_COUNT_SENTINEL)
    expect(node.value_preview).toContain('…')
    expect(node.value_preview).not.toContain('remaining')
    expect(node.remaining_count).toBeUndefined()
  })

  it('è sempre un nodo sintetico con children_count 0', () => {
    const node = makeLoadMoreNode(5, 0, 100)
    expect(node.synthetic_kind).toBe('load-more')
    expect(node.children_count).toBe(0)
    expect(node.parent_node_id).toBe(5)
  })
})
