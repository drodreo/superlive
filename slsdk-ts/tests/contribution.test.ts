// Contribution channel tests: registerControl (immutable, rule-checked) +
// mergeCatalog (entry fold + defs migration + reference integrity) + the
// full chain — custom control → registry → merged catalog → rendered surface
// → action out, plus the rejection paths on every gate.
import { describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { mergeCatalog } from "../src/ui/catalog/merge";
import { COMMON_TYPES_BASE } from "../src/ui/catalog/validate";
import type { CatalogComponentDefinition } from "../src/ui/catalog/types";
import {
  SurfaceModel,
  type CatalogDocument,
} from "../src/ui/a2ui/renderer/surface";
import type {
  ActionMessagePayload,
  ErrorMessagePayload,
  ServerToClientMessage,
} from "../src/ui/a2ui/types";
import {
  A2uiRegistry,
  bindAction,
  type A2uiComponentImpl,
  type ComponentRenderContext,
} from "../src/ui/a2ui/renderer/react/wiring";
import {
  createPresetRegistry,
  defaultRegistry,
} from "../src/ui/a2ui/renderer/registry";
import { A2uiSurface } from "../src/ui/a2ui/renderer/react/A2uiSurface";

// ===== the contributed control under test =====

/** A minimal Badge: label (dynamic) + variant (literal, via a local $def). */
const badgeImpl: A2uiComponentImpl = bindAction((ctx: ComponentRenderContext) =>
  createElement(
    "span",
    {
      className: `sl-test-badge sl-test-badge--${
        ctx.get<string>("variant") ?? "neutral"
      }`,
      onClick: () => ctx.dispatchAction("action"),
    },
    ctx.get<string>("text") ?? "",
  ),
);

const badgeDefs = {
  BadgeVariant: {
    type: "string",
    enum: ["neutral", "accent"],
    default: "neutral",
  },
};

const badgeSchema = {
  type: "object",
  allOf: [
    { $ref: `${COMMON_TYPES_BASE}#/$defs/ComponentCommon` },
    { $ref: "#/$defs/CatalogComponentCommon" },
    {
      type: "object",
      properties: {
        component: { const: "Badge" },
        text: {
          $ref: `${COMMON_TYPES_BASE}#/$defs/DynamicString`,
          description: "Badge label.",
        },
        variant: {
          $ref: "#/$defs/BadgeVariant",
          description: "Visual variant.",
        },
        action: {
          $ref: `${COMMON_TYPES_BASE}#/$defs/Action`,
          description: "Dispatched when the badge is activated.",
        },
      },
      required: ["component", "text"],
    },
  ],
  unevaluatedProperties: false,
} as unknown as CatalogComponentDefinition;

// The preset catalog document, deep-parsed fresh for each suite that needs it.
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const here = path.dirname(fileURLToPath(import.meta.url));
const catalogDocumentRaw = readFileSync(
  path.join(here, "../src/ui/catalog/superlive.catalog.json"),
  "utf-8",
);

function badgeDescription(): ServerToClientMessage[] {
  return [
    {
      version: "v0.9.1",
      createSurface: { surfaceId: "s1", catalogId: "superlive" },
    },
    {
      version: "v0.9.1",
      updateDataModel: {
        surfaceId: "s1",
        path: "/msg",
        value: "hello badge",
      },
    },
    {
      version: "v0.9.1",
      updateComponents: {
        surfaceId: "s1",
        components: [
          {
            id: "root",
            component: "Badge",
            text: { path: "/msg" },
            variant: "accent",
            action: { event: { name: "badgeAction" } },
          },
        ],
      },
    },
  ];
}

// ===== registerControl =====

describe("registerControl", () => {
  it("returns a fresh registry carrying the contribution, receiver untouched", () => {
    const base = createPresetRegistry();
    const size = base.size;
    const next = base.registerControl(
      "Badge",
      badgeImpl,
      badgeSchema,
      badgeDefs,
    );

    expect(next).not.toBe(base);
    expect(base.has("Badge")).toBe(false);
    expect(base.size).toBe(size);
    const entry = next.get("Badge");
    expect(entry?.render).toBeDefined();
    expect(entry?.schema).toBe(badgeSchema);
    expect(entry?.defs).toBe(badgeDefs);
    // The implementation handed in is not mutated either.
    expect(badgeImpl.schema).toBeUndefined();
    // Preset entries survive in the fork.
    expect(next.has("Button")).toBe(true);
  });

  it("rejects overriding an existing vocabulary member", () => {
    const base = createPresetRegistry();
    expect(() =>
      base.registerControl("Button", badgeImpl, badgeSchema),
    ).toThrow(/already registered/);
    // And on the contributed layer too.
    const forked = base.registerControl("Badge", badgeImpl, badgeSchema);
    expect(() =>
      forked.registerControl("Badge", badgeImpl, badgeSchema),
    ).toThrow(/already registered/);
  });

  it("rejects declarations violating the structural rules", () => {
    const base = createPresetRegistry();
    const cases: Array<[string, string, Record<string, unknown>]> = [
      [
        "no unevaluatedProperties closure",
        "must close with unevaluatedProperties:false",
        {
          type: "object",
          allOf: [
            { $ref: `${COMMON_TYPES_BASE}#/$defs/ComponentCommon` },
            { $ref: "#/$defs/CatalogComponentCommon" },
            {
              type: "object",
              properties: { component: { const: "Badge" } },
              required: ["component"],
            },
          ],
        },
      ],
      [
        "no allOf composition",
        "must use allOf",
        { type: "object", properties: { component: { const: "Badge" } } },
      ],
      [
        "const discriminator mismatch",
        'component const discriminator mismatch (expected "Badge")',
        {
          type: "object",
          allOf: [
            { $ref: `${COMMON_TYPES_BASE}#/$defs/ComponentCommon` },
            { $ref: "#/$defs/CatalogComponentCommon" },
            {
              type: "object",
              properties: { component: { const: "Chip" } },
            },
          ],
          unevaluatedProperties: false,
        },
      ],
      [
        "structural slot as a raw type",
        "must be a $ref, not a raw type",
        {
          type: "object",
          allOf: [
            { $ref: `${COMMON_TYPES_BASE}#/$defs/ComponentCommon` },
            { $ref: "#/$defs/CatalogComponentCommon" },
            {
              type: "object",
              properties: {
                component: { const: "Badge" },
                child: { type: "string" },
              },
            },
          ],
          unevaluatedProperties: false,
        },
      ],
      [
        "unexpected $ref base",
        "unexpected $ref base",
        {
          type: "object",
          allOf: [
            { $ref: `${COMMON_TYPES_BASE}#/$defs/ComponentCommon` },
            { $ref: "#/$defs/CatalogComponentCommon" },
            {
              type: "object",
              properties: {
                component: { const: "Badge" },
                text: { $ref: "https://example.com/x#/$defs/Y" },
              },
            },
          ],
          unevaluatedProperties: false,
        },
      ],
    ];
    for (const [label, message, schema] of cases) {
      expect(
        () =>
          base.registerControl(
            "Badge",
            badgeImpl,
            schema as unknown as CatalogComponentDefinition,
          ),
        label,
      ).toThrow(message);
    }
    // Nothing leaked into the registry through the failed attempts.
    expect(base.has("Badge")).toBe(false);
  });
});

// ===== mergeCatalog =====

describe("mergeCatalog", () => {
  it("folds the contribution into a fresh document, base untouched", () => {
    const base: CatalogDocument = JSON.parse(catalogDocumentRaw);
    const baseComponents = Object.keys(base.components);
    const registry = createPresetRegistry().registerControl(
      "Badge",
      badgeImpl,
      badgeSchema,
      badgeDefs,
    );

    const merged = mergeCatalog(base, registry);

    expect(merged).not.toBe(base);
    expect(base.components).not.toBe(merged.components);
    expect(Object.keys(base.components)).toEqual(baseComponents);
    expect(Object.keys(merged.components)).toEqual([
      ...baseComponents,
      "Badge",
    ]);
    expect(merged.components.Badge).toBe(badgeSchema);
    expect(merged.catalogId).toBe("superlive");
  });

  it("migrates the contributed $defs and keeps anyComponent exhaustive", () => {
    const base: CatalogDocument = JSON.parse(catalogDocumentRaw);
    const registry = createPresetRegistry().registerControl(
      "Badge",
      badgeImpl,
      badgeSchema,
      badgeDefs,
    );

    const merged = mergeCatalog(base, registry);

    expect(merged.$defs?.BadgeVariant).toBe(badgeDefs.BadgeVariant);
    expect(base.$defs?.BadgeVariant).toBeUndefined();
    const oneOf = (
      merged.$defs?.anyComponent as { oneOf: Array<{ $ref: string }> }
    ).oneOf;
    expect(oneOf.map((b) => b.$ref)).toContain("#/components/Badge");
    // The base oneOf entries survive.
    expect(oneOf.map((b) => b.$ref)).toContain("#/components/Button");
  });

  it("returns a shallow copy when there is nothing to contribute", () => {
    const base: CatalogDocument = JSON.parse(catalogDocumentRaw);
    const merged = mergeCatalog(base, createPresetRegistry());
    expect(merged).not.toBe(base);
    expect(merged.components).toEqual(base.components);
  });

  it("rejects component-name and $defs collisions", () => {
    const base: CatalogDocument = JSON.parse(catalogDocumentRaw);
    const registry = createPresetRegistry().registerControl(
      "Badge",
      badgeImpl,
      badgeSchema,
      badgeDefs,
    );
    expect(() => mergeCatalog(base, registry)).not.toThrow();

    const colliding = new A2uiRegistry();
    colliding.set("Button", { ...badgeImpl, schema: badgeSchema });
    expect(() => mergeCatalog(base, colliding)).toThrow(
      /collides with an existing base catalog entry/,
    );

    // A legally registered "Tag" whose local def key clashes with a base def.
    const tagOwnBranch = {
      ...(badgeSchema.allOf[2] as Record<string, unknown>),
      properties: {
        ...(badgeSchema.allOf[2] as { properties: Record<string, unknown> })
          .properties,
        component: { const: "Tag" },
      },
    };
    const tagSchema = {
      ...badgeSchema,
      allOf: [...badgeSchema.allOf.slice(0, 2), tagOwnBranch],
    } as unknown as CatalogComponentDefinition;
    const clashRegistry = createPresetRegistry().registerControl(
      "Tag",
      badgeImpl,
      tagSchema,
      { CatalogComponentCommon: { type: "object" } },
    );
    expect(() => mergeCatalog(base, clashRegistry)).toThrow(
      /collides with an existing definition/,
    );
  });

  it("rejects entries whose document-relative refs dangle", () => {
    const base: CatalogDocument = JSON.parse(catalogDocumentRaw);
    const dangling = {
      ...badgeSchema,
      allOf: [
        ...badgeSchema.allOf.slice(0, 2),
        {
          type: "object",
          properties: {
            component: { const: "Badge" },
            tone: { $ref: "#/$defs/NoSuchDef" },
          },
          required: ["component"],
        },
      ],
    } as unknown as CatalogComponentDefinition;
    const registry = createPresetRegistry().registerControl(
      "Badge",
      badgeImpl,
      dangling,
    );
    expect(() => mergeCatalog(base, registry)).toThrow(/does not resolve/);
  });
});

// ===== full chain: registry + merged catalog → rendered surface =====

describe("contributed control end to end", () => {
  const base: CatalogDocument = JSON.parse(catalogDocumentRaw);
  const contribRegistry = createPresetRegistry().registerControl(
    "Badge",
    badgeImpl,
    badgeSchema,
    badgeDefs,
  );
  const mergedCatalog = mergeCatalog(base, contribRegistry);

  it("evaluates and renders the contributed control through the real pipeline", () => {
    const surface = new SurfaceModel({ catalog: mergedCatalog });
    surface.apply(badgeDescription());
    const tree = surface.getTree();
    expect(tree?.component).toBe("Badge");
    const binding = surface.createNodeBinding(tree as NonNullable<typeof tree>);
    expect(binding.getSnapshot().text).toBe("hello badge");
    expect(binding.getSnapshot().variant).toBe("accent");
    binding.dispose();

    const html = renderToString(
      createElement(A2uiSurface, {
        description: badgeDescription(),
        registry: contribRegistry,
        catalog: mergedCatalog,
      }),
    );
    expect(html).toContain("sl-test-badge--accent");
    expect(html).toContain("hello badge");
  });

  it("dispatches the declared action to the host", () => {
    const onAction = vi.fn<(payload: ActionMessagePayload) => void>();
    const surface = new SurfaceModel({
      catalog: mergedCatalog,
      onAction,
    });
    surface.apply(badgeDescription());
    surface.dispatchAction(surface.getTree()!, "action");
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction).toHaveBeenCalledWith(
      expect.objectContaining({
        name: "badgeAction",
        surfaceId: "s1",
        sourceComponentId: "root",
      }),
    );
  });

  it("rejects a contributed name through the validate pass on the pure preset catalog", () => {
    const onError = vi.fn<(payload: ErrorMessagePayload) => void>();
    const surface = new SurfaceModel({ catalog: base, onError });
    surface.apply(badgeDescription());
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "VALIDATION_FAILED",
        message: expect.stringContaining('"Badge"'),
      }),
    );
    expect(surface.getTree()).toBeNull();
  });

  it("reports an unknown implementation when the registry lacks the control", () => {
    const onError = vi.fn<(payload: ErrorMessagePayload) => void>();
    const html = renderToString(
      createElement(A2uiSurface, {
        description: badgeDescription(),
        catalog: mergedCatalog,
        onError,
      }),
    );
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({
        code: "UNKNOWN_COMPONENT_IMPLEMENTATION",
      }),
    );
    expect(html).not.toContain("sl-test-badge");
  });

  it("keeps the shared preset registry and default catalog untouched", () => {
    expect(defaultRegistry.has("Badge")).toBe(false);
  });
});
