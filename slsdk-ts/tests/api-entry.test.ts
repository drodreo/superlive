import { describe, expect, it } from "vitest";

// Type-level pin: the api entry no longer exports the query model types —
// they are internal to the query component family now (architecture.md,
// the api-entry surface contract). If this ever compiles again, the
// consolidation has regressed.
import type * as ApiEntry from "../src/index";
// @ts-expect-error QuerySchema must not be exported from the api entry
export type _QuerySchemaFromApiEntry = ApiEntry.QuerySchema;

describe("api entry surface", () => {
  it("keeps exactly the HTTP convenience surface at runtime", async () => {
    const mod = await import("../src/index");
    expect(Object.keys(mod).sort()).toEqual([
      "ApiError",
      "SdkResponse",
      "createClient",
    ]);
  });
});
