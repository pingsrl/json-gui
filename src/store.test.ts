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

// ── releaseDistantNodes — scroll dopo expand-all ──────────────────────────────
//
// Verifica il comportamento dell'albero quando, dopo expand-all, l'utente
// sposta la barra di scorrimento a metà: i nodi distanti vengono liberati
// dalla memoria mentre quelli nel viewport rimangono espansi.

describe('releaseDistantNodes — scroll dopo expand-all', () => {
  // Albero di test (3 livelli):
  //   root(1)
  //     sectionA(10) → [item1(100), item2(101), item3(102)]
  //     sectionB(11) → [item4(200), item5(201), item6(202)]
  //     sectionC(12) → [item7(300), item8(301), item9(302)]

  const root = makeNode(1, null, 'object', 3)
  const sectionA = makeNode(10, 'sectionA', 'array', 3)
  const sectionB = makeNode(11, 'sectionB', 'array', 3)
  const sectionC = makeNode(12, 'sectionC', 'array', 3)
  const item1 = makeNode(100, '0')
  const item2 = makeNode(101, '1')
  const item3 = makeNode(102, '2')
  const item4 = makeNode(200, '0')
  const item5 = makeNode(201, '1')
  const item6 = makeNode(202, '2')
  const item7 = makeNode(300, '0')
  const item8 = makeNode(301, '1')
  const item9 = makeNode(302, '2')

  const rootChildren = [sectionA, sectionB, sectionC]

  // Stato dopo expand-all: tutte le sezioni sono espanse
  function buildFullyExpandedMap(): Map<number, NodeDto[]> {
    return new Map([
      [root.id, rootChildren],
      [sectionA.id, [item1, item2, item3]],
      [sectionB.id, [item4, item5, item6]],
      [sectionC.id, [item7, item8, item9]],
    ])
  }

  // Simula releaseDistantNodes: rimuove le entry non nel visibleParentIds
  function releaseDistantNodes(
    expandedNodes: Map<number, NodeDto[]>,
    visibleParentIds: ReadonlySet<number>,
  ): Map<number, NodeDto[]> {
    const next = new Map(expandedNodes)
    for (const parentId of next.keys()) {
      if (!visibleParentIds.has(parentId)) {
        next.delete(parentId)
      }
    }
    return next
  }

  it('dopo expand-all mostra tutti i nodi di tutte le sezioni', () => {
    const expandedNodes = buildFullyExpandedMap()
    const visible = buildVisibleNodes(rootChildren, expandedNodes)
    // 3 sezioni + 3 figli per sezione = 12 VNode
    expect(visible).toHaveLength(12)
  })

  it('dopo scroll a metà: sectionA e sectionC vengono liberate, sectionB rimane', () => {
    const expandedNodes = buildFullyExpandedMap()

    // Viewport a metà: solo sectionB è visibile (+ root sempre presente)
    const visibleParentIds = new Set([root.id, sectionB.id])
    const afterRelease = releaseDistantNodes(expandedNodes, visibleParentIds)

    // sectionA e sectionC sono state rilasciate
    expect(afterRelease.has(sectionA.id)).toBe(false)
    expect(afterRelease.has(sectionC.id)).toBe(false)

    // sectionB è ancora espansa
    expect(afterRelease.has(sectionB.id)).toBe(true)
    expect(afterRelease.get(sectionB.id)).toHaveLength(3)
  })

  it('buildVisibleNodes con nodi rilasciati mostra solo le sezioni non espanse', () => {
    const expandedNodes = buildFullyExpandedMap()
    const visibleParentIds = new Set([root.id, sectionB.id])
    const afterRelease = releaseDistantNodes(expandedNodes, visibleParentIds)

    const visible = buildVisibleNodes(rootChildren, afterRelease)
    // sectionA: chiusa → 1 nodo; sectionB: aperta → 4 nodi; sectionC: chiusa → 1 nodo
    expect(visible).toHaveLength(6)

    const ids = visible.map((v) => v.node.id)
    // sectionA è presente come foglia chiusa
    expect(ids).toContain(sectionA.id)
    // i figli di sectionB sono visibili
    expect(ids).toContain(item4.id)
    expect(ids).toContain(item5.id)
    expect(ids).toContain(item6.id)
    // sectionC è presente come foglia chiusa
    expect(ids).toContain(sectionC.id)
    // i figli di sectionA e sectionC non sono visibili
    expect(ids).not.toContain(item1.id)
    expect(ids).not.toContain(item9.id)
  })

  it('dopo la release, il re-expand di sectionA ripristina i suoi figli', () => {
    const expandedNodes = buildFullyExpandedMap()
    const visibleParentIds = new Set([root.id, sectionB.id])
    const afterRelease = releaseDistantNodes(expandedNodes, visibleParentIds)

    // L'utente torna in cima: sectionA viene ri-espansa (simulazione caricamento)
    const reExpanded = new Map(afterRelease)
    reExpanded.set(sectionA.id, [item1, item2, item3])

    const visible = buildVisibleNodes(rootChildren, reExpanded)
    const ids = visible.map((v) => v.node.id)
    expect(ids).toContain(item1.id)
    expect(ids).toContain(item2.id)
    expect(ids).toContain(item3.id)
  })

  it('getVisibleSlice dopo scroll a metà restituisce solo i nodi della finestra', () => {
    const expandedNodes = buildFullyExpandedMap()
    // Preorder DFS con tutte le sezioni espanse (12 nodi totali):
    // 0:sectionA, 1:item1, 2:item2, 3:item3, 4:sectionB, 5:item4,
    // 6:item5, 7:item6, 8:sectionC, 9:item7, 10:item8, 11:item9
    // Scroll a offset 6, limit 4 => item5, item6, sectionC, item7
    const slice = getVisibleSlice(rootChildren, expandedNodes, 6, 4)
    expect(slice).toHaveLength(4)
    expect(slice[0].node.id).toBe(item5.id)   // 201
    expect(slice[1].node.id).toBe(item6.id)   // 202
    expect(slice[2].node.id).toBe(sectionC.id) // 12
    expect(slice[3].node.id).toBe(item7.id)   // 300
  })
})
