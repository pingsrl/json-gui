/**
 * Benchmark JS delle operazioni critiche del frontend di JsonGUI.
 *
 * Misura:
 *  1. buildVisibleNodes  — ricostruzione dell'array piatto dei nodi visibili
 *  2. new Map(existing)  — costo copia Map (fatto ad ogni throttled update)
 *  3. pending.push()     — spread dei chunk nell'array pending
 *  4. Map insertion      — inserimento incrementale in Map
 *
 * Esegui con:
 *   npx tsx src/bench/perf.mts
 */

// ── tipi minimi compatibili con store.ts ─────────────────────────────────────

interface NodeDto {
  id: number;
  key: string | null;
  value_type: string;
  value_preview: string;
  has_children: boolean;
  children_count: number;
}

interface VNode {
  node: NodeDto;
  depth: number;
}

// ── buildVisibleNodes (copia esatta da store.ts) ──────────────────────────────

function buildVisibleNodes(
  rootChildren: NodeDto[],
  expandedNodes: Map<number, NodeDto[]>
): VNode[] {
  const result: VNode[] = [];
  const stack: Array<{ nodes: NodeDto[]; depth: number; index: number }> = [
    { nodes: rootChildren, depth: 0, index: 0 }
  ];
  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    if (frame.index >= frame.nodes.length) {
      stack.pop();
      continue;
    }
    const node = frame.nodes[frame.index++];
    result.push({ node, depth: frame.depth });
    const children = expandedNodes.get(node.id);
    if (children && children.length > 0) {
      stack.push({ nodes: children, depth: frame.depth + 1, index: 0 });
    }
  }
  return result;
}

// ── Generatori ────────────────────────────────────────────────────────────────

/** Crea N nodi foglia con id sequenziali a partire da startId */
function makeLeafNodes(count: number, startId: number): NodeDto[] {
  return Array.from({ length: count }, (_, i) => ({
    id: startId + i,
    key: `key${i}`,
    value_type: "string",
    value_preview: `"value${i}"`,
    has_children: false,
    children_count: 0
  }));
}

/**
 * Costruisce un albero piatto (2 livelli): N root con M figli ciascuno.
 * Restituisce [rootChildren, expandedNodes].
 */
function buildFlatTree(
  rootCount: number,
  childrenPerRoot: number
): [NodeDto[], Map<number, NodeDto[]>] {
  const rootChildren: NodeDto[] = [];
  const expandedNodes = new Map<number, NodeDto[]>();
  let nextId = 0;

  for (let i = 0; i < rootCount; i++) {
    const parentId = nextId++;
    const children = makeLeafNodes(childrenPerRoot, nextId);
    nextId += childrenPerRoot;

    rootChildren.push({
      id: parentId,
      key: `root${i}`,
      value_type: "object",
      value_preview: `{${childrenPerRoot} keys}`,
      has_children: true,
      children_count: childrenPerRoot
    });
    expandedNodes.set(parentId, children);
  }

  return [rootChildren, expandedNodes];
}

// ── Infrastruttura benchmark ──────────────────────────────────────────────────

function bench(name: string, fn: () => void, iterations = 50): void {
  // Warm-up
  for (let i = 0; i < 5; i++) fn();

  const times: number[] = [];
  for (let i = 0; i < iterations; i++) {
    const t0 = performance.now();
    fn();
    times.push(performance.now() - t0);
  }
  times.sort((a, b) => a - b);
  const median = times[Math.floor(times.length / 2)];
  const p95 = times[Math.floor(times.length * 0.95)];
  const mean = times.reduce((a, b) => a + b, 0) / times.length;
  console.log(
    `  ${name.padEnd(52)} mean=${mean.toFixed(2).padStart(7)}ms  median=${median.toFixed(2).padStart(7)}ms  p95=${p95.toFixed(2).padStart(7)}ms`
  );
}

// ── 1. buildVisibleNodes ──────────────────────────────────────────────────────

console.log("\n=== buildVisibleNodes (albero completamente espanso) ===");
for (const [roots, children] of [
  [100, 10], //   1 000 nodi visibili
  [500, 10], //   5 000
  [1000, 10], //  10 000
  [2000, 10], //  20 000
  [5000, 10], //  50 000
  [10000, 10], // 100 000
  [50000, 10] // 500 000  (stima per JSON 150MB)
] as [number, number][]) {
  const [rootChildren, expandedNodes] = buildFlatTree(roots, children);
  const totalNodes = roots * (1 + children);
  bench(
    `${(totalNodes / 1000).toFixed(0)}k nodi (${roots} root × ${children} figli)`,
    () => buildVisibleNodes(rootChildren, expandedNodes),
    totalNodes > 200_000 ? 10 : 50
  );
}

// ── 2. new Map(existing) — costo copia ────────────────────────────────────────

console.log("\n=== new Map(existing) — copia su ogni throttled update ===");
for (const size of [1_000, 5_000, 10_000, 50_000, 100_000, 500_000]) {
  const src = new Map<number, NodeDto[]>();
  const leaf = makeLeafNodes(5, 0);
  for (let i = 0; i < size; i++) src.set(i, leaf);

  bench(
    `Map copia di ${(size / 1000).toFixed(0)}k entries`,
    () => {
      const _m = new Map(src);
    },
    size > 200_000 ? 10 : 50
  );
}

// ── 3. pending.push(...chunk) — spread dei chunk ───────────────────────────────

console.log("\n=== pending.push(...chunk) — spread di chunk IPC ===");
for (const chunkSize of [100, 500, 1000, 5000]) {
  const chunk: [number, NodeDto[]][] = Array.from(
    { length: chunkSize },
    (_, i) => [i, makeLeafNodes(10, i * 10)]
  );
  bench(`spread chunk da ${chunkSize} entries in array pending`, () => {
    const pending: [number, NodeDto[]][] = [];
    pending.push(...chunk);
  });
}

// ── 4. Map insertions incrementali ────────────────────────────────────────────

console.log("\n=== Map.set incrementale (BFS accumulator) ===");
for (const [roots, children] of [
  [1000, 10],
  [10000, 10],
  [50000, 10]
] as [number, number][]) {
  const [, expandedNodes] = buildFlatTree(roots, children);
  const entries = [...expandedNodes.entries()];

  bench(`inserire ${(roots / 1000).toFixed(0)}k entries in Map vuota`, () => {
    const m = new Map<number, NodeDto[]>();
    for (const [k, v] of entries) m.set(k, v);
  });
}

// ── 5. Stima overhead totale per expand_all con N aggiornamenti ───────────────

console.log(
  "\n=== Overhead stimato per expand_all streaming (buildVisibleNodes + Map copy) ==="
);
const TOTAL_NODES = 100_000;
const [rc, en] = buildFlatTree(10_000, 10);

for (const updatesCount of [5, 10, 20, 50]) {
  const times: number[] = [];
  for (let rep = 0; rep < 20; rep++) {
    const t0 = performance.now();
    let current = new Map(en); // simula Map accumulata
    for (let u = 0; u < updatesCount; u++) {
      buildVisibleNodes(rc, current);
      current = new Map(current); // copia su ogni update
    }
    times.push(performance.now() - t0);
  }
  times.sort((a, b) => a - b);
  const median = times[Math.floor(times.length / 2)];
  console.log(
    `  ${TOTAL_NODES / 1000}k nodi, ${String(updatesCount).padStart(2)} UI updates: ${median.toFixed(0)}ms totali (simulazione)`
  );
}

console.log("\nDone.\n");
