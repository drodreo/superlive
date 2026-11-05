// superlive A2UI catalog — TS companion types.
//
// The authoritative source is the JSON Schema file next to this module
// (superlive.catalog.json, an A2UI v0.9.1 catalog distributable as a whole
// file, catalogId "superlive"). These types only ease programmatic
// construction and inspection of that file.

/** Component names declared by the superlive catalog */
export type CatalogComponentName =
  | "Button"
  | "Checkbox"
  | "ConfirmDialog"
  | "DataTable"
  | "EmptyState"
  | "ErrorMessage"
  | "FormDialog"
  | "Input"
  | "LoadingSpinner"
  | "Modal"
  | "Pagination"
  | "QueryListToolbar"
  | "SearchBar"
  | "Select"
  | "Text"
  | "Textarea";

/** A single component declaration under catalog.components */
export interface CatalogComponentDefinition {
  type: "object";
  /** [ComponentCommon ref, CatalogComponentCommon ref, component-specific object] */
  allOf: Record<string, unknown>[];
  unevaluatedProperties: false;
}

/** The superlive catalog file shape */
export interface SuperliveCatalog {
  $schema: string;
  $id: string;
  title: string;
  description: string;
  catalogId: string;
  components: Record<CatalogComponentName, CatalogComponentDefinition>;
  $defs: Record<string, unknown>;
}
