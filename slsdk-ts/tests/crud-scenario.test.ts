// CrudListPage scenario end-to-end (batch 2 closeout / B2-3).
//
// The full list-page description — toolbar + search + table + pagination +
// empty state over seeded data — exercises the renderer the way the batch-4
// host integration will: keystrokes stay local, assembled filters and page
// selections reach the host as event payloads, host data updates flow back
// through bindings.
//
// Two coverage layers (same environment constraints as renderer.test.ts —
// plain node, no jsdom):
// - logic layer: the whole tree under one Modal root (the catalog's only
//   ChildList container today; layout vocabulary is a catalog evolution),
//   driving interactions through SurfaceModel's writeBack/dispatchAction —
//   exactly the calls the registry glue issues from component callbacks;
// - render smoke: react-dom/server projections of the scene's portal-free
//   faces (toolbar / data table seeded and emptied). The Modal-rooted full
//   tree cannot render server-side (portal), the known B2-2 Dialog SSR
//   limitation — the logic layer covers that tree.

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { renderToString } from "react-dom/server";
import { createElement } from "react";
import {
  SurfaceModel,
  type CatalogDocument,
  type SurfaceNode,
} from "../src/ui/a2ui/renderer";
import { A2uiSurface } from "../src/ui/a2ui/renderer/react/A2uiSurface";
import type {
  ActionMessagePayload,
  A2uiComponentInstance,
  ErrorMessagePayload,
  ServerToClientMessage,
} from "../src/ui/a2ui/types";

const here = path.dirname(fileURLToPath(import.meta.url));
const catalog = JSON.parse(
  readFileSync(
    path.join(here, "../src/ui/catalog/superlive.catalog.json"),
    "utf-8",
  ),
) as unknown as CatalogDocument;

// ===== scenario fixtures =====

/** The entity's queryable-fields declaration (literal, per the catalog). */
const TODO_SCHEMA = {
  fields: [
    { key: "title", label: "标题", type: "string", operators: ["contains"] },
    { key: "done", label: "已完成", type: "boolean", operators: ["eq"] },
  ],
  defaultField: "title",
};

const SEED_ITEMS = [
  { id: "t1", title: "buy milk", done: false },
  { id: "t2", title: "write report", done: false },
  { id: "t3", title: "call alice", done: true },
];

const TODO_COLUMNS = [
  { key: "title", label: "标题" },
  { key: "done", label: "已完成" },
];

/**
 * The list-page tree. Root is a Modal (the only ChildList container in the
 * current catalog) holding the five scenario siblings in declared order.
 */
function scenarioComponents(): A2uiComponentInstance[] {
  return [
    {
      id: "root",
      component: "Modal",
      open: true,
      title: "待办列表",
      children: ["toolbar", "search", "table", "pager", "empty"],
    },
    {
      id: "toolbar",
      component: "QueryListToolbar",
      querySchema: TODO_SCHEMA,
      filterChangeAction: { event: { name: "todo.filter" } },
    },
    {
      id: "search",
      component: "SearchBar",
      value: { path: "/m/search" },
      placeholder: "quick search",
    },
    {
      id: "table",
      component: "DataTable",
      columns: TODO_COLUMNS,
      rows: { path: "/items" },
      emptyMessage: "暂无待办",
    },
    {
      id: "pager",
      component: "Pagination",
      page: { path: "/m/page" },
      total: { path: "/m/total" },
      changeAction: { event: { name: "todo.page" } },
    },
    {
      id: "empty",
      component: "EmptyState",
      icon: "data",
      title: "暂无待办",
      description: "all caught up",
    },
  ];
}

function scenarioDescription(
  items: typeof SEED_ITEMS = SEED_ITEMS,
): ServerToClientMessage[] {
  return [
    {
      version: "v0.9.1",
      createSurface: { surfaceId: "s1", catalogId: "superlive" },
    },
    // Seeding goes through explicit absolute pointers — the spec treats an
    // omitted path and "/" alike as the entire model, so subtrees are
    // addressed through their own pointers like "/items".
    {
      version: "v0.9.1",
      updateDataModel: { surfaceId: "s1", path: "/items", value: items },
    },
    {
      version: "v0.9.1",
      updateDataModel: {
        surfaceId: "s1",
        path: "/m",
        value: { search: "", page: 1, total: items.length },
      },
    },
    {
      version: "v0.9.1",
      updateComponents: {
        surfaceId: "s1",
        components: scenarioComponents(),
      },
    },
  ];
}

function makeScenario(items: typeof SEED_ITEMS = SEED_ITEMS) {
  const onAction = vi.fn<(payload: ActionMessagePayload) => void>();
  const onError = vi.fn<(payload: ErrorMessagePayload) => void>();
  const surface = new SurfaceModel({ catalog, onAction, onError });
  surface.apply(scenarioDescription(items));
  return { surface, onAction, onError };
}

/** Depth-first lookup across child / ChildList slots of the resolved tree. */
function nodeOf(surface: SurfaceModel, id: string): SurfaceNode {
  const visit = (node: SurfaceNode): SurfaceNode | undefined => {
    if (node.componentId === id) return node;
    if (node.child) {
      const hit = visit(node.child);
      if (hit) return hit;
    }
    for (const list of Object.values(node.childrenMap ?? {})) {
      for (const child of list) {
        const hit = visit(child);
        if (hit) return hit;
      }
    }
    return undefined;
  };
  const root = surface.getTree();
  expect(root).not.toBeNull();
  const found = visit(root as SurfaceNode);
  expect(found, `component "${id}" should resolve in the tree`).toBeDefined();
  return found as SurfaceNode;
}

// ===== logic layer: the whole scenario over the resolved tree =====

describe("crud list scenario (logic layer)", () => {
  it("resolves the full list-page tree with bindings wired", () => {
    const { surface, onError } = makeScenario();
    const root = surface.getTree() as SurfaceNode;
    expect(root.component).toBe("Modal");
    const siblings = root.childrenMap?.children ?? [];
    expect(siblings.map((n) => n.component)).toEqual([
      "QueryListToolbar",
      "SearchBar",
      "DataTable",
      "Pagination",
      "EmptyState",
    ]);
    // Data binding declarations survive resolution verbatim.
    expect(nodeOf(surface, "table").propsSource.rows).toEqual({
      path: "/items",
    });
    expect(nodeOf(surface, "pager").propsSource.page).toEqual({
      path: "/m/page",
    });
    expect(onError).not.toHaveBeenCalled();
  });

  it("keeps search keystrokes local: write-back lands in the data model, nothing dispatches", () => {
    const { surface, onAction } = makeScenario();
    const search = nodeOf(surface, "search");
    expect(surface.writeBack(search, "value", "buy milk")).toBe(true);
    expect(surface.dataModel.get("/m/search")).toBe("buy milk");
    expect(onAction).not.toHaveBeenCalled();
  });

  it("delivers the assembled filter through filterChangeAction's context", () => {
    const { surface, onAction } = makeScenario();
    const filter = {
      conditions: [
        {
          field: "title",
          operator: "contains",
          reversed: false,
          value: { type: "single", data: "buy" },
        },
      ],
      sorting: undefined,
      includeDeleted: false,
    };
    surface.dispatchAction(nodeOf(surface, "toolbar"), "filterChangeAction", {
      filter,
    });
    expect(onAction).toHaveBeenCalledTimes(1);
    const payload = onAction.mock.calls[0][0];
    expect(payload.name).toBe("todo.filter");
    expect(payload.sourceComponentId).toBe("toolbar");
    expect(payload.context).toEqual({ filter });
  });

  it("follows host data updates through the binding (rows shrink without a tree rebuild)", () => {
    const { surface } = makeScenario();
    const table = nodeOf(surface, "table");
    const binding = surface.createNodeBinding(table);
    expect((binding.getSnapshot().rows as unknown[]).length).toBe(
      SEED_ITEMS.length,
    );

    const versionBefore = surface.getVersion();
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/items",
          value: [SEED_ITEMS[0]],
        },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/m/total", value: 1 },
      },
    ]);
    expect(binding.getSnapshot().rows).toEqual([SEED_ITEMS[0]]);
    // Binding-driven: the tree shape did not change for a content-only update.
    expect(surface.getVersion()).toBe(versionBefore);
  });

  it("drains to the empty branch: zero rows, empty state still mounted, no errors", () => {
    const { surface, onError } = makeScenario();
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/items", value: [] },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/m/total", value: 0 },
      },
    ]);
    const binding = surface.createNodeBinding(nodeOf(surface, "table"));
    expect(binding.getSnapshot().rows).toEqual([]);
    // The empty-state fallback stays part of the tree for the host to show.
    expect(nodeOf(surface, "empty").component).toBe("EmptyState");
    expect(onError).not.toHaveBeenCalled();
  });

  it("carries the selected page through write-back and the changeAction context", () => {
    const { surface, onAction } = makeScenario();
    const pager = nodeOf(surface, "pager");
    // The registry glue writes the page back locally, then dispatches.
    expect(surface.writeBack(pager, "page", 2)).toBe(true);
    expect(surface.dataModel.get("/m/page")).toBe(2);
    surface.dispatchAction(pager, "changeAction", { page: 2 });
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0].name).toBe("todo.page");
    expect(onAction.mock.calls[0][0].context).toEqual({ page: 2 });
  });
});

// ===== render smoke: portal-free faces of the same scenario =====

describe("crud list scenario (render smoke)", () => {
  const msg = (m: object) =>
    ({ version: "v0.9.1", ...m }) as ServerToClientMessage;

  function tableDescription(items: typeof SEED_ITEMS): ServerToClientMessage[] {
    return [
      msg({ createSurface: { surfaceId: "s1", catalogId: "superlive" } }),
      msg({
        updateDataModel: {
          surfaceId: "s1",
          path: "/items",
          value: items,
        },
      }),
      msg({
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "DataTable",
              columns: TODO_COLUMNS,
              rows: { path: "/items" },
              emptyMessage: "暂无待办",
            },
          ],
        },
      }),
    ];
  }

  it("renders the toolbar face with the schema-driven search placeholder", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          msg({ createSurface: { surfaceId: "s1", catalogId: "superlive" } }),
          msg({
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "QueryListToolbar",
                  querySchema: TODO_SCHEMA,
                  filterChangeAction: { event: { name: "todo.filter" } },
                },
              ],
            },
          }),
        ],
      }),
    );
    expect(html).toContain("按标题搜索…");
    expect(html).toContain("排序");
  });

  it("renders the seeded table face with every row", () => {
    const html = renderToString(
      createElement(A2uiSurface, { description: tableDescription(SEED_ITEMS) }),
    );
    expect(html).toContain("标题");
    for (const item of SEED_ITEMS) {
      expect(html).toContain(item.title);
    }
  });

  it("renders the empty branch of the table face", () => {
    const html = renderToString(
      createElement(A2uiSurface, { description: tableDescription([]) }),
    );
    expect(html).toContain("暂无待办");
    expect(html).not.toContain("buy milk");
  });
});
