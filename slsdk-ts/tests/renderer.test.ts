// Renderer core tests (batch 2 / B2-1).
//
// The vitest environment is plain node (no jsdom, no @testing-library on the
// dependency whitelist), so coverage splits in two:
// - logic layer: data model, binding evaluation, two-pass application,
//   progressive materialization, write-back and action payloads — tested
//   directly against SurfaceModel / DataModel;
// - render smoke: react-dom/server renderToString runs the real projection
//   pipeline (hooks included) without a DOM.

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { renderToString } from "react-dom/server";
import { createElement } from "react";
import {
  DataModel,
  DataModelError,
  MAX_ARRAY_INDEX,
  SurfaceModel,
  type CatalogDocument,
  type SurfaceNode,
} from "../src/ui/a2ui/renderer";
import { A2uiSurface } from "../src/ui/a2ui/renderer/react/A2uiSurface";
import type {
  ActionMessagePayload,
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

function makeSurface(
  overrides: Partial<ConstructorParameters<typeof SurfaceModel>[0]> = {},
) {
  const onAction = vi.fn<(payload: ActionMessagePayload) => void>();
  const onError = vi.fn<(payload: ErrorMessagePayload) => void>();
  const surface = new SurfaceModel({
    catalog,
    onAction,
    onError,
    ...overrides,
  });
  return { surface, onAction, onError };
}

/**
 * Preset catalog extended with extra function declarations (deep copy): the
 * load-time FunctionCall vocabulary gate holds every declared call to the
 * catalog `functions` segment, so tests exercising host-provided functions
 * declare them here (official FunctionCall oneOf: catalog anyFunction).
 */
function catalogWithFunctions(extra: Record<string, unknown>): CatalogDocument {
  const copy = structuredClone(catalog) as CatalogDocument & {
    functions: Record<string, unknown>;
  };
  copy.functions = { ...(copy.functions ?? {}), ...extra };
  return copy;
}

/** Declaration shape for a simple single-argument string-returning test fn. */
function stringFnDecl(name: string, arg: string): Record<string, unknown> {
  return {
    type: "object",
    description: `Test helper function "${name}".`,
    properties: {
      call: { const: name },
      args: {
        type: "object",
        properties: {
          [arg]: { $ref: "#/$defs/DynamicString" },
        },
        required: [arg],
        unevaluatedProperties: false,
      },
      returnType: { const: "string" },
    },
    required: ["call", "args"],
    unevaluatedProperties: false,
  };
}

/** Description messages for a minimal Input surface bound to /model/name. */
function inputDescription(
  extra: ServerToClientMessage[] = [],
): ServerToClientMessage[] {
  return [
    {
      version: "v0.9.1",
      createSurface: { surfaceId: "s1", catalogId: "superlive" },
    },
    {
      version: "v0.9.1",
      updateDataModel: {
        surfaceId: "s1",
        path: "/model",
        value: { name: "a" },
      },
    },
    {
      version: "v0.9.1",
      updateComponents: {
        surfaceId: "s1",
        components: [
          {
            id: "root",
            component: "Input",
            value: { path: "/model/name" },
            placeholder: "type here",
          },
        ],
      },
    },
    ...extra,
  ];
}

// ===== data model =====

describe("DataModel", () => {
  it("sets and gets through auto-vivified object and array containers", () => {
    const model = new DataModel();
    model.set("/a/b/0", "x");
    expect(model.get("/a")).toEqual({ b: ["x"] });
    expect(model.get("/a/b/0")).toBe("x");
    model.set("/a/b/1", { deep: true });
    expect(model.get("/a/b/1/deep")).toBe(true);
  });

  it("replaces scalars sitting in the way of a deeper path", () => {
    const model = new DataModel();
    model.set("/a", "scalar");
    model.set("/a/b", 1);
    expect(model.get("/a/b")).toBe(1);
    model.set("/items/0/x", 7);
    expect(model.get("/items/0")).toEqual({ x: 7 });
  });

  it("honors RFC 6901 escapes (~0 and ~1)", () => {
    const model = new DataModel();
    model.set("/my~1key/sub~0x", "v");
    // "/my/key" would address two literal segments; the escaped form is one.
    expect(model.get("/my~1key")).toEqual({ "sub~x": "v" });
    expect(model.get("/my~1key/sub~0x")).toBe("v");
  });

  it("deletes object keys and splices array elements", () => {
    const model = new DataModel();
    model.set("/list", ["a", "b", "c"]);
    model.delete("/list/1");
    expect(model.get("/list")).toEqual(["a", "c"]);
    model.set("/obj", { x: 1, y: 2 });
    model.delete("/obj/x");
    expect(model.get("/obj")).toEqual({ y: 2 });
    model.delete("");
    expect(model.get("")).toEqual({}); // "" addresses the whole document
  });

  it("notifies either-direction prefix subscribers and skips unrelated ones", () => {
    const model = new DataModel();
    const parent = vi.fn();
    const child = vi.fn();
    const unrelated = vi.fn();
    model.subscribe("/items", parent);
    model.subscribe("/items/0/name", child);
    model.subscribe("/other", unrelated);

    model.set("/items/0/name", "n");
    expect(parent).toHaveBeenCalledTimes(1);
    expect(child).toHaveBeenCalledTimes(1);
    expect(unrelated).not.toHaveBeenCalled();

    // Replacing the whole subtree wakes the deep listener too.
    model.set("/items", [{ name: "m" }]);
    expect(parent).toHaveBeenCalledTimes(2);
    expect(child).toHaveBeenCalledTimes(2);
    expect(unrelated).not.toHaveBeenCalled();
  });

  it("rejects prototype-pollution keys in paths and payloads", () => {
    const model = new DataModel();
    expect(() => model.set("/__proto__/x", 1)).toThrow(DataModelError);
    expect(() => model.set("/constructor", 1)).toThrow(DataModelError);
    expect(() =>
      model.set("/safe", JSON.parse('{"__proto__": {"x": 1}}')),
    ).toThrow(DataModelError);
  });

  it("caps auto-vivification at MAX_ARRAY_INDEX", () => {
    const model = new DataModel();
    expect(MAX_ARRAY_INDEX).toBe(10000);
    expect(() => model.set(`/items/${MAX_ARRAY_INDEX + 1}`, 1)).toThrow(
      DataModelError,
    );
    expect(() => model.set("/items/9999", 1)).not.toThrow();
  });

  it("rejects malformed pointers", () => {
    const model = new DataModel();
    expect(() => model.get("items/0")).toThrow(DataModelError);
  });
});

// ===== binding evaluation =====

describe("binding evaluation", () => {
  it("passes literals through per family and reports type mismatches", () => {
    const { surface, onError } = makeSurface();
    surface.apply(inputDescription());
    const node = surface.getTree() as SurfaceNode;
    const binding = surface.createNodeBinding(node);
    // value is a binding; placeholder is a literal string.
    expect(binding.getSnapshot()).toEqual({
      value: "a",
      placeholder: "type here",
    });
    expect(onError).not.toHaveBeenCalled();
  });

  it("re-evaluates only when a dependent path changes, and only then notifies", () => {
    const { surface } = makeSurface();
    surface.apply(inputDescription());
    const node = surface.getTree() as SurfaceNode;
    const binding = surface.createNodeBinding(node);
    expect(binding.getDependencies()).toEqual(["/model/name"]);

    const onChange = vi.fn();
    binding.subscribe(onChange);

    // Unrelated write: subscription stays silent.
    surface.dataModel.set("/model/other", 1);
    expect(onChange).not.toHaveBeenCalled();

    // Same-value write: dependency hit, but the snapshot is unchanged.
    surface.dataModel.set("/model/name", "a");
    expect(onChange).not.toHaveBeenCalled();

    // Real change: notified with the new snapshot.
    surface.dataModel.set("/model/name", "b");
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(binding.getSnapshot().value).toBe("b");
  });

  it("resolves {call} bindings against the local function table with arg deps", () => {
    const fn = vi.fn((args: Record<string, unknown>) => `hi ${args.who}`);
    const { surface, onError } = makeSurface({
      catalog: catalogWithFunctions({ greet: stringFnDecl("greet", "who") }),
      functions: { greet: fn },
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/who", value: "agent" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Input",
              value: { call: "greet", args: { who: { path: "/who" } } },
            },
          ],
        },
      },
    ]);
    expect(onError).not.toHaveBeenCalled();
    const node = surface.getTree() as SurfaceNode;
    const binding = surface.createNodeBinding(node);
    expect(fn).toHaveBeenCalled();
    expect(binding.getSnapshot().value).toBe("hi agent");
    expect(binding.getDependencies()).toContain("/who");
  });

  it("rejects unknown functions at load time and rejects the whole batch", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "Input", value: { call: "nope" } },
          ],
        },
      },
    ]);
    // The FunctionCall vocabulary gate: "nope" is not declared in the
    // catalog functions segment, so the batch is rejected at load.
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "VALIDATION_FAILED",
        message: expect.stringContaining(
          'function "nope" is not declared in the catalog functions segment',
        ),
      }),
    );
    expect(surface.getTree()).toBeNull();
  });

  it("reports missing implementations through the error channel at binding time", () => {
    // "ghost" is declared in the catalog vocabulary but no implementation is
    // registered: load passes, resolution fails when the binding store is
    // created (render time).
    const { surface, onError } = makeSurface({
      catalog: catalogWithFunctions({ ghost: stringFnDecl("ghost", "k") }),
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Input",
              value: { call: "ghost", args: { k: "x" } },
            },
          ],
        },
      },
    ]);
    expect(surface.getTree()).not.toBeNull();
    surface.createNodeBinding(surface.getTree() as SurfaceNode);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "BINDING_EVALUATION_FAILED",
        message: expect.stringContaining('function "ghost" is not registered'),
      }),
    );
  });
});

// ===== two-pass application =====

describe("updateComponents two passes", () => {
  it("rejects unknown component names and applies nothing", () => {
    const { surface, onError } = makeSurface();
    surface.apply(inputDescription());
    const before = surface.getTree();
    surface.apply([
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "NotInVocabulary", value: "v" },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({ code: "VALIDATION_FAILED" }),
    );
    expect(surface.getTree()).toBe(before); // batch rejected wholesale
  });

  it("rejects duplicate ids within one batch", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "Input", value: "0" },
            { id: "a", component: "Input", value: "1" },
            { id: "a", component: "Input", value: "2" },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "VALIDATION_FAILED",
        message: expect.stringContaining('duplicate component id "a"'),
      }),
    );
    expect(surface.getTree()).toBeNull(); // "root" never landed
  });

  it("rejects missing required props and undeclared props", () => {
    const { surface, onError } = makeSurface();
    // Input requires "value".
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [{ id: "root", component: "Input", placeholder: "p" }],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        message: expect.stringContaining('required prop "value"'),
      }),
    );
    // Undeclared prop (unevaluatedProperties: false parity).
    surface.apply([
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "Input", value: "v", bogus: 1 },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        message: expect.stringContaining('undeclared prop "bogus"'),
      }),
    );
  });

  it("rejects malformed dynamic shapes but accepts binding forms", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [{ id: "root", component: "Input", value: { path: 42 } }],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({ code: "VALIDATION_FAILED" }),
    );

    surface.apply([
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Input",
              value: { path: "/x" },
              disabled: true,
            },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledTimes(1); // second batch accepted
    expect(surface.getTree()).not.toBeNull();
  });

  it("accepts common props declared through shared $refs, still typed", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              // CatalogComponentCommon (local $defs) and ComponentCommon
              // (official common_types.json) both contribute props.
              weight: 2,
              accessibility: { label: "submit" },
              action: { event: { name: "go" } },
            },
            { id: "label1", component: "ErrorMessage", error: "label text" },
          ],
        },
      },
    ]);
    expect(onError).not.toHaveBeenCalled();
    const tree = surface.getTree();
    expect(tree).not.toBeNull();
    expect(tree?.component).toBe("Button");
    expect(tree?.child?.componentId).toBe("label1");

    // The whitelisted prop is still typed: a non-number weight is rejected.
    surface.apply([
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              weight: "big",
              action: { event: { name: "go" } },
            },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({ code: "VALIDATION_FAILED" }),
    );
  });

  it("rejects a binding declaring both path and call", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Input",
              value: { path: "/x", call: "lookup" },
            },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "VALIDATION_FAILED",
        message: expect.stringContaining(
          'both "path" and "call": {"path":"/x","call":"lookup"}',
        ),
      }),
    );
    expect(surface.getTree()).toBeNull();
  });

  it("rejects a batch without the required root component", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "label1", component: "ErrorMessage", error: "orphan" },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "VALIDATION_FAILED",
        message: expect.stringContaining('"root" component'),
      }),
    );
    expect(surface.getTree()).toBeNull();
  });

  it("reports a catalog mismatch on createSurface", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "other-catalog" },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({ code: "CATALOG_MISMATCH" }),
    );
    expect(surface.surfaceId).toBeUndefined();
  });
});

// ===== progressive materialization =====

describe("progressive materialization", () => {
  it("leaves an optional childRef slot empty when undeclared", () => {
    // EmptyState.child is optional in the catalog; omitting it must not hold
    // the whole tree pending — the slot simply stays empty.
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "EmptyState", title: "nothing here" },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    expect(node?.component).toBe("EmptyState");
    expect(node?.child).toBeUndefined();
    expect(onError).not.toHaveBeenCalled();
  });

  function buttonDescription(): ServerToClientMessage[] {
    return [
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: { event: { name: "go" } },
            },
            {
              id: "label1",
              component: "ErrorMessage",
              error: "label text",
            },
          ],
        },
      },
    ];
  }

  it("keeps the tree pending until out-of-order definitions complete it", () => {
    const { surface } = makeSurface();
    const bumps = vi.fn();
    surface.subscribe(bumps);

    // Root first, referencing a definition that has not streamed in: the
    // branch stays pending (loading state).
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: { event: { name: "go" } },
            },
          ],
        },
      },
    ]);
    expect(surface.getTree()).toBeNull();
    expect(bumps).toHaveBeenCalled();

    // The referenced definition arrives (every batch carries root): the
    // branch materializes.
    surface.apply([
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: { event: { name: "go" } },
            },
            { id: "label1", component: "ErrorMessage", error: "label text" },
          ],
        },
      },
    ]);
    const tree = surface.getTree();
    expect(tree).not.toBeNull();
    expect(tree?.component).toBe("Button");
    expect(tree?.child?.componentId).toBe("label1");
  });

  it("upserting an identical declaration keeps table references stable", () => {
    const { surface } = makeSurface();
    surface.apply(buttonDescription());
    const first = surface.getTree() as SurfaceNode;
    surface.apply([
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: { event: { name: "go" } },
            },
          ],
        },
      },
    ]);
    const second = surface.getTree() as SurfaceNode;
    expect(second.propsSource).toBe(first.propsSource);
  });

  it("detects reference cycles instead of hanging", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "c1",
              action: { event: { name: "x" } },
            },
            { id: "a", component: "ErrorMessage", error: "x" },
            { id: "b", component: "ErrorMessage", error: "y" },
          ],
        },
      },
    ]);
    // Forge a cycle through the raw table via a fresh surface (the validate
    // pass would reject cross-references on Button since child must be a
    // string id — cycles across Button parents are what can happen).
    surface.apply([
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "c1",
              action: { event: { name: "x" } },
            },
            {
              id: "c1",
              component: "Button",
              child: "c2",
              action: { event: { name: "x" } },
            },
            {
              id: "c2",
              component: "Button",
              child: "c1",
              action: { event: { name: "x" } },
            },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({ code: "CYCLIC_REFERENCE" }),
    );
    expect(surface.getTree()).toBeNull();
  });
});

// ===== controlled wiring + action pipeline (logic layer) =====

describe("controlled wiring and action pipeline", () => {
  it("writeBack writes the bound path locally and never dispatches", () => {
    const { surface, onAction } = makeSurface();
    surface.apply(inputDescription());
    const node = surface.getTree() as SurfaceNode;
    expect(surface.writeBack(node, "value", "typed")).toBe(true);
    expect(surface.dataModel.get("/model/name")).toBe("typed");
    expect(onAction).not.toHaveBeenCalled();
    // Unbound slot: no-op.
    expect(surface.writeBack(node, "placeholder", "x")).toBe(false);
  });

  it("dispatches event actions with evaluated context payloads", () => {
    const { surface, onAction } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/row",
          value: { id: 42 },
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: {
                event: {
                  name: "todo.delete",
                  context: { id: { path: "/row/id" }, hard: false },
                },
              },
            },
            { id: "label1", component: "ErrorMessage", error: "del" },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    surface.dispatchAction(node, "action");
    expect(onAction).toHaveBeenCalledTimes(1);
    const payload = onAction.mock.calls[0][0];
    expect(payload.name).toBe("todo.delete");
    expect(payload.surfaceId).toBe("s1");
    expect(payload.sourceComponentId).toBe("root");
    expect(payload.timestamp).toEqual(expect.any(String));
    expect(payload.context).toEqual({ id: 42, hard: false });
  });

  it("executes functionCall actions locally without touching the host", () => {
    const fn = vi.fn();
    const { surface, onAction } = makeSurface({
      catalog: catalogWithFunctions({ local: stringFnDecl("local", "k") }),
      functions: { local: fn },
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: { functionCall: { call: "local", args: { k: "v" } } },
            },
            { id: "label1", component: "ErrorMessage", error: "x" },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    surface.dispatchAction(node, "action");
    expect(fn).toHaveBeenCalledWith({ k: "v" });
    expect(onAction).not.toHaveBeenCalled();
  });

  it("awaits async function implementations before settling", async () => {
    const fn = vi.fn((args: Record<string, unknown>) =>
      Promise.resolve(`done ${args.k}`),
    );
    const { surface } = makeSurface({
      catalog: catalogWithFunctions({ local: stringFnDecl("local", "k") }),
      functions: { local: fn },
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: {
                functionCall: { call: "local", args: { k: "x" } },
              },
            },
            { id: "label1", component: "ErrorMessage", error: "x" },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    surface.dispatchAction(node, "action");
    await vi.waitFor(() => expect(fn).toHaveBeenCalledTimes(1));
    expect(fn).toHaveBeenCalledWith({ k: "x" });
  });

  it("executes functionCall row actions with row-scoped argument bindings", async () => {
    const fn = vi.fn();
    const { surface } = makeSurface({
      catalog: catalogWithFunctions({ local: stringFnDecl("local", "id") }),
      functions: { local: fn },
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/rows",
          value: [
            { id: "a1", label: "first" },
            { id: "b2", label: "second" },
          ],
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "DataTable",
              rows: { path: "/rows" },
              columns: [{ key: "label", label: "Label" }],
              rowActions: [
                {
                  label: "run",
                  action: {
                    functionCall: {
                      call: "local",
                      args: { id: { path: "id" } },
                    },
                  },
                },
              ],
            },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    surface.dispatchRowAction(
      node,
      "rows",
      {
        functionCall: { call: "local", args: { id: { path: "id" } } },
      },
      1,
    );
    await vi.waitFor(() => expect(fn).toHaveBeenCalledTimes(1));
    // The relative {"path": "id"} resolved against row 1's scope.
    expect(fn).toHaveBeenCalledWith({ id: "b2" });
  });

  it("reports function execution failures through the error channel", async () => {
    const fn = vi.fn(() => {
      throw new Error("boom");
    });
    const { surface, onError } = makeSurface({
      catalog: catalogWithFunctions({ local: stringFnDecl("local", "k") }),
      functions: { local: fn },
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: {
                functionCall: { call: "local", args: { k: "x" } },
              },
            },
            { id: "label1", component: "ErrorMessage", error: "x" },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    surface.dispatchAction(node, "action");
    await vi.waitFor(() =>
      expect(onError).toHaveBeenCalledWith(
        expect.objectContaining({
          code: "FUNCTION_EXECUTION_FAILED",
          message: expect.stringContaining("boom"),
        }),
      ),
    );
  });

  it("evaluates literal object argument entries (body of computed values)", async () => {
    // The httpPatch body is a literal object whose entries are DynamicValues:
    // {"completed": {"call": "not", …}} ships the negated boolean.
    const fn = vi.fn();
    const { surface } = makeSurface({
      catalog: catalogWithFunctions({
        httpPatchLike: {
          type: "object",
          description: "Test helper mirroring the httpPatch body shape.",
          properties: {
            call: { const: "httpPatchLike" },
            args: {
              type: "object",
              properties: {
                path: { $ref: "#/$defs/DynamicString" },
                body: { type: "object" },
              },
              required: ["path"],
              unevaluatedProperties: false,
            },
            returnType: { const: "void" },
          },
          required: ["call", "args"],
          unevaluatedProperties: false,
        },
      }),
      functions: { httpPatchLike: fn },
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/row",
          value: { id: "t-1", completed: false },
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: {
                functionCall: {
                  call: "httpPatchLike",
                  args: {
                    path: { path: "/row/id" },
                    body: {
                      completed: {
                        call: "not",
                        args: { value: { path: "/row/completed" } },
                      },
                    },
                  },
                },
              },
            },
            { id: "label1", component: "ErrorMessage", error: "x" },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    surface.dispatchAction(node, "action");
    await vi.waitFor(() => expect(fn).toHaveBeenCalledTimes(1));
    // The body entry was evaluated (the predicate ran), not shipped verbatim.
    expect(fn).toHaveBeenCalledWith({
      path: "t-1",
      body: { completed: true },
    });
  });

  it("evaluates the builtin predicate vocabulary (not / eq)", async () => {
    const fn = vi.fn();
    const { surface } = makeSurface({
      catalog: catalogWithFunctions({
        probe: {
          type: "object",
          description: "Test helper carrying predicate args.",
          properties: {
            call: { const: "probe" },
            args: {
              type: "object",
              properties: {
                value: { $ref: "#/$defs/DynamicValue" },
              },
              required: ["value"],
              unevaluatedProperties: false,
            },
            returnType: { const: "void" },
          },
          required: ["call", "args"],
          unevaluatedProperties: false,
        },
      }),
      functions: { probe: fn },
    });
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/row",
          value: { completed: false, title: "x" },
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              action: {
                functionCall: {
                  call: "probe",
                  args: {
                    value: {
                      call: "eq",
                      args: {
                        left: {
                          call: "not",
                          args: { value: { path: "/row/completed" } },
                        },
                        right: true,
                      },
                    },
                  },
                },
              },
            },
            { id: "label1", component: "ErrorMessage", error: "x" },
          ],
        },
      },
    ]);
    const node = surface.getTree() as SurfaceNode;
    surface.dispatchAction(node, "action");
    await vi.waitFor(() => expect(fn).toHaveBeenCalledTimes(1));
    // not(false) === true, eq(true, true) === true.
    expect(fn).toHaveBeenCalledWith({ value: true });
  });

  it("rejects functionCall declarations violating their args schema at load", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Button",
              child: "label1",
              // "not" requires the "value" argument.
              action: { functionCall: { call: "not", args: {} } },
            },
            { id: "label1", component: "ErrorMessage", error: "x" },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "VALIDATION_FAILED",
        message: expect.stringContaining("does not match its declaration"),
      }),
    );
    expect(surface.getTree()).toBeNull();
  });

  it("updateDataModel applies increments and removals", () => {
    const { surface } = makeSurface();
    surface.apply(inputDescription());
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/model/name", value: "z" },
      },
    ]);
    expect(surface.dataModel.get("/model/name")).toBe("z");
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/model/name" },
      },
    ]);
    expect(surface.dataModel.get("/model/name")).toBeUndefined();
  });

  it("updateDataModel: omitted path and '/' both address the whole model", () => {
    const { surface } = makeSurface();
    surface.apply(inputDescription());
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          value: { model: { name: "whole" } },
        },
      },
    ]);
    expect(surface.dataModel.get("/model/name")).toBe("whole");
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/",
          value: { model: { name: "slash" } },
        },
      },
    ]);
    expect(surface.dataModel.get("/model/name")).toBe("slash");
    expect(surface.dataModel.get("/model")).toEqual({ name: "slash" });
  });

  it('wakes existing bindings on a whole-model update (omitted path and "/")', () => {
    const { surface } = makeSurface();
    surface.apply(inputDescription());
    const node = surface.getTree() as SurfaceNode;
    const binding = surface.createNodeBinding(node);
    expect(binding.getSnapshot().value).toBe("a");
    const onChange = vi.fn();
    binding.subscribe(onChange);

    // Omitted path = the entire model (spec: omission and "/" alike).
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          value: { model: { name: "whole" } },
        },
      },
    ]);
    expect(binding.getSnapshot().value).toBe("whole");
    expect(onChange).toHaveBeenCalled();

    // The "/" form likewise replaces the whole document.
    onChange.mockClear();
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/",
          value: { model: { name: "slash" } },
        },
      },
    ]);
    expect(binding.getSnapshot().value).toBe("slash");
    expect(onChange).toHaveBeenCalled();
  });

  it("deleteSurface clears the table and tree", () => {
    const { surface } = makeSurface();
    surface.apply(inputDescription());
    expect(surface.getTree()).not.toBeNull();
    surface.apply([{ version: "v0.9.1", deleteSurface: { surfaceId: "s1" } }]);
    expect(surface.getTree()).toBeNull();
  });

  it("deleteSurface wipes the data model: no stale data after recreation", () => {
    const { surface } = makeSurface();
    surface.apply(inputDescription());
    expect(surface.dataModel.get("/model/name")).toBe("a");
    surface.apply([{ version: "v0.9.1", deleteSurface: { surfaceId: "s1" } }]);
    expect(surface.dataModel.get("")).toEqual({});
    // Recreating the same surface id must not inherit previous data.
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
    ]);
    expect(surface.dataModel.get("/model")).toBeUndefined();
    expect(surface.dataModel.get("/model/name")).toBeUndefined();
  });
});

// ===== render smoke (react-dom/server, real pipeline, no DOM) =====

describe("A2uiSurface render smoke", () => {
  it("renders a bound Input through the real projection pipeline", () => {
    const html = renderToString(
      createElement(A2uiSurface, { description: inputDescription() }),
    );
    expect(html).toContain('value="a"');
    expect(html).toContain('placeholder="type here"');
  });

  it("renders the loading placeholder while root is pending", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "Button",
                  child: "kid",
                  action: { event: { name: "go" } },
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("sl-ui-a2ui-placeholder");
  });

  it("renders Button with its registered child subtree", () => {
    // The Text-pairing variant lives in the B2-2 block below; here the child
    // pipeline is verified with a stateful control to prove arbitrary
    // subtrees (not just display leaves) mount inside the slot.
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "Button",
                  child: "label1",
                  variant: "primary",
                  action: { event: { name: "go" } },
                },
                {
                  id: "label1",
                  component: "Input",
                  value: "seed",
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("sl-ui-btn--primary");
    // The child subtree renders inside the button label slot.
    expect(html).toContain('value="seed"');
  });

  it("is idempotent under double render (StrictMode simulation)", () => {
    const element = () =>
      createElement(A2uiSurface, { description: inputDescription() });
    const first = renderToString(element());
    const second = renderToString(element());
    expect(first).toBe(second);
    expect(first).toContain('value="a"');
  });
});

// ===== B2-2: ChildList template iteration =====

/** A todo-list description: List iterates /items with the rowText template. */
function iterationSurface() {
  const { surface, onAction, onError } = makeSurface();
  surface.apply([
    {
      version: "v0.9.1",
      createSurface: { surfaceId: "s1", catalogId: "superlive" },
    },
    {
      version: "v0.9.1",
      updateDataModel: {
        surfaceId: "s1",
        path: "/items",
        value: [
          { name: "alpha", done: true },
          { name: "beta", done: false },
          { name: "gamma", done: false },
        ],
      },
    },
    {
      version: "v0.9.1",
      updateComponents: {
        surfaceId: "s1",
        components: [
          {
            id: "root",
            component: "Modal",
            open: false,
            title: "list",
            children: { componentId: "rowText", path: "/items" },
          },
          {
            id: "rowText",
            component: "Text",
            // Relative: resolves against the iterated element per scope rules.
            text: { path: "name" },
          },
        ],
      },
    },
  ]);
  return { surface, onAction, onError };
}

describe("ChildList template iteration", () => {
  it("materializes one instance per element with the element scope burned in", () => {
    const { surface, onError } = iterationSurface();
    expect(onError).not.toHaveBeenCalled();
    const root = surface.getTree();
    expect(root).not.toBeNull();
    const rows = root?.childrenMap?.children ?? [];
    expect(rows).toHaveLength(3);
    expect(rows.map((r) => r.dataPath)).toEqual([
      "/items/0",
      "/items/1",
      "/items/2",
    ]);
    // All instances share the template component id; scope is the differentiator.
    expect(rows.every((r) => r.componentId === "rowText")).toBe(true);
  });

  it("resolves relative bindings against the element, absolute from the root", () => {
    const { surface } = iterationSurface();
    const rows = surface.getTree()?.childrenMap?.children ?? [];
    const snapshots = rows.map((row) =>
      Object.fromEntries(
        Object.entries(surface.createNodeBinding(row).getSnapshot()),
      ),
    );
    // Relative {path: "name"} → /items/N/name per instance.
    expect(snapshots.map((s) => s.text)).toEqual(["alpha", "beta", "gamma"]);

    // Absolute paths stay root-scoped inside an iteration subtree.
    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/title", value: "ROOT" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Modal",
              open: false,
              title: "list",
              children: { componentId: "rowText", path: "/items" },
            },
            { id: "rowText", component: "Text", text: { path: "/title" } },
          ],
        },
      },
    ]);
    const absolute = surface.getTree()?.childrenMap?.children ?? [];
    const values = absolute.map(
      (row) => surface.createNodeBinding(row).getSnapshot().text,
    );
    expect(values).toEqual(["ROOT", "ROOT", "ROOT"]);
  });

  it("grows and shrinks instances with the array length", () => {
    const { surface } = iterationSurface();
    expect(surface.getTree()?.childrenMap?.children).toHaveLength(3);

    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/items",
          value: [{ name: "alpha" }],
        },
      },
    ]);
    expect(surface.getTree()?.childrenMap?.children).toHaveLength(1);

    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/items",
          value: [
            { name: "alpha" },
            { name: "beta" },
            { name: "gamma" },
            { name: "delta" },
          ],
        },
      },
    ]);
    const rows = surface.getTree()?.childrenMap?.children ?? [];
    expect(rows).toHaveLength(4);
    expect(rows[3].dataPath).toBe("/items/3");
  });

  it("keeps the tree pending until the list data streams in", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Modal",
              open: false,
              title: "list",
              children: { componentId: "rowText", path: "/late" },
            },
            { id: "rowText", component: "Text", text: { path: "name" } },
          ],
        },
      },
    ]);
    // Components first, data later: the tree stays pending, no error.
    expect(surface.getTree()).toBeNull();
    expect(onError).not.toHaveBeenCalled();

    surface.apply([
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/late",
          value: [{ name: "x" }],
        },
      },
    ]);
    expect(surface.getTree()?.childrenMap?.children).toHaveLength(1);
  });

  it("reports a non-array iteration value through the validation channel", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/items", value: 42 },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Modal",
              open: false,
              title: "list",
              children: { componentId: "rowText", path: "/items" },
            },
            { id: "rowText", component: "Text", text: { path: "name" } },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0].code).toBe("VALIDATION_FAILED");
    expect(onError.mock.calls[0][0].message).toContain("/items");
  });

  it("rejects relative iteration paths in the root scope", () => {
    const { surface, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Modal",
              open: false,
              title: "list",
              children: { componentId: "rowText", path: "items" },
            },
            { id: "rowText", component: "Text", text: { path: "name" } },
          ],
        },
      },
    ]);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0].code).toBe("VALIDATION_FAILED");
    expect(onError.mock.calls[0][0].message).toContain("iteration scope");
  });

  it("writes controlled props back into the element subtree", () => {
    const { surface } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/items",
          value: [{ name: "a" }, { name: "b" }],
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Modal",
              open: false,
              title: "edit",
              children: { componentId: "rowInput", path: "/items" },
            },
            {
              id: "rowInput",
              component: "Input",
              value: { path: "name" },
            },
          ],
        },
      },
    ]);
    const rows = surface.getTree()?.childrenMap?.children ?? [];
    expect(rows).toHaveLength(2);
    surface.writeBack(rows[1], "value", "edited");
    expect(surface.dataModel.get("/items/1/name")).toBe("edited");
    expect(surface.dataModel.get("/items/0/name")).toBe("a");
  });
});

// ===== B2-2: full vocabulary wiring =====

describe("B2-2 wiring", () => {
  it("Text renders its evaluated content and variant", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateDataModel: {
              surfaceId: "s1",
              path: "/m",
              value: { line: "hello" },
            },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "Text",
                  text: { path: "/m/line" },
                  variant: "h3",
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("hello");
    expect(html).toContain("sl-ui-text--h3");
  });

  it("Button.child carries a Text label (the catalog's envisioned pairing)", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "Button",
                  child: "label1",
                  action: { event: { name: "go" } },
                },
                { id: "label1", component: "Text", text: "Save" },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("Save");
  });

  it("Select renders static options with the bound value selected", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateDataModel: {
              surfaceId: "s1",
              path: "/m",
              value: { pick: "b" },
            },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "Select",
                  value: { path: "/m/pick" },
                  options: [
                    { value: "a", label: "Alpha" },
                    { value: "b", label: "Beta" },
                  ],
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("Beta");
    expect(html).toContain('value="b"');
  });

  it("Pagination merges the selected page into the action context", () => {
    const { surface, onAction } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/m", value: { page: 2 } },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Pagination",
              page: { path: "/m/page" },
              total: 100,
              changeAction: { event: { name: "page.change" } },
            },
          ],
        },
      },
    ]);
    const node = surface.getTree();
    expect(node?.component).toBe("Pagination");
    surface.dispatchAction(node!, "changeAction", { page: 5 });
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0].name).toBe("page.change");
    expect(onAction.mock.calls[0][0].context).toEqual({ page: 5 });
  });

  it("LoadingSpinner passes size/message/overlay through", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "LoadingSpinner",
                  size: "sm",
                  message: "loading…",
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("sl-ui-loading-spinner--sm");
    expect(html).toContain("loading…");
  });

  it("EmptyState hosts its declared child as the action slot", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "EmptyState",
                  icon: "search",
                  title: "Nothing here",
                  description: "Try another query",
                  child: "cta",
                },
                {
                  id: "cta",
                  component: "Button",
                  child: "ctaLabel",
                  action: { event: { name: "reset" } },
                },
                { id: "ctaLabel", component: "Text", text: "Reset" },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("Nothing here");
    expect(html).toContain("Reset");
  });

  it("ErrorMessage renders the bound error and dispatches retry", () => {
    const { surface, onAction } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/m",
          value: { err: "boom" },
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "ErrorMessage",
              error: { path: "/m/err" },
              retryAction: { event: { name: "retry" } },
            },
          ],
        },
      },
    ]);
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "ErrorMessage",
                  error: "boom",
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("boom");
    surface.dispatchAction(surface.getTree()!, "retryAction");
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0].name).toBe("retry");
  });

  it("Modal materializes children and footer ChildLists; closed modal renders empty", () => {
    const { surface } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Modal",
              open: false,
              title: "dlg",
              children: ["body1"],
              footer: ["foot1"],
            },
            { id: "body1", component: "Text", text: "body" },
            {
              id: "foot1",
              component: "Button",
              child: "footLabel",
              action: { event: { name: "ok" } },
            },
            { id: "footLabel", component: "Text", text: "OK" },
          ],
        },
      },
    ]);
    const root = surface.getTree();
    expect(root?.childrenMap?.children).toHaveLength(1);
    expect(root?.childrenMap?.footer).toHaveLength(1);
    // Portal-based dialogs render nothing on the server while closed.
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "Modal",
                  open: false,
                  title: "dlg",
                  children: ["body1"],
                },
                { id: "body1", component: "Text", text: "body" },
              ],
            },
          },
        ],
      }),
    );
    expect(html).not.toContain("sl-ui-modal-backdrop");
  });

  it("ConfirmDialog dispatches confirm and cancel through the declared actions", () => {
    const { surface, onAction } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "ConfirmDialog",
              open: false,
              title: "delete?",
              message: "sure?",
              confirmAction: { event: { name: "confirm" } },
              cancelAction: { event: { name: "cancel" } },
            },
          ],
        },
      },
    ]);
    const node = surface.getTree()!;
    surface.dispatchAction(node, "confirmAction");
    surface.dispatchAction(node, "cancelAction");
    expect(onAction.mock.calls.map((c) => c[0].name)).toEqual([
      "confirm",
      "cancel",
    ]);
  });

  it("FormDialog accepts a formSchema and carries values in the submit context", () => {
    const { surface, onAction, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "FormDialog",
              open: false,
              title: "new item",
              formSchema: {
                fields: [
                  { key: "name", label: "Name", type: "input" },
                  {
                    key: "kind",
                    label: "Kind",
                    type: "select",
                    options: [{ value: "x", label: "X" }],
                  },
                ],
              },
              submitAction: { event: { name: "submit" } },
            },
          ],
        },
      },
    ]);
    // The schema passes validation verbatim (data-driven body, no error).
    expect(onError).not.toHaveBeenCalled();
    expect(surface.getTree()?.component).toBe("FormDialog");

    // Submit merges the (locally held) values into the event context.
    surface.dispatchAction(surface.getTree()!, "submitAction", {
      values: { name: "n", kind: "x" },
    });
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0].name).toBe("submit");
    expect(onAction.mock.calls[0][0].context).toEqual({
      values: { name: "n", kind: "x" },
    });
  });

  it("FormDialog renders a declared children body instead of the schema", () => {
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "FormDialog",
                  open: false,
                  title: "edit item",
                  children: ["field1"],
                  submitAction: { event: { name: "submit" } },
                },
                {
                  id: "field1",
                  component: "Input",
                  value: { path: "/form/name" },
                },
              ],
            },
          },
        ],
      }),
    );
    // Closed portal dialog: empty server output, but the description validated.
    expect(html).not.toContain("sl-ui-modal-backdrop");
  });

  it("QueryListToolbar passes the schema through and merges the filter into the action context", () => {
    const { surface, onAction, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "QueryListToolbar",
              querySchema: {
                fields: [
                  {
                    key: "title",
                    label: "Title",
                    type: "string",
                    operators: ["contains"],
                  },
                ],
              },
              filterChangeAction: { event: { name: "filter.change" } },
            },
          ],
        },
      },
    ]);
    expect(onError).not.toHaveBeenCalled();
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "QueryListToolbar",
                  querySchema: {
                    fields: [
                      {
                        key: "title",
                        label: "Title",
                        type: "string",
                        operators: ["contains"],
                      },
                    ],
                  },
                  filterChangeAction: { event: { name: "filter.change" } },
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("sl-ui-query");
    surface.dispatchAction(surface.getTree()!, "filterChangeAction", {
      filter: { conditions: [{ key: "title", op: "contains", value: "x" }] },
    });
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0].name).toBe("filter.change");
    expect(onAction.mock.calls[0][0].context).toEqual({
      filter: {
        conditions: [{ key: "title", op: "contains", value: "x" }],
      },
    });
  });

  it("DataTable renders data-driven columns over bound rows and returns the sort key", () => {
    const { surface, onAction } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/rows",
          value: [
            { name: "alpha", size: 3 },
            { name: "beta", size: 4 },
          ],
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "DataTable",
              columns: [
                { key: "name", label: "Name", sortable: true },
                { key: "size", label: "Size" },
              ],
              rows: { path: "/rows" },
              sortAction: { event: { name: "sort" } },
            },
          ],
        },
      },
    ]);
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateDataModel: {
              surfaceId: "s1",
              path: "/rows",
              value: [{ name: "alpha", size: 3 }],
            },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "DataTable",
                  columns: [
                    { key: "name", label: "Name" },
                    { key: "size", label: "Size" },
                  ],
                  rows: { path: "/rows" },
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("sl-ui-data-table");
    expect(html).toContain("alpha");
    expect(html).toContain("Size");

    surface.dispatchAction(surface.getTree()!, "sortAction", { key: "name" });
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0].name).toBe("sort");
    expect(onAction.mock.calls[0][0].context).toEqual({ key: "name" });
  });

  it("carries the declared context alongside extraContext (declared first, runtime wins)", () => {
    const { surface, onAction } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/m",
          value: { origin: "declared" },
        },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "Pagination",
              page: 1,
              total: 10,
              changeAction: {
                event: {
                  name: "page.change",
                  context: { origin: { path: "/m/origin" } },
                },
              },
            },
          ],
        },
      },
    ]);
    surface.dispatchAction(surface.getTree()!, "changeAction", {
      origin: "runtime",
      page: 7,
    });
    expect(onAction.mock.calls[0][0].context).toEqual({
      origin: "runtime",
      page: 7,
    });
  });
});

describe("DataTable rowActions (row-scoped dispatch)", () => {
  const TOGGLE_DECL = {
    event: {
      name: "todo.toggle",
      context: { id: { path: "id" }, completed: { path: "completed" } },
    },
  };

  function applyRowActionsSurface(
    surface: SurfaceModel,
    rows: unknown,
    rowActions: unknown,
  ): void {
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/todos", value: rows },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "DataTable",
              columns: [{ key: "title", label: "Title" }],
              rows: { path: "/todos" },
              rowActions,
            },
          ],
        },
      },
    ]);
  }

  it("renders the trailing operations column with per-row labels", () => {
    const { surface } = makeSurface();
    applyRowActionsSurface(
      surface,
      [
        { id: "a", title: "alpha", completed: false },
        { id: "b", title: "beta", completed: true },
      ],
      [{ label: "Delete", action: TOGGLE_DECL }],
    );
    const html = renderToString(
      createElement(A2uiSurface, {
        description: [
          {
            version: "v0.9.1",
            createSurface: { surfaceId: "s1", catalogId: "superlive" },
          },
          {
            version: "v0.9.1",
            updateDataModel: {
              surfaceId: "s1",
              path: "/todos",
              value: [{ id: "a", title: "alpha", completed: false }],
            },
          },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "root",
                  component: "DataTable",
                  columns: [{ key: "title", label: "Title" }],
                  rows: { path: "/todos" },
                  rowActions: [{ label: "Delete", action: TOGGLE_DECL }],
                },
              ],
            },
          },
        ],
      }),
    );
    expect(html).toContain("sl-ui-data-table-action");
    expect(html).toContain("Delete");
    expect(html).toContain("alpha");
  });

  it("dispatches with the row element as the evaluation scope", () => {
    const { surface, onAction } = makeSurface();
    applyRowActionsSurface(
      surface,
      [
        { id: "a", title: "alpha", completed: false },
        { id: "b", title: "beta", completed: true },
      ],
      [{ label: "Toggle", action: TOGGLE_DECL }],
    );
    const tree = surface.getTree();
    expect(tree).not.toBeNull();
    surface.dispatchRowAction(tree!, "rows", TOGGLE_DECL, 1);
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0].name).toBe("todo.toggle");
    // Relative pointers resolved against row 1 ("b"), not row 0 or the root.
    expect(onAction.mock.calls[0][0].context).toEqual({
      id: "b",
      completed: true,
    });
  });

  it("resolves labels against the row scope (relative binding and literal)", () => {
    const { surface } = makeSurface();
    applyRowActionsSurface(
      surface,
      [{ id: "a", title: "alpha", completed: false }],
      [{ label: { path: "title" }, action: TOGGLE_DECL }],
    );
    const tree = surface.getTree()!;
    expect(surface.resolveRowValue(tree, "rows", { path: "title" }, 0)).toBe(
      "alpha",
    );
    expect(surface.resolveRowValue(tree, "rows", "Literal", 0)).toBe("Literal");
  });

  it("reports and skips dispatch when rows is not a path binding", () => {
    const { surface, onAction, onError } = makeSurface();
    surface.apply([
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            {
              id: "root",
              component: "DataTable",
              columns: [{ key: "title", label: "Title" }],
              rows: [{ id: "a", title: "alpha" }],
              rowActions: [{ label: "Toggle", action: TOGGLE_DECL }],
            },
          ],
        },
      },
    ]);
    const tree = surface.getTree()!;
    surface.dispatchRowAction(tree, "rows", TOGGLE_DECL, 0);
    expect(onAction).not.toHaveBeenCalled();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0].code).toBe("VALIDATION_FAILED");
  });
});
