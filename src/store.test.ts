/**
 * Test suite per le funzioni pure di store.ts.
 * Non richiede Tauri né DOM: eseguibili con `npm test`.
 *
 * Obiettivo: bloccare le PR di Dependabot che rompono
 * la logica di costruzione dell'albero e delle cache.
 */

import { describe, it, expect } from 'vitest'
import { buildVisibleNodes, insertVisibleChildren } from './store'
import type { NodeDto, VNode } from './store'

// ── helpers ───────────────────────────────────────────────────────────────────

function makeNode(
  id: number,
  key: string | null,
  type: string = 'string',
  hasChildren = false,
  childrenCount = 0,
): NodeDto {
  return {
    id,
    key,
    value_type: type,
    value_preview: String(id),
    has_children: hasChildren,
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
    const parent = makeNode(1, 'root', 'object', true, 2)
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
    const p1 = makeNode(1, 'p1', 'object', true, 2)
    const p2 = makeNode(2, 'p2')
    const expanded = new Map([[1, [c1, c2]]])
    const result = buildVisibleNodes([p1, p2], expanded)
    expect(result.map((v) => v.node.id)).toEqual([1, 10, 11, 2])
  })

  it('nodo non espanso non mostra i figli anche se la mappa ha altri espansi', () => {
    const child = makeNode(10, 'c')
    const p1 = makeNode(1, 'p1', 'object', true, 1)
    const p2 = makeNode(2, 'p2', 'object', true, 1) // non espanso
    const expanded = new Map([[1, [child]]]) // solo p1 espanso
    const result = buildVisibleNodes([p1, p2], expanded)
    expect(result.map((v) => v.node.id)).toEqual([1, 10, 2])
  })

  it('supporta espansione profonda ricorsiva', () => {
    const level3 = makeNode(100, 'deep')
    const level2 = makeNode(20, 'mid', 'object', true, 1)
    const level1 = makeNode(10, 'top', 'object', true, 1)
    const root = makeNode(1, 'root', 'object', true, 1)
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
    const child = makeNode(10, 'c', 'object', true, 1)
    const parent = makeNode(1, 'p', 'object', true, 1)

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
    const parent = makeNode(1, 'p', 'object', true, 1)
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
    const arrayNode = makeNode(1, 'arr', 'array', true, 3)
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
    const parent = makeNode(1, 'parent', 'object', true, 2)
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
    const parent = makeNode(1, 'parent', 'object', true, 1)
    const child = makeNode(10, 'child', 'object', true, 1)
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

// ── NodeDto type contract ─────────────────────────────────────────────────────

describe('NodeDto shape', () => {
  it('has_children è false quando children_count è 0', () => {
    const node = makeNode(1, 'x', 'string', false, 0)
    expect(node.has_children).toBe(false)
    expect(node.children_count).toBe(0)
  })

  it('key può essere null per nodi radice array', () => {
    const node = makeNode(1, null, 'object', false, 0)
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
