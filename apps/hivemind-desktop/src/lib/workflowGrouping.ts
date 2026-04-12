/**
 * A node in a hierarchical namespace tree.
 *
 * Items are placed in the node whose `fullPath` matches their parent namespace
 * (all segments except the last one).  Intermediate nodes that have no direct
 * items may still exist to connect deeper sub-trees.
 */
export interface NamespaceNode<T> {
  /** This level's segment name (e.g. `"software"`). */
  segment: string;
  /** Full slash-separated path from root (e.g. `"system/software"`). */
  fullPath: string;
  /** Items whose parent namespace is exactly this path, sorted alphabetically. */
  items: T[];
  /** Sorted child namespace nodes. */
  children: NamespaceNode<T>[];
}

/**
 * Build a hierarchical namespace tree from a flat list of items.
 *
 * Each item's namespaced key is split on `/`.  The *parent namespace* (all
 * segments except the last) determines where the item is placed in the tree.
 * Intermediate nodes are created as needed so that the tree is fully connected.
 *
 * @param items      Array of items to group.
 * @param getKey     Accessor for the namespaced key (defaults to `item.name`).
 * @param getSortKey Optional accessor for the sort key within each namespace
 *                   (defaults to `getKey`).
 * @returns Sorted array of root-level `NamespaceNode` objects.
 */
export function buildNamespaceTree<T>(
  items: T[],
  getKey?: (item: T) => string,
  getSortKey?: (item: T) => string,
): NamespaceNode<T>[] {
  const accessor = getKey ?? ((item: T) => (item as any).name as string);
  const sortAccessor = getSortKey ?? accessor;

  // 1. Group items by their parent namespace path.
  const nsItems = new Map<string, T[]>();
  for (const item of items) {
    const key = accessor(item);
    const segments = key.split('/');
    const ns = segments.length > 1 ? segments.slice(0, -1).join('/') : 'other';
    if (!nsItems.has(ns)) nsItems.set(ns, []);
    nsItems.get(ns)!.push(item);
  }

  // Sort items within each namespace alphabetically.
  for (const [, group] of nsItems) {
    group.sort((a, b) => sortAccessor(a).localeCompare(sortAccessor(b)));
  }

  // 2. Collect every namespace path that must exist (including intermediates).
  const allPaths = new Set<string>();
  for (const path of nsItems.keys()) {
    const segments = path.split('/');
    for (let i = 1; i <= segments.length; i++) {
      allPaths.add(segments.slice(0, i).join('/'));
    }
  }

  // 3. Create a node for each path.
  const nodeMap = new Map<string, NamespaceNode<T>>();
  for (const path of allPaths) {
    const segments = path.split('/');
    nodeMap.set(path, {
      segment: segments[segments.length - 1],
      fullPath: path,
      items: nsItems.get(path) ?? [],
      children: [],
    });
  }

  // 4. Wire parent → child relationships and collect roots.
  const roots: NamespaceNode<T>[] = [];
  const sortedPaths = Array.from(allPaths).sort();
  for (const path of sortedPaths) {
    const node = nodeMap.get(path)!;
    const segments = path.split('/');
    if (segments.length > 1) {
      const parentPath = segments.slice(0, -1).join('/');
      const parent = nodeMap.get(parentPath);
      if (parent) {
        parent.children.push(node);
        continue;
      }
    }
    roots.push(node);
  }

  // 5. Sort children at every level.
  for (const node of nodeMap.values()) {
    node.children.sort((a, b) => a.segment.localeCompare(b.segment));
  }

  roots.sort((a, b) => a.segment.localeCompare(b.segment));
  return roots;
}

/**
 * Flatten a namespace tree into `[fullPath, items[]]` tuples in depth-first
 * order.  Useful for `<select>` / `<optgroup>` elements where HTML does not
 * support nesting.  Empty intermediate groups are omitted.
 */
export function flattenNamespaceTree<T>(
  roots: NamespaceNode<T>[],
): [string, T[]][] {
  const result: [string, T[]][] = [];
  function walk(nodes: NamespaceNode<T>[]) {
    for (const node of nodes) {
      if (node.items.length > 0) {
        result.push([node.fullPath, node.items]);
      }
      walk(node.children);
    }
  }
  walk(roots);
  return result;
}

/**
 * Collect every `fullPath` in the tree (depth-first).  Useful for
 * "expand all namespaces" initialisation.
 */
export function collectAllPaths<T>(roots: NamespaceNode<T>[]): Set<string> {
  const paths = new Set<string>();
  function walk(nodes: NamespaceNode<T>[]) {
    for (const node of nodes) {
      paths.add(node.fullPath);
      walk(node.children);
    }
  }
  walk(roots);
  return paths;
}


