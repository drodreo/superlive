// superlive catalog validation tests.
//
// No JSON-Schema library on the dependency whitelist, so a minimal validator
// for the keyword subset actually used by the catalog lives below: type / const
// / enum / properties / required / additionalProperties / unevaluatedProperties
// / allOf / oneOf / items / $ref (+ ignored discriminator annotation).
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  COMMON_TYPES_BASE,
  collectRefs,
  createCatalogValidator,
  validateComponentEntry,
} from "../src/ui/catalog/validate";

const here = path.dirname(fileURLToPath(import.meta.url));
const catalog = JSON.parse(
  readFileSync(
    path.join(here, "../src/ui/catalog/superlive.catalog.json"),
    "utf-8",
  ),
) as Record<string, unknown>;
// Spec fixture: copy of the A2UI v0.9 common_types.json snapshot
// (whitespace-normalized, definitions untouched).
const commonTypes = JSON.parse(
  readFileSync(path.join(here, "./fixtures/common-types.schema.json"), "utf-8"),
) as Record<string, unknown>;

const { validate } = createCatalogValidator({ document: catalog, commonTypes });

type Schema = Record<string, unknown>;

function componentNames(): string[] {
  return Object.keys(catalog.components as object);
}

describe("catalog file shape", () => {
  it("is a valid A2UI catalog JSON Schema document", () => {
    expect(catalog.$schema).toBe(
      "https://json-schema.org/draft/2020-12/schema",
    );
    expect(typeof catalog.$id).toBe("string");
    expect(catalog.catalogId).toBe("superlive");
    expect(catalog.components).toBeTypeOf("object");
    const defs = catalog.$defs as Record<string, unknown>;
    for (const def of [
      "CatalogComponentCommon",
      "anyComponent",
      "SelectOption",
      "FileAutoFill",
      "FormFieldSchema",
      "FormSchema",
      "QueryFieldSpec",
      "QuerySchema",
    ]) {
      expect(defs[def], `missing $defs/${def}`).toBeTypeOf("object");
    }
  });

  it("declares exactly the 16 designed components", () => {
    expect(componentNames().sort()).toEqual(
      [
        "Button",
        "Checkbox",
        "ConfirmDialog",
        "DataTable",
        "EmptyState",
        "ErrorMessage",
        "FormDialog",
        "Input",
        "LoadingSpinner",
        "Modal",
        "Pagination",
        "QueryListToolbar",
        "SearchBar",
        "Select",
        "Text",
        "Textarea",
      ].sort(),
    );
  });

  it("only references the catalog itself and the official common types", () => {
    for (const ref of collectRefs(catalog)) {
      const base = ref.split("#")[0];
      expect(
        [COMMON_TYPES_BASE, ""],
        `unexpected $ref base in "${ref}"`,
      ).toContain(base);
    }
  });
});

describe("component declaration hard rules", () => {
  it("every component is closed and const-discriminated", () => {
    const components = catalog.components as Record<string, Schema>;
    for (const [name, def] of Object.entries(components)) {
      expect(
        def.unevaluatedProperties,
        `${name}: must close with unevaluatedProperties:false`,
      ).toBe(false);
      expect(Array.isArray(def.allOf), `${name}: must use allOf`).toBe(true);
      const branches = def.allOf as Schema[];
      expect(
        (branches[0] as Schema).$ref,
        `${name}: first allOf branch must be ComponentCommon`,
      ).toBe(`${COMMON_TYPES_BASE}#/$defs/ComponentCommon`);
      expect(
        (branches[1] as Schema).$ref,
        `${name}: second allOf branch must be CatalogComponentCommon`,
      ).toBe("#/$defs/CatalogComponentCommon");
      const own = branches
        .filter((b) => !("$ref" in b))
        .find((b) => b.properties);
      expect(own, `${name}: must have own properties branch`).toBeDefined();
      const componentProp = (own!.properties as Record<string, Schema>)
        .component;
      expect(
        componentProp?.const,
        `${name}: component const discriminator mismatch`,
      ).toBe(name);
    }
  });

  it("structural child references use $ref ComponentId / ChildList, never raw strings", () => {
    const structuralProps = new Set(["child", "children", "footer"]);
    const components = catalog.components as Record<string, Schema>;
    for (const [name, def] of Object.entries(components)) {
      for (const branch of def.allOf as Schema[]) {
        const props = branch.properties as Record<string, Schema> | undefined;
        if (!props) continue;
        for (const [propName, propSchema] of Object.entries(props)) {
          if (!structuralProps.has(propName)) continue;
          const ref = propSchema.$ref as string | undefined;
          expect(
            ref,
            `${name}.${propName}: must be a $ref, not a raw type`,
          ).toBeDefined();
          expect(
            ["ComponentId", "ChildList"].some((tail) =>
              (ref as string).endsWith(`#/$defs/${tail}`),
            ),
            `${name}.${propName}: $ref must target ComponentId or ChildList, got ${ref}`,
          ).toBe(true);
          expect(
            propSchema.type,
            `${name}.${propName}: must not declare a raw string type`,
          ).toBeUndefined();
        }
      }
    }
  });

  it("anyComponent covers every declared component", () => {
    const anyComponent = (catalog.$defs as Record<string, Schema>).anyComponent;
    const refs = (anyComponent.oneOf as Schema[]).map((b) =>
      (b.$ref as string).split("/").pop(),
    );
    expect(refs.sort()).toEqual(componentNames().sort());
  });

  it("the shared declaration validator agrees on every preset entry", () => {
    for (const [name, def] of Object.entries(
      catalog.components as Record<string, Schema>,
    )) {
      expect(validateComponentEntry(name, def), name).toEqual([]);
    }
  });
});

describe("functions segment hard rules", () => {
  const functions = catalog.functions as Record<string, Schema>;

  it("declares exactly the designed function vocabulary", () => {
    expect(Object.keys(functions).sort()).toEqual(
      [
        "httpDelete",
        "httpGet",
        "httpPatch",
        "httpPost",
        "httpPut",
        "eq",
        "not",
      ].sort(),
    );
  });

  it("every function declaration is closed, const-discriminated and returns", () => {
    for (const [name, def] of Object.entries(functions)) {
      expect(def.type, `${name}: must declare type "object"`).toBe("object");
      expect(
        def.unevaluatedProperties,
        `${name}: must close with unevaluatedProperties:false`,
      ).toBe(false);
      const props = def.properties as Record<string, Schema>;
      expect(
        (props.call as Schema).const,
        `${name}: call must be const-discriminated`,
      ).toBe(name);
      expect(
        (props.returnType as Schema).const,
        `${name}: returnType must be const`,
      ).toBeTypeOf("string");
      const args = props.args as Schema;
      expect(
        (def.required as string[]).slice().sort(),
        `${name}: must require call + args`,
      ).toEqual(["args", "call"]);
      expect(
        args.unevaluatedProperties,
        `${name}.args: must close with unevaluatedProperties:false`,
      ).toBe(false);
    }
  });

  it("anyFunction covers every declared function", () => {
    const anyFunction = (catalog.$defs as Record<string, Schema>).anyFunction;
    const refs = (anyFunction.oneOf as Schema[]).map((b) =>
      (b.$ref as string).split("/").pop(),
    );
    expect(refs.sort()).toEqual(Object.keys(functions).sort());
  });

  it("the official Dynamic*/FunctionCall/DataBinding defs ride in $defs", () => {
    const defs = catalog.$defs as Record<string, Schema>;
    for (const def of [
      "DataBinding",
      "DynamicValue",
      "DynamicString",
      "DynamicBoolean",
      "FunctionCall",
    ]) {
      expect(defs[def], `missing $defs/${def}`).toBeTypeOf("object");
    }
    // The official FunctionCall constraint: the call name must be declared
    // in this catalog's anyFunction (oneOf), re-anchored to this document.
    const functionCall = defs.FunctionCall;
    expect((functionCall.oneOf as Schema[])[0].$ref).toBe(
      "#/$defs/anyFunction",
    );
  });
});

describe("function call instances validate against the catalog", () => {
  const functions = catalog.functions as Record<string, Schema>;
  const validator = createCatalogValidator({
    document: catalog as Record<string, unknown>,
  });

  const validateCall = (decl: unknown): string[] => {
    const call = (decl as { call?: unknown }).call;
    const schema = functions[call as string];
    if (!schema) return [`function "${String(call)}" is not declared`];
    return validator.validate(schema, decl, "functions");
  };

  it("accepts well-formed declarations", () => {
    expect(
      validateCall({ call: "httpGet", args: { path: "api/v1/items" } }),
    ).toEqual([]);
    expect(
      validateCall({
        call: "httpDelete",
        args: { path: { path: "id" }, query: { force: true } },
      }),
    ).toEqual([]);
    expect(
      validateCall({
        call: "httpPatch",
        args: {
          path: { path: "id" },
          body: {
            completed: { call: "not", args: { value: { path: "completed" } } },
          },
        },
      }),
    ).toEqual([]);
    expect(
      validateCall({ call: "not", args: { value: { path: "completed" } } }),
    ).toEqual([]);
    expect(
      validateCall({
        call: "eq",
        args: { left: { path: "a" }, right: "x" },
      }),
    ).toEqual([]);
  });

  it("rejects unknown function names", () => {
    expect(validateCall({ call: "nope" })).toContain(
      'function "nope" is not declared',
    );
  });

  it("rejects missing required args and unknown args", () => {
    // httpGet requires the "path" argument.
    const missing = validateCall({ call: "httpGet", args: {} });
    expect(missing.join(" ")).toContain('missing required property "path"');
    // httpGet does not declare a "body" argument.
    const extra = validateCall({
      call: "httpGet",
      args: { path: "x", body: {} },
    });
    expect(extra.join(" ")).toContain("unevaluated property not allowed");
    // "not" requires the "value" argument.
    const noValue = validateCall({ call: "not", args: {} });
    expect(noValue.join(" ")).toContain('missing required property "value"');
  });

  it("rejects wrong argument types", () => {
    // httpGet path is a DynamicString — a boolean literal is neither branch.
    const bad = validateCall({ call: "httpGet", args: { path: true } });
    expect(bad.length).toBeGreaterThan(0);
  });
});

describe("instance samples validate against the catalog", () => {
  const components = catalog.components as Record<string, Schema>;

  it("Button with an action event", () => {
    const errors = validate(
      components.Button,
      {
        id: "submit_btn",
        component: "Button",
        child: "submit_label",
        variant: "primary",
        loading: false,
        action: {
          event: { name: "submitContact", context: { source: "toolbar" } },
        },
      },
      "$",
    );
    expect(errors).toEqual([]);
  });

  it("rowActions embedding functionCall actions (cross-document refs)", () => {
    // The official common-types Action → FunctionCall chain references
    // "catalog.json#/$defs/anyFunction", and anyFunction's branches use
    // catalog-relative refs (#/functions/*). The validator must resolve the
    // literal catalog.json base to this catalog and keep the nested relative
    // refs resolving against the catalog document, not the common-types doc.
    const errors = validate(
      components.DataTable,
      {
        id: "root",
        component: "DataTable",
        columns: [{ key: "title", label: "Title" }],
        rows: { path: "/todos" },
        emptyMessage: "Nothing here",
        rowActions: [
          {
            label: "Delete",
            action: {
              functionCall: {
                call: "httpDelete",
                args: { path: { path: "self" } },
              },
            },
          },
          {
            label: "Toggle",
            action: {
              functionCall: {
                call: "httpPatch",
                args: {
                  path: { path: "self" },
                  body: {
                    completed: {
                      call: "not",
                      args: { value: { path: "completed" } },
                    },
                  },
                },
              },
            },
          },
        ],
      },
      "$",
    );
    expect(errors).toEqual([]);
  });

  it("Input with a DynamicString data binding", () => {
    const errors = validate(
      components.Input,
      {
        id: "email_input",
        component: "Input",
        value: { path: "/form/email" },
        type: "email",
        placeholder: "Email address",
        error: "Invalid email",
      },
      "$",
    );
    expect(errors).toEqual([]);
  });

  it("QueryListToolbar with an inline QuerySchema", () => {
    const errors = validate(
      components.QueryListToolbar,
      {
        id: "list_toolbar",
        component: "QueryListToolbar",
        querySchema: {
          defaultField: "title",
          fields: [
            {
              key: "title",
              label: "标题",
              type: "string",
              operators: ["eq", "contains"],
            },
            {
              key: "status",
              label: "状态",
              type: "enum",
              operators: ["eq"],
              enumValues: ["open", "done"],
            },
            {
              key: "owner",
              label: "负责人",
              type: "reference",
              operators: ["eq"],
              reference: { entity: "user" },
            },
          ],
        },
        filterChangeAction: {
          event: { name: "filterChange", context: { conditions: [] } },
        },
      },
      "$",
    );
    expect(errors).toEqual([]);
  });

  it("DataTable with rowActions (row-scoped action contexts)", () => {
    const errors = validate(
      components.DataTable,
      {
        id: "todos_table",
        component: "DataTable",
        columns: [
          { key: "title", label: "Title" },
          { key: "completed", label: "Done" },
        ],
        rows: { path: "/todos" },
        emptyMessage: "Nothing yet",
        rowActions: [
          {
            label: "Toggle",
            action: {
              event: {
                name: "todo.toggle",
                context: {
                  id: { path: "id" },
                  completed: { path: "completed" },
                },
              },
            },
          },
          {
            label: "Delete",
            action: {
              event: { name: "todo.delete", context: { id: { path: "id" } } },
            },
          },
        ],
      },
      "$",
    );
    expect(errors).toEqual([]);
  });

  it("rejects invalid instances", () => {
    // Unknown property (icon was cut from the description surface).
    expect(
      validate(
        components.Button,
        {
          id: "b",
          component: "Button",
          child: "l",
          action: { event: { name: "x" } },
          icon: "search",
        },
        "$",
      ),
    ).not.toEqual([]);
    // Missing required value.
    expect(
      validate(components.Input, { id: "i", component: "Input" }, "$"),
    ).not.toEqual([]);
    // Wrong discriminator.
    expect(
      validate(
        components.Button,
        {
          id: "b",
          component: "Modal",
          child: "l",
          action: { event: { name: "x" } },
        },
        "$",
      ),
    ).not.toEqual([]);
    // QueryFieldSpec missing operators.
    expect(
      validate(
        components.QueryListToolbar,
        {
          id: "t",
          component: "QueryListToolbar",
          querySchema: {
            defaultField: "title",
            fields: [{ key: "title", label: "标题", type: "string" }],
          },
          filterChangeAction: { event: { name: "filterChange" } },
        },
        "$",
      ),
    ).not.toEqual([]);
    // rowActions entry missing the label.
    expect(
      validate(
        components.DataTable,
        {
          id: "t",
          component: "DataTable",
          columns: [{ key: "title", label: "Title" }],
          rows: { path: "/todos" },
          rowActions: [{ action: { event: { name: "todo.delete" } } }],
        },
        "$",
      ),
    ).not.toEqual([]);
    // rowActions entry with a malformed action.
    expect(
      validate(
        components.DataTable,
        {
          id: "t",
          component: "DataTable",
          columns: [{ key: "title", label: "Title" }],
          rows: { path: "/todos" },
          rowActions: [{ label: "Delete", action: { event: {} } }],
        },
        "$",
      ),
    ).not.toEqual([]);
    // rowActions entry with an undeclared extra field.
    expect(
      validate(
        components.DataTable,
        {
          id: "t",
          component: "DataTable",
          columns: [{ key: "title", label: "Title" }],
          rows: { path: "/todos" },
          rowActions: [
            {
              label: "Delete",
              icon: "trash",
              action: { event: { name: "todo.delete" } },
            },
          ],
        },
        "$",
      ),
    ).not.toEqual([]);
  });
});
