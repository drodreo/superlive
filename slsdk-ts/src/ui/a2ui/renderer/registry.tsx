/**
 * Preset superlive vocabulary wired into the renderer (batch 2 complete set:
 * all 16 catalog components).
 *
 * Every entry is thin glue over the existing controlled component — the
 * components themselves stay protocol-unaware and untouched; the render
 * context carries all A2UI semantics (evaluated props in, local write-back
 * for keystrokes, declared actions out, ChildList slots as declared
 * subtrees). Interaction products the description cannot declare statically
 * (selected page, assembled filter, submitted form values, clicked sort
 * column) ride into the event context through `dispatchAction`'s
 * `extraContext` — the catalog descriptions promise these "returned in the
 * event context". Unregistered names degrade through the error channel.
 */
import { useState } from "react";
import {
  Button,
  Checkbox,
  ConfirmDialog,
  DataTable,
  EmptyState,
  ErrorMessage,
  FormDialog,
  FormField,
  Input,
  LoadingSpinner,
  Modal,
  Pagination,
  SearchBar,
  Select,
  Text,
  Textarea,
  type FormSchema,
  type InputProps,
} from "../../components";
import { QueryListToolbar } from "../../components/query/QueryListToolbar";
import {
  A2uiRegistry,
  bindAction,
  bindControlled,
  type ComponentRenderContext,
} from "./react/wiring";

/**
 * A fresh registry pre-wired with the full preset superlive vocabulary.
 * Hosts and component authors take their own instance from here when they
 * need to register custom controls — mutating a shared map would leak the
 * customization into every surface using the default.
 */
export function createPresetRegistry(): A2uiRegistry {
  return new A2uiRegistry([
    [
      "Input",
      bindControlled((ctx) => (
        <Input
          value={ctx.get<string>("value") ?? ""}
          onChange={(value) => ctx.writeBack("value", value)}
          placeholder={ctx.get<string>("placeholder")}
          type={ctx.get<InputProps["type"]>("type")}
          disabled={ctx.get<boolean>("disabled")}
          error={ctx.get<string>("error")}
        />
      )),
    ],
    [
      "Textarea",
      bindControlled((ctx) => (
        <Textarea
          value={ctx.get<string>("value") ?? ""}
          onChange={(value) => ctx.writeBack("value", value)}
          placeholder={ctx.get<string>("placeholder")}
          rows={ctx.get<number>("rows")}
          disabled={ctx.get<boolean>("disabled")}
          error={ctx.get<string>("error")}
        />
      )),
    ],
    [
      "SearchBar",
      bindControlled((ctx) => (
        <SearchBar
          value={ctx.get<string>("value") ?? ""}
          onChange={(value) => ctx.writeBack("value", value)}
          placeholder={ctx.get<string>("placeholder")}
          debounceMs={ctx.get<number>("debounceMs")}
          showAdvanced={ctx.get<boolean>("showAdvanced")}
          onAdvanced={() => ctx.dispatchAction("advancedAction")}
        />
      )),
    ],
    [
      "Checkbox",
      bindControlled((ctx) => (
        <Checkbox
          checked={ctx.get<boolean>("checked") ?? false}
          onChange={(checked) => ctx.writeBack("checked", checked)}
          label={ctx.get<string>("label")}
          disabled={ctx.get<boolean>("disabled")}
        />
      )),
    ],
    [
      "Select",
      bindControlled((ctx) => {
        // Unselected is expressed as an empty data model value (catalog
        // description); the component speaks string | null.
        const value = ctx.get<string>("value");
        return (
          <Select
            value={value ? value : null}
            onChange={(v) => ctx.writeBack("value", v ?? "")}
            options={ctx.get<SelectOptions>("options") ?? []}
            placeholder={ctx.get<string>("placeholder")}
            disabled={ctx.get<boolean>("disabled")}
          />
        );
      }),
    ],
    [
      "Pagination",
      bindAction((ctx) => (
        <Pagination
          page={ctx.get<number>("page") ?? 1}
          total={ctx.get<number>("total") ?? 0}
          pageSize={ctx.get<number>("pageSize")}
          onChange={(page) => {
            ctx.writeBack("page", page);
            ctx.dispatchAction("changeAction", { page });
          }}
        />
      )),
    ],
    [
      "Text",
      bindControlled((ctx) => (
        <Text
          text={ctx.get<string>("text") ?? ""}
          variant={ctx.get<TextVariant>("variant")}
        />
      )),
    ],
    [
      "Button",
      bindAction((ctx) => (
        <Button
          variant={ctx.get<
            "primary" | "secondary" | "ghost" | "danger" | "info"
          >("variant")}
          size={ctx.get<"sm" | "md" | "lg">("size")}
          loading={ctx.get<boolean>("loading")}
          block={ctx.get<boolean>("block")}
          onClick={() => ctx.dispatchAction("action")}
        >
          {ctx.renderChild()}
        </Button>
      )),
    ],
    [
      "LoadingSpinner",
      bindControlled((ctx) => (
        <LoadingSpinner
          size={ctx.get<"sm" | "md" | "lg">("size")}
          message={ctx.get<string>("message")}
          overlay={ctx.get<boolean>("overlay")}
        />
      )),
    ],
    [
      "EmptyState",
      bindControlled((ctx) => (
        <EmptyState
          icon={ctx.get<"search" | "data" | "file">("icon")}
          title={ctx.get<string>("title") ?? ""}
          description={ctx.get<string>("description")}
          action={ctx.renderChild()}
        />
      )),
    ],
    [
      "ErrorMessage",
      bindAction((ctx) => (
        <ErrorMessage
          error={ctx.get<string>("error") ?? null}
          title={ctx.get<string>("title")}
          onRetry={() => ctx.dispatchAction("retryAction")}
        />
      )),
    ],
    [
      "Modal",
      bindControlled((ctx) => (
        <Modal
          open={ctx.get<boolean>("open") ?? false}
          onClose={() => ctx.dispatchAction("closeAction")}
          title={ctx.get<string>("title") ?? ""}
          footer={ctx.renderChildren("footer")}
        >
          {ctx.renderChildren("children")}
        </Modal>
      )),
    ],
    [
      "ConfirmDialog",
      bindAction((ctx) => (
        <ConfirmDialog
          open={ctx.get<boolean>("open") ?? false}
          onClose={() => ctx.dispatchAction("cancelAction")}
          onConfirm={() => ctx.dispatchAction("confirmAction")}
          title={ctx.get<string>("title") ?? ""}
          message={ctx.get<string>("message") ?? ""}
          confirmText={ctx.get<string>("confirmText")}
          cancelText={ctx.get<string>("cancelText")}
          danger={ctx.get<boolean>("danger")}
          warningText={ctx.get<string>("warningText")}
          loading={ctx.get<boolean>("loading")}
        />
      )),
    ],
    ["FormDialog", bindAction((ctx) => <WiredFormDialog ctx={ctx} />)],
    [
      "QueryListToolbar",
      bindAction((ctx) => (
        <QueryListToolbar
          // Required by the catalog; the validate pass guarantees presence.
          querySchema={ctx.get<QuerySchema>("querySchema") as QuerySchema}
          defaultIncludeDeleted={ctx.get<boolean>("defaultIncludeDeleted")}
          onFilterChange={(filter) =>
            ctx.dispatchAction("filterChangeAction", { filter })
          }
        />
      )),
    ],
    [
      "DataTable",
      bindControlled((ctx) => {
        // Row actions ride in verbatim (literal-class prop): the declarations
        // carry un-evaluated bindings that resolve against each row's scope.
        const rowActionDecls =
          ctx.get<{ label: unknown; action: unknown }[]>("rowActions") ?? [];
        return (
          <DataTable
            columns={ctx.get<DataTableColumns>("columns") ?? []}
            rows={ctx.get<Record<string, unknown>[]>("rows") ?? []}
            sortKey={ctx.get<string>("sortKey")}
            sortDir={ctx.get<"asc" | "desc">("sortDir")}
            emptyMessage={ctx.get<string>("emptyMessage")}
            onSort={(key) => ctx.dispatchAction("sortAction", { key })}
            rowActions={
              rowActionDecls.length > 0
                ? rowActionDecls.map((decl) => ({
                    label: (rowIndex: number) =>
                      String(
                        ctx.resolveRowValue("rows", decl.label, rowIndex) ?? "",
                      ),
                    onAction: (rowIndex: number) =>
                      ctx.dispatchRowAction("rows", decl.action, rowIndex),
                  }))
                : undefined
            }
          />
        );
      }),
    ],
  ]);
}

/**
 * Shared preset singleton backing <A2uiSurface>'s default vocabulary.
 * Read-only by convention: consumers registering custom controls must take
 * their own instance via createPresetRegistry().
 */
export const defaultRegistry: A2uiRegistry = createPresetRegistry();

type SelectOptions = Parameters<typeof Select>[0]["options"];
type TextVariant = "h1" | "h2" | "h3" | "h4" | "h5" | "caption" | "body";
type QuerySchema = Parameters<typeof QueryListToolbar>[0]["querySchema"];
type DataTableColumns = Parameters<typeof DataTable>[0]["columns"];

/**
 * FormDialog wiring: the body is either a declared component subtree
 * (`children`) or the data-driven `formSchema` (the two are alternatives per
 * the catalog). The formSchema branch owns the field values locally — the
 * submit action carries them in the event context as `values`
 * (Record<field.key, value>); initial seeding stays a host-side
 * updateDataModel + children-form concern, the data-driven form starts
 * empty.
 */
function WiredFormDialog({ ctx }: { ctx: ComponentRenderContext }) {
  const formSchema = ctx.get<FormSchema>("formSchema");
  const [values, setValues] = useState<Record<string, string | boolean>>({});

  const setField = (key: string, value: string | boolean): void => {
    setValues((s) => ({ ...s, [key]: value }));
  };

  return (
    <FormDialog
      open={ctx.get<boolean>("open") ?? false}
      onClose={() => ctx.dispatchAction("cancelAction")}
      onSubmit={() => ctx.dispatchAction("submitAction", { values })}
      title={ctx.get<string>("title") ?? ""}
      submitText={ctx.get<string>("submitText")}
      cancelText={ctx.get<string>("cancelText")}
      loading={ctx.get<boolean>("loading")}
      submitDisabled={ctx.get<boolean>("submitDisabled")}
    >
      {formSchema
        ? formSchema.fields.map((field) => (
            <FormField
              key={field.key}
              label={field.label}
              required={field.required}
              hint={field.hint}
            >
              {schemaFieldInput(field, values[field.key], setField)}
            </FormField>
          ))
        : ctx.renderChildren("children")}
    </FormDialog>
  );
}

type SchemaField = FormSchema["fields"][number];

/**
 * One schema-declared field → its input control. Served tiers: input /
 * textarea / select / checkbox map straight onto the facility controls;
 * datetime degrades to an ISO text input (the datetime-local enhancement
 * belongs to the CrudListPage human path; A2UI keeps a single input shape);
 * reference / directory / file depend on host-injected data sources
 * (references table, file probe) the description cannot carry, so they
 * degrade to plain text inputs.
 */
function schemaFieldInput(
  field: SchemaField,
  value: string | boolean | undefined,
  setField: (key: string, value: string | boolean) => void,
) {
  const text = value === undefined || value === false ? "" : String(value);
  switch (field.type) {
    case "textarea":
      return (
        <Textarea
          value={text}
          onChange={(v) => setField(field.key, v)}
          placeholder={field.placeholder}
          disabled={field.disabled}
        />
      );
    case "select":
      return (
        <Select
          value={text ? text : null}
          onChange={(v) => setField(field.key, v ?? "")}
          options={field.options ?? []}
          placeholder={field.placeholder ?? "请选择"}
          disabled={field.disabled}
        />
      );
    case "checkbox":
      return (
        <Checkbox
          checked={value === true}
          onChange={(checked) => setField(field.key, checked)}
          disabled={field.disabled}
        />
      );
    default:
      // input / datetime / reference / directory / file
      return (
        <Input
          value={text}
          onChange={(v) => setField(field.key, v)}
          placeholder={field.placeholder}
          disabled={field.disabled}
        />
      );
  }
}
